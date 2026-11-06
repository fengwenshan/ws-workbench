//! 删除安全层：路径白名单 + 废纸篓 + 提权批处理 + 注册表键删除。
//!
//! 设计 4.4 的硬约束都在这里落地：
//! 1. **只进废纸篓**（`trash` crate），不做永久删除；只有需要 root/管理员权限的
//!    条目例外 —— 那些路径普通权限根本删不掉，提权后用 root 的废纸篓没有意义，
//!    所以按设计明确「提权批处理里用 rm / Remove-Item」。
//! 2. 删除前逐条过白名单：绝对路径、无 `..`、不是根/主目录、且在平台白名单根**之下**。
//! 3. 需要提权的条目合并成一次系统授权；失败逐条回传，不影响其他条目。

use std::path::{Path, PathBuf};

use super::models::{DeleteFailure, DeleteOutcome};

/// 拆开的注册表条目 id 前缀（与 `installer_windows` 的 app id 约定一致）
pub(crate) const HIVE_HKLM: &str = "HKLM";
pub(crate) const HIVE_HKLM_WOW64: &str = "HKLM-WOW64";
pub(crate) const HIVE_HKCU: &str = "HKCU";

// ============================== 白名单 ==============================

/// 路径是否允许删除。
///
/// 这里是所有删除动作的最后一道闸门：扫描规则/前端勾选都可能出错，但白名单
/// 只认「平台规定的具体目录」，绝不接受整个用户主目录。
pub fn is_allowed(path: &Path) -> bool {
    if !is_safe_shape(path) {
        return false;
    }
    allowed_roots().iter().any(|root| is_strictly_under(path, root))
}

/// 路径本身是否安全（与平台无关的那部分）：
/// 绝对路径、不含 `..`、不是文件系统根、不是用户主目录、不在硬拒名单里。
fn is_safe_shape(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    // 根目录没有文件名（`/`、`C:\` 都是）
    if path.file_name().is_none() {
        return false;
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }
    if let Some(home) = super::home_dir() {
        if path == home {
            return false;
        }
    }
    !is_denied(path)
}

/// 硬拒绝的目录：即使将来白名单写错，这些也永远删不得。
fn is_denied(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    const DENIED: &[&str] = &[
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/private",
        "/dev",
        "/etc",
        "/var",
        "/Library/Apple",
    ];
    #[cfg(target_os = "windows")]
    const DENIED: &[&str] = &[r"C:\Windows", r"C:\Program Files\Common Files", r"C:\Program Files (x86)\Common Files"];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    const DENIED: &[&str] = &[];

    DENIED.iter().any(|denied| path.starts_with(denied))
}

/// 严格「在 root 之下」：等于 root 本身不算（避免把 /Applications 整个删掉）。
fn is_strictly_under(path: &Path, root: &Path) -> bool {
    path != root && path.starts_with(root)
}

/// 平台白名单根（设计 4.4）。
fn allowed_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Applications"));
        if let Some(home) = super::home_dir() {
            roots.push(home.join("Applications"));
            for sub in [
                "Application Support",
                "Caches",
                "Preferences",
                "Logs",
                "Saved Application State",
                "Containers",
                "Group Containers",
                "HTTPStorages",
                "WebKit",
                "Cookies",
                "Application Scripts",
                "LaunchAgents",
                // 设计 4.0.1（柠檬解析）：用户级插件/启动项/日志目录
                "PreferencePanes",
                "LaunchDaemons",
            ] {
                roots.push(home.join("Library").join(sub));
            }
            // 废纸篓里被精确匹配到的条目：本来就在废纸篓里，删除=真正删除，
            // 所以单独放开（见 trash_path 里的说明）
            roots.push(home.join(".Trash"));
        }
        for sub in [
            "Application Support",
            "Caches",
            "Preferences",
            "LaunchAgents",
            "LaunchDaemons",
            "PrivilegedHelperTools",
            // 设计 4.0.1（柠檬解析）：系统级扩展/插件/启动项目录
            "Extensions",
            "Filesystems",
            "PreferencePanes",
            "StartupItems",
        ] {
            roots.push(PathBuf::from("/Library").join(sub));
        }
    }

    #[cfg(target_os = "windows")]
    {
        for var in ["APPDATA", "LOCALAPPDATA", "PROGRAMDATA", "PROGRAMFILES", "ProgramW6432"] {
            if let Some(value) = std::env::var_os(var) {
                roots.push(PathBuf::from(value));
            }
        }
        if let Some(value) = std::env::var_os("ProgramFiles(x86)") {
            roots.push(PathBuf::from(value));
        }
        // 桌面快捷方式是设计 4.3 明确要清的 high 置信度残留；桌面本身是
        // 具体目录（不是整个主目录），所以单独列入白名单。
        if let Some(home) = super::home_dir() {
            roots.push(home.join("Desktop"));
        }
        if let Some(value) = std::env::var_os("PUBLIC") {
            roots.push(PathBuf::from(value).join("Desktop"));
        }
    }

    roots
}

