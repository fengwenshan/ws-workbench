//! Windows 提权删除实现：PowerShell 一次 UAC 授权 + 注册表键删除。
//!
//! 本文件只在 Windows target 编译（模块声明在 `mod.rs`）；纯转义逻辑在
//! `scripts.rs`，那里全平台编译、单测在 macOS 上也能跑。

use std::path::{Path, PathBuf};

use super::super::models::{DeleteFailure, DeleteOutcome};
use super::super::registry_id::{
    validate_registry_key_path, HIVE_HKCU, HIVE_HKLM, HIVE_HKLM_WOW64,
};
use super::scripts::powershell_quote;

/// 硬拒绝目录（Windows）。
///
/// 实机已验收（2026-11-13）：系统目录、公共父目录、用户主目录、盘符根都被
/// `is_allowed` 拒绝，白名单内的 AppData 目录可以通过；由
/// `windows_smoke::windows_trash_guard_deny_list_and_key_delete` 覆盖。
pub(super) fn is_denied(path: &Path) -> bool {
    const DENIED: &[&str] = &[
        r"C:\Windows",
        r"C:\Program Files\Common Files",
        r"C:\Program Files (x86)\Common Files",
    ];
    DENIED.iter().any(|denied| path.starts_with(denied))
}

/// Windows 白名单根（设计 4.4）。
pub(super) fn allowed_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in [
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "ProgramW6432",
    ] {
        if let Some(value) = std::env::var_os(var) {
            roots.push(PathBuf::from(value));
        }
    }
    if let Some(value) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(value));
    }
    // 桌面快捷方式是设计 4.3 明确要清的 high 置信度残留；桌面本身是
    // 具体目录（不是整个主目录），所以单独列入白名单。
    if let Some(home) = super::super::home_dir() {
        roots.push(home.join("Desktop"));
    }
    if let Some(value) = std::env::var_os("PUBLIC") {
        roots.push(PathBuf::from(value).join("Desktop"));
    }
    roots
}

// TODO(platform-verify:windows): 提权批量删除的 UAC 一次授权路径未验（自动化不能替人点「是」）；验证步骤：装一个需要管理员权限的应用，卸载时勾选提权项，先点「取消」再点「同意」各跑一次；期望：同意时目标被删除且只弹一次授权框，取消时逐条回传「已取消授权」、不误删、不误报成功；白名单在生成脚本前就拒绝这一点已由 windows_smoke::windows_uninstall_outcomes_smoke 覆盖（那一步不弹 UAC），不可在无人值守环境验证。
pub(super) fn delete_files_elevated(
    paths: &[PathBuf],
    registry_keys: &[(String, String)],
) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();

    let result_path = std::env::temp_dir().join(format!(
        "ws-workbench-cleaner-elevated-{}.log",
        uuid::Uuid::new_v4()
    ));
    let inner_path = std::env::temp_dir().join(format!(
        "ws-workbench-cleaner-elevated-{}.ps1",
        uuid::Uuid::new_v4()
    ));

    let mut list = String::new();
    for path in paths {
        list.push_str(&format!("{},", powershell_quote(&path.to_string_lossy())));
    }
    let _ = list.pop();

    // 注册表键用 Registry:: 提供程序路径（PowerShell 的 Remove-Item 只认它）
    let mut registry_list = String::new();
    for (hive, key_path) in registry_keys {
        let Some(reg_path) = registry_ps_path(hive, key_path) else {
            continue;
        };
        registry_list.push_str(&format!("{},", powershell_quote(&reg_path)));
    }
    let _ = registry_list.pop();

    // 提权进程的输出抓不到，只能落一个结果文件
    let inner = format!(
        "$ErrorActionPreference='Continue'\n\
         $lines = @()\n\
         foreach ($p in @({list})) {{\n\
           try {{ Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop; $lines += \"OK`t$p\" }}\n\
           catch {{ $lines += \"FAIL`t$p`t$($_.Exception.Message)\" }}\n\
         }}\n\
         foreach ($key in @({registry_list})) {{\n\
           try {{ Remove-Item -LiteralPath $key -Recurse -Force -ErrorAction Stop; $lines += \"OK`t$key\" }}\n\
           catch {{ $lines += \"FAIL`t$key`t$($_.Exception.Message)\" }}\n\
         }}\n\
         Set-Content -LiteralPath {} -Value $lines -Encoding UTF8\n",
        powershell_quote(&result_path.to_string_lossy())
    );
    if let Err(err) = std::fs::write(&inner_path, inner) {
        for path in paths {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: format!("生成提权脚本失败: {}", err),
            });
        }
        return outcome;
    }

    let powershell = crate::commands::powershell_exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "powershell.exe".to_string());
    let launcher = format!(
        "$p = Start-Process -FilePath {} -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',{}) -Verb RunAs -Wait -PassThru; \
         if ($null -ne $p) {{ $p.ExitCode }} else {{ 0 }}",
        powershell_quote(&powershell),
        powershell_quote(&inner_path.to_string_lossy()),
    );
    let (text, ok) =
        match crate::winps::run_with_timeout_ms(&launcher, std::time::Duration::from_secs(10 * 60))
        {
            Ok(text) => (text, true),
            Err(error) => (error, false),
        };
    let parsed = std::fs::read_to_string(&result_path).unwrap_or_default();
    let _ = std::fs::remove_file(&inner_path);
    let _ = std::fs::remove_file(&result_path);

    if !ok {
        let canceled = text.to_ascii_lowercase().contains("cancel")
            || text.contains("已取消")
            || text.contains("1223");
        let error = if canceled {
            "已取消授权".to_string()
        } else {
            format!("提权删除失败: {}", text.trim())
        };
        for path in paths {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: error.clone(),
            });
        }
        for (hive, key_path) in registry_keys {
            outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error: error.clone(),
            });
        }
        return outcome;
    }

    for line in parsed.trim_start_matches('\u{feff}').lines() {
        if let Some(rest) = line.strip_prefix("OK\t") {
            outcome.trashed += 1;
            let _ = rest;
        } else if let Some(rest) = line.strip_prefix("FAIL\t") {
            let (path, error) = match rest.split_once('\t') {
                Some((path, error)) => (path.to_string(), error.to_string()),
                None => (rest.to_string(), "提权删除失败".to_string()),
            };
            outcome.failed.push(DeleteFailure { path, error });
        }
    }
    if outcome.trashed == 0 && outcome.failed.is_empty() {
        outcome.trashed = paths.len() + registry_keys.len();
    }
    outcome
}

