//! Windows 注册表读取：三处 Uninstall 键、单条记录字段解析、安装列表与运行检测候选。
//!
//! 这里也是同目录几个 Windows 实现共用 helper 的唯一来源（`pub(super)`）：
//! `read_string` / `open_entry_key` / `parse_app_id` / `display_icon_path` /
//! `is_forbidden_install_dir`，避免在 `uninstall.rs` / `leftovers.rs` / `related.rs`
//! 里复制第二份。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
};
use winreg::RegKey;

use crate::cleaner::models::{CleanerProgress, InstalledApp};

use super::parse::{
    dedupe, format_install_date, is_self_install, parse_estimated_size, should_show,
    uninstall_kind, Hive, UninstallEntry,
};

/// 三处数据源：HKLM 64 位视图、HKLM 的 Wow6432Node（32 位视图）、HKCU。
fn sources() -> Vec<(Hive, RegKey, &'static str, u32)> {
    vec![
        (
            Hive::Hklm,
            RegKey::predef(HKEY_LOCAL_MACHINE),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_WOW64_64KEY,
        ),
        (
            Hive::HklmWow64,
            RegKey::predef(HKEY_LOCAL_MACHINE),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_WOW64_32KEY,
        ),
        (
            Hive::Hkcu,
            RegKey::predef(HKEY_CURRENT_USER),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            0,
        ),
    ]
}

/// hive → (根, 注册表视图)
fn hive_root(hive: Hive) -> (RegKey, u32) {
    match hive {
        Hive::Hklm => (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_64KEY),
        Hive::HklmWow64 => (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_32KEY),
        Hive::Hkcu => (RegKey::predef(HKEY_CURRENT_USER), 0),
    }
}

pub(super) fn read_string(key: &RegKey, name: &str) -> String {
    if let Ok(value) = key.get_value::<String, _>(name) {
        return value;
    }
    // 有些安装器把版本号之类写成了 DWORD
    if let Ok(value) = key.get_value::<u32, _>(name) {
        return value.to_string();
    }
    String::new()
}