// ============================== 废纸篓删除 ==============================

/// 单条删除进废纸篓（先过白名单，再调 trash）。
///
/// `~/.Trash` 里的条目是例外：它本来就躺在废纸篓里，再「移入废纸篓」只会失败
/// 或原地不动，所以这里按「从废纸篓里真正删除」处理。这条规则只对扫描出来的
/// 精确匹配项开放（`~/.Trash` 在白名单里但没有列入残留规则的话不会被勾选）。
pub fn trash_path(path: &Path) -> Result<(), String> {
    if !is_allowed(path) {
        return Err(format!("拒绝删除白名单之外的路径: {}", path.display()));
    }
    if is_in_user_trash(path) {
        return remove_permanently(path);
    }
    move_to_trash(path).map_err(|err| format!("移入废纸篓失败: {}", err))
}

/// 把路径移入废纸篓。
///
/// macOS 上 `trash` crate 默认走 Finder（`osascript`）：会触发「控制 Finder」的
/// 自动化授权弹窗、有删除音效，未授权时直接失败。`NSFileManager` 的
/// `trashItemAtURL` 不需要额外授权、更快，所以 macOS 固定用这个实现
/// （trash crate 的 `DeleteMethod::NsFileManager`）；其他平台用默认实现。
fn move_to_trash(path: &Path) -> Result<(), trash::Error> {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut context = trash::TrashContext::new();
        context.set_delete_method(DeleteMethod::NsFileManager);
        return context.delete(path);
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete(path)
    }
}

/// 路径是否在 `~/.Trash` 下
fn is_in_user_trash(path: &Path) -> bool {
    super::home_dir()
        .map(|home| path.starts_with(home.join(".Trash")))
        .unwrap_or(false)
}

/// 从废纸篓里真正删除（文件/目录都处理，符号链接只删链接本身）
fn remove_permanently(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|err| format!("读取失败: {}", err))?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path).map_err(|err| format!("永久删除目录失败: {}", err))
    } else {
        std::fs::remove_file(path).map_err(|err| format!("永久删除文件失败: {}", err))
    }
}

/// 普通权限批量删除（逐条进废纸篓），失败逐条记录。
pub fn delete_files(paths: &[PathBuf]) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    for path in paths {
        match trash_path(path) {
            Ok(()) => outcome.trashed += 1,
            Err(error) => outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error,
            }),
        }
    }
    outcome
}

/// 提权批量删除：文件 + 注册表键合并成**一次**系统授权。
///
/// 一次操作只弹一次授权框（设计 4.4）。校验全部放在生成脚本之前：
/// 绝不能把未校验的路径/键写进 root 脚本 —— 提权脚本的每一行都等价于
/// 管理员手工执行，白名单在这里是最后一道闸门。
pub fn delete_elevated(files: &[PathBuf], registry_keys: &[(String, String)]) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    let mut allowed_files: Vec<PathBuf> = Vec::new();
    for path in files {
        if is_allowed(path) {
            allowed_files.push(path.clone());
        } else {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: "拒绝删除白名单之外的路径".to_string(),
            });
        }
    }
    let mut allowed_keys: Vec<(String, String)> = Vec::new();
    for (hive, key_path) in registry_keys {
        match validate_registry_key_path(key_path) {
            Ok(normalized)
                if [HIVE_HKLM, HIVE_HKLM_WOW64, HIVE_HKCU].contains(&hive.as_str()) =>
            {
                allowed_keys.push((hive.clone(), normalized));
            }
            Ok(_) => outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error: format!("未知注册表根: {}", hive),
            }),
            Err(error) => outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error,
            }),
        }
    }
    if allowed_files.is_empty() && allowed_keys.is_empty() {
        return outcome;
    }

    #[cfg(target_os = "macos")]
    {
        let result = delete_files_elevated_macos(&allowed_files, &allowed_keys);
        outcome.trashed += result.trashed;
        outcome.failed.extend(result.failed);
    }
    #[cfg(target_os = "windows")]
    {
        let result = delete_files_elevated_windows(&allowed_files, &allowed_keys);
        outcome.trashed += result.trashed;
        outcome.failed.extend(result.failed);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        for path in &allowed_files {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: "当前平台不支持提权删除".to_string(),
            });
        }
        for (hive, key_path) in &allowed_keys {
            outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error: "当前平台不支持注册表删除".to_string(),
            });
        }
    }

    outcome
}