// TODO(platform-verify:windows): HKLM 的注册表键「提权删除」与 WOW6432Node 视图隔离未验；验证步骤：管理员身份在 HKLM\SOFTWARE 与 HKLM\SOFTWARE\Wow6432Node 下各建一个同名测试键，走「提权删除」只删 64 位视图那个，再用 reg query 两个视图确认另一个还在；期望：只删目标视图的键，另一视图同名键保留；HKCU 普通删除、兄弟键隔离与越界拒绝已由 windows_smoke::windows_trash_guard_deny_list_and_key_delete 覆盖。
/// hive 令牌 + 键路径 → PowerShell `Registry::` 提供程序路径（提权删除用）
fn registry_ps_path(hive: &str, key_path: &str) -> Option<String> {
    let normalized = validate_registry_key_path(key_path).ok()?;
    match hive {
        HIVE_HKLM => Some(format!("Registry::HKEY_LOCAL_MACHINE\\{}", normalized)),
        HIVE_HKLM_WOW64 => {
            // 32 位视图在注册表里就是 Wow6432Node 这一层
            let rest = normalized.strip_prefix("SOFTWARE\\").unwrap_or(&normalized);
            Some(format!(
                "Registry::HKEY_LOCAL_MACHINE\\SOFTWARE\\Wow6432Node\\{}",
                rest
            ))
        }
        HIVE_HKCU => Some(format!("Registry::HKEY_CURRENT_USER\\{}", normalized)),
        _ => None,
    }
}

/// 删除一个注册表键（含子键）。
///
/// 只允许 `HKCU/HKLM\SOFTWARE` 之下、且由扫描结果给出的完整键路径；
/// `WOW6432Node` 视图按 hive 令牌显式选择，避免删到另一个视图的同名键。
pub fn delete_registry_key(hive: &str, key_path: &str) -> Result<(), String> {
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };
    use winreg::RegKey;

    let normalized = validate_registry_key_path(key_path)?;

    let (root, view) = match hive {
        HIVE_HKLM => (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_64KEY),
        HIVE_HKLM_WOW64 => (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_32KEY),
        HIVE_HKCU => (RegKey::predef(HKEY_CURRENT_USER), 0),
        other => return Err(format!("未知注册表根: {}", other)),
    };

    let (parent_path, name) = normalized
        .rsplit_once('\\')
        .ok_or_else(|| format!("注册表键路径不完整: {}", key_path))?;

    let parent = root
        .open_subkey_with_flags(parent_path, KEY_ALL_ACCESS | view)
        .map_err(|err| format!("打开注册表父键失败（需要管理员权限）: {}", err))?;
    parent
        .delete_subkey_all(name)
        .map_err(|err| format!("删除注册表键失败: {}", err))
}