fn read_estimated_size(key: &RegKey) -> Option<String> {
    if let Ok(value) = key.get_value::<u32, _>("EstimatedSize") {
        return Some(value.to_string());
    }
    if let Ok(value) = key.get_value::<String, _>("EstimatedSize") {
        let trimmed = value.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

fn read_system_component(key: &RegKey) -> Option<String> {
    if let Ok(value) = key.get_value::<u32, _>("SystemComponent") {
        return Some(value.to_string());
    }
    if let Ok(value) = key.get_value::<String, _>("SystemComponent") {
        let trimmed = value.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

/// 枚举三处 Uninstall 键下的所有条目
///
/// 实机已验收（2026-11-13，Windows 11，Win32 清单第 1 条）：三视图原始条目数与
/// 列表量级一致、Wow6432Node 来源条目可见、过滤（系统组件/补丁）与去重生效，
/// 由 `windows_smoke::windows_list_matches_registry_views` 覆盖，原先的
/// 平台验证待办标记已按项目规则删除。
/// per-user（HKCU）条目的**写入**验证在 `windows_leftover_scan_and_recycle_smoke`。
fn enumerate_entries() -> Vec<(Hive, UninstallEntry)> {
    let mut entries: Vec<(Hive, UninstallEntry)> = Vec::new();
    for (hive, root, base_path, view) in sources() {
        let Ok(key) = root.open_subkey_with_flags(base_path, KEY_READ | view) else {
            // 某个视图打不开（极少数受限环境）不应该让整个列表失败
            continue;
        };
        for sub_name in key.enum_keys().flatten() {
            let Ok(sub) = key.open_subkey_with_flags(&sub_name, KEY_READ | view) else {
                continue;
            };
            let display_name = read_string(&sub, "DisplayName");
            if display_name.trim().is_empty() {
                continue;
            }
            let entry = UninstallEntry {
                display_name,
                display_version: read_string(&sub, "DisplayVersion"),
                publisher: read_string(&sub, "Publisher"),
                install_location: read_string(&sub, "InstallLocation"),
                display_icon: read_string(&sub, "DisplayIcon"),
                uninstall_string: read_string(&sub, "UninstallString"),
                quiet_uninstall_string: read_string(&sub, "QuietUninstallString"),
                estimated_size: read_estimated_size(&sub),
                install_date: read_string(&sub, "InstallDate"),
                system_component: read_system_component(&sub),
                parent_key_name: read_string(&sub, "ParentKeyName"),
                release_type: read_string(&sub, "ReleaseType"),
                key_path: format!(r"{}\{}", base_path, sub_name),
            };
            entries.push((hive, entry));
        }
    }
    entries
}

/// 从 app id 解析 (hive, 注册表键路径)
pub(super) fn parse_app_id(app_id: &str) -> Result<(Hive, &str), String> {
    let rest = app_id
        .strip_prefix("win:")
        .ok_or_else(|| format!("无效的应用标识: {}", app_id))?;
    let (hive_token, key_path) = rest
        .split_once('|')
        .ok_or_else(|| format!("无效的应用标识: {}", app_id))?;
    let hive =
        Hive::from_token(hive_token).ok_or_else(|| format!("未知的注册表根: {}", hive_token))?;
    if key_path.trim().is_empty() {
        return Err(format!("无效的应用标识: {}", app_id));
    }
    Ok((hive, key_path))
}

/// 按 hive + 键路径打开注册表项（卸载/残留扫描都要用）
pub(super) fn open_entry_key(hive: Hive, key_path: &str) -> Result<RegKey, String> {
    let (root, view) = hive_root(hive);
    root.open_subkey_with_flags(key_path, KEY_READ | view)
        .map_err(|err| format!("打开注册表项失败: {}", err))
}

/// `DisplayIcon` 里可能带 `,0` 图标索引和引号，取真正的文件路径
pub(super) fn display_icon_path(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return None;
    }
    let (path, _index) = match raw.rsplit_once(',') {
        Some((path, index)) if index.trim().parse::<i32>().is_ok() => (path, Some(index)),
        _ => (raw, None),
    };
    let path = Path::new(path.trim().trim_matches('"'));
    if path.as_os_str().is_empty() {
        return None;
    }
    Some(path.to_path_buf())
}

/// 大小：优先 EstimatedSize，其次递归统计 InstallLocation（最多 3 层，防卡死）
fn size_of_entry(entry: &UninstallEntry) -> u64 {
    let estimated = parse_estimated_size(entry.estimated_size.as_deref());
    if estimated > 0 {
        return estimated;
    }
    let location = entry.install_location.trim().trim_matches('"');
    if location.is_empty() {
        return 0;
    }
    let path = Path::new(location);
    if path.is_dir() {
        crate::cleaner::dir_size_limited(path, 3, 200_000)
    } else {
        0
    }
}

/// 已安装应用列表（设计 3.2 的 `cleaner_list_apps`）
pub fn list_apps(emit: &(dyn Fn(CleanerProgress) + Sync)) -> Result<Vec<InstalledApp>, String> {
    emit(CleanerProgress::new(
        "list_apps",
        "scanning",
        0,
        0,
        "正在读取注册表卸载项…",
    ));
    let mut entries = enumerate_entries();
    entries.retain(|(_, entry)| should_show(entry));
    let entries = dedupe(entries);
    let total = entries.len();
    emit(CleanerProgress::new(
        "list_apps",
        "loading_icons",
        0,
        total,
        format!("找到 {} 个应用，正在提取图标…", total),
    ));

    // 图标一次 PowerShell 批量取：几百个应用逐个起 PowerShell 会慢到不可用
    let icon_sources: Vec<Option<PathBuf>> = entries
        .iter()
        .map(|(_, entry)| display_icon_path(&entry.display_icon))
        .collect();
    let unique: Vec<PathBuf> = icon_sources
        .iter()
        .flatten()
        .cloned()
        .collect::<HashSet<PathBuf>>()
        .into_iter()
        .collect();
    let refs: Vec<&Path> = unique.iter().map(|path| path.as_path()).collect();
    let icon_map = crate::commands::openwith::windows_icons(&refs);

    emit(CleanerProgress::new(
        "list_apps",
        "sizing",
        0,
        total,
        "正在统计安装大小…".to_string(),
    ));
    let sizes = crate::cleaner::parallel_map(&entries, 8, |index, (_, entry)| {
        if index % 10 == 0 || index + 1 == total {
            let name = entry.display_name.trim();
            emit(CleanerProgress::new(
                "list_apps",
                "sizing",
                index + 1,
                total,
                format!("正在统计大小：{}", name),
            ));
        }
        size_of_entry(entry)
    });

    let mut apps: Vec<InstalledApp> = entries
        .into_iter()
        .enumerate()
        .map(|(index, (hive, entry))| {
            let icon = icon_sources
                .get(index)
                .and_then(|path| path.as_ref())
                .and_then(|path| icon_map.get(path))
                .cloned();
            InstalledApp {
                id: format!("win:{}|{}", hive.token(), entry.key_path),
                name: entry.display_name.trim().to_string(),
                version: entry.display_version.trim().to_string(),
                publisher: entry.publisher.trim().to_string(),
                install_location: entry.install_location.trim().trim_matches('"').to_string(),
                size_bytes: sizes.get(index).copied().unwrap_or(0),
                icon,
                source: "registry".to_string(),
                can_uninstall: true,
                uninstall_kind: uninstall_kind(&entry).to_string(),
                install_date: format_install_date(&entry.install_date),
                detail: format!(r"注册表键: {}", entry.key_path),
            }
        })
        .collect();
    apps.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    // 不能卸载自己：当前进程 exe 位于某条目的安装目录内 → 从列表里剔除
    let current_exe = std::env::current_exe().unwrap_or_default();
    apps.retain(|app| !is_self_install(&current_exe, &app.install_location));
    emit(CleanerProgress::new(
        "list_apps",
        "done",
        total,
        total,
        format!("已加载 {} 个应用", apps.len()),
    ));
    Ok(apps)
}

/// 运行检测用的安装根候选：注册表 `InstallLocation` + `DisplayIcon` 所在目录。
///
/// 只返回「原始候选」，是否可用（不是 Program Files / AppData 这类公共父目录）
/// 由 `cleaner::running` 过滤；卸载条目被删掉后返回空列表，调用方按未运行处理。
pub fn install_root_candidates(app_id: &str) -> Vec<std::path::PathBuf> {
    let Ok((hive, key_path)) = parse_app_id(app_id) else {
        return Vec::new();
    };
    let Ok(key) = open_entry_key(hive, key_path) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    let location = read_string(&key, "InstallLocation");
    let location = location.trim().trim_matches('"');
    if !location.is_empty() {
        roots.push(std::path::PathBuf::from(location));
    }
    if let Some(icon) = display_icon_path(&read_string(&key, "DisplayIcon")) {
        if let Some(parent) = icon.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    roots
}

// ============================== 关联文件的目录判定 ==============================

/// 系统目录和公共父目录本身不能当作应用安装目录。
///
/// 典型反例：MSI 的 `UninstallString` 是 `System32\msiexec.exe`，旧逻辑会把
/// `C:\Windows\System32` 整个列成「程序文件」并默认勾选；用户一眼看到 6 GB
/// 的系统目录，点删除还会被白名单拒绝。整个 `%WINDIR%` 一律拒绝，公共目录
/// 只拒绝根目录本身（子目录如 `Program Files\Foo` 仍允许）。
pub(super) fn is_forbidden_install_dir(path: &Path) -> bool {
    let Some(normalized) = crate::cleaner::running::normalize_win_path(&path.to_string_lossy())
    else {
        return true;
    };
    if normalized.trim_end_matches('\\').len() <= 2 {
        return true;
    }
    let windows_roots = ["WINDIR", "SystemRoot"]
        .iter()
        .filter_map(|name| std::env::var_os(name))
        .filter_map(|value| crate::cleaner::running::normalize_win_path(&value.to_string_lossy()))
        .collect::<Vec<_>>();
    if windows_roots
        .iter()
        .any(|root| crate::cleaner::running::win_path_within(&normalized, root))
    {
        return true;
    }
    [
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "ProgramData",
        "APPDATA",
        "LOCALAPPDATA",
        "USERPROFILE",
    ]
    .iter()
    .filter_map(|name| std::env::var_os(name))
    .filter_map(|value| crate::cleaner::running::normalize_win_path(&value.to_string_lossy()))
    .any(|root| root == normalized)
}

#[cfg(test)]
mod dir_tests {
    use super::*;

    #[test]
    fn rejects_windows_and_public_roots() {
        let windir = std::env::var_os("WINDIR").expect("WINDIR");
        assert!(is_forbidden_install_dir(Path::new(&windir)));
        assert!(is_forbidden_install_dir(
            &PathBuf::from(&windir).join("System32")
        ));
        assert!(is_forbidden_install_dir(Path::new(r"C:\")));
        assert!(!is_forbidden_install_dir(Path::new(
            r"C:\Program Files\Foo"
        )));
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            assert!(is_forbidden_install_dir(Path::new(&program_files)));
        }
    }
}