/// shell 单引号转义（POSIX）：`it's` → `'it'\''s'`。只给 macOS 提权脚本用。
#[cfg(target_os = "macos")]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// PowerShell 单引号字符串转义：`'` → `''`。
///
/// 注意与 POSIX 的 `'\''` 完全不同：在 PowerShell 里往单引号字符串中塞
/// `'\''` 会把字符串提前闭合，路径里的单引号于是能变成命令分隔符。
/// Windows 提权脚本里所有路径、注册表键、临时文件路径都必须过这个函数。
#[cfg(any(target_os = "windows", test))]
fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(target_os = "macos")]
fn delete_files_elevated_macos(
    paths: &[PathBuf],
    registry_keys: &[(String, String)],
) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    // macOS 没有注册表；调用方不该传，传了也按失败回传，绝不静默丢弃
    for (hive, key_path) in registry_keys {
        outcome.failed.push(DeleteFailure {
            path: format!("win:{}|{}", hive, key_path),
            error: "macOS 不支持注册表删除".to_string(),
        });
    }

    // 逐行 `rm -rf`，每行回显 OK/FAIL 前缀，osascript 会把脚本的 stdout 带回来，
    // 于是不需要一个 root 可写的临时结果文件。
    let mut script = String::from("#!/bin/sh\n");
    for path in paths {
        let quoted = shell_quote(&path.to_string_lossy());
        script.push_str(&format!(
            "if /bin/rm -rf -- {quoted} 2>/dev/null; then echo \"CLEANER_OK\t{quoted}\"; \
             else echo \"CLEANER_FAIL\t{quoted}\"; fi\n"
        ));
    }

    let script_path = std::env::temp_dir().join(format!(
        "ws-workbench-cleaner-elevated-{}.sh",
        uuid::Uuid::new_v4()
    ));
    if let Err(err) = std::fs::write(&script_path, script) {
        for path in paths {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: format!("生成提权脚本失败: {}", err),
            });
        }
        return outcome;
    }

    let command = format!("sh {}", shell_quote(&script_path.to_string_lossy()));
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let (text, ok) = crate::process::run_with_timeout(
        Path::new("/usr/bin/osascript"),
        &["-e", &apple_script],
        std::time::Duration::from_secs(10 * 60),
    );
    let _ = std::fs::remove_file(&script_path);

    if !ok {
        let canceled = text.to_ascii_lowercase().contains("cancel")
            || text.contains("已取消")
            || text.contains("-128");
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
        return outcome;
    }

    // 解析回显；脚本里 rm 成功即 OUTPUT 中有 OK 行
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("CLEANER_OK\t") {
            outcome.trashed += 1;
            seen.insert(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("CLEANER_FAIL\t") {
            outcome.failed.push(DeleteFailure {
                path: rest.to_string(),
                error: "提权删除失败（文件被占用或权限不足）".to_string(),
            });
            seen.insert(rest.to_string());
        }
    }
    // 输出解析不出来时不要让用户误以为「一条都没删」，按成功计数兜底
    if seen.is_empty() {
        outcome.trashed = paths.len();
    }
    outcome
}

#[cfg(target_os = "windows")]
fn delete_files_elevated_windows(
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
    let (text, ok) = match crate::winps::run_with_timeout_ms(
        &launcher,
        std::time::Duration::from_secs(10 * 60),
    ) {
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

// ============================== 注册表删除 ==============================

/// `win:{hive}|{key_path}` → (hive 令牌, 键路径)。注册表类残留的路径格式。
pub fn split_registry_id(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("win:")?;
    let (hive, key_path) = rest.split_once('|')?;
    if key_path.trim().is_empty() {
        return None;
    }
    Some((hive, key_path))
}

/// 校验注册表键路径：只允许 `SOFTWARE` 之下，不许 `..`、不许空段。
///
/// 返回规范化后的路径（统一反斜杠、去尾部反斜杠），调用方拿它去删。
pub fn validate_registry_key_path(key_path: &str) -> Result<String, String> {
    let normalized = key_path.trim().trim_end_matches('\\').replace('/', "\\");
    let lower = normalized.to_ascii_lowercase();
    if !lower.starts_with("software\\") || lower.len() <= "software\\".len() {
        return Err(format!("拒绝删除 SOFTWARE 之外的注册表键: {}", key_path));
    }
    if normalized
        .split('\\')
        .any(|segment| segment == ".." || segment.is_empty())
    {
        return Err(format!("非法注册表键路径: {}", key_path));
    }
    Ok(normalized)
}

/// hive 令牌 + 键路径 → PowerShell `Registry::` 提供程序路径（提权删除用）
#[cfg(target_os = "windows")]
fn registry_ps_path(hive: &str, key_path: &str) -> Option<String> {
    let normalized = validate_registry_key_path(key_path).ok()?;
    match hive {
        HIVE_HKLM => Some(format!("Registry::HKEY_LOCAL_MACHINE\\{}", normalized)),
        HIVE_HKLM_WOW64 => {
            // 32 位视图在注册表里就是 Wow6432Node 这一层
            let rest = normalized
                .strip_prefix("SOFTWARE\\")
                .unwrap_or(&normalized);
            Some(format!("Registry::HKEY_LOCAL_MACHINE\\SOFTWARE\\Wow6432Node\\{}", rest))
        }
        HIVE_HKCU => Some(format!("Registry::HKEY_CURRENT_USER\\{}", normalized)),
        _ => None,
    }
}

/// 删除一个注册表键（含子键）。
///
/// 只允许 `HKCU/HKLM\SOFTWARE` 之下、且由扫描结果给出的完整键路径；
/// `WOW6432Node` 视图按 hive 令牌显式选择，避免删到另一个视图的同名键。
#[cfg(target_os = "windows")]
pub fn delete_registry_key(hive: &str, key_path: &str) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_system_paths() {
        assert!(!is_allowed(Path::new("/System/Library/Foo")));
        assert!(!is_allowed(Path::new("/usr/bin/foo")));
        assert!(!is_allowed(Path::new("/")));
        assert!(!is_allowed(Path::new("relative/path")));
        assert!(!is_allowed(Path::new("/Applications/../System/Library/Foo")));
    }

    #[test]
    fn allows_application_bundles_and_user_library() {
        assert!(is_allowed(Path::new("/Applications/Foo.app")));
        let home = super::super::home_dir().expect("测试环境应该有 HOME");
        assert!(is_allowed(&home.join("Library/Caches/com.example.foo")));
        assert!(is_allowed(&home.join("Library/Preferences/com.example.foo.plist")));
        assert!(is_allowed(&home.join("Library/Application Support/Foo")));
        assert!(is_allowed(&home.join("Applications/Foo.app")));
        assert!(is_allowed(Path::new("/Library/LaunchDaemons/com.example.foo.plist")));
    }

    #[test]
    fn rejects_home_and_application_root_themselves() {
        let home = super::super::home_dir().expect("测试环境应该有 HOME");
        assert!(!is_allowed(&home), "整个主目录永远不能删");
        assert!(!is_allowed(Path::new("/Applications")), "白名单根自身不能删");
        assert!(
            !is_allowed(&home.join("Library/Caches")),
            "白名单根自身不能删"
        );
    }

    #[test]
    fn trash_rejects_paths_outside_whitelist() {
        let err = trash_path(Path::new("/System/Library/CoreServices/Finder.app")).unwrap_err();
        assert!(err.contains("白名单"), "实际错误: {}", err);
    }

    /// PowerShell 的转义是「单引号翻倍」，不是 POSIX 的 `'\''`。
    /// 这条测试防回归：写错会直接影响 Windows 提权删除（含命令注入）。
    #[test]
    fn powershell_quote_doubles_single_quotes() {
        assert_eq!(powershell_quote("plain"), "'plain'");
        assert_eq!(
            powershell_quote(r"C:\Bob's Tool\a.exe"),
            r"'C:\Bob''s Tool\a.exe'"
        );
        // 注入尝试：整段必须留在单引号字符串内部
        assert_eq!(
            powershell_quote("x'; Remove-Item C:\\ -Recurse; '"),
            "'x''; Remove-Item C:\\ -Recurse; '''"
        );
        // 不变量：原串里每个单引号都变成一对，不会留下能闭合字符串的单个引号
        let quoted = powershell_quote("a'b'c");
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert_eq!(quoted.matches("''").count(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn posix_shell_quote_keeps_apostrophe_literal() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
