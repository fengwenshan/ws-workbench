//! Windows 侧安装列表、卸载执行与残留规则。
//!
//! 数据源是注册表三处 Uninstall 键（设计 4.1）：HKLM 64 位视图、HKLM 的
//! Wow6432Node（32 位视图）、HKCU。用 `KEY_WOW64_32KEY/64KEY` 显式指定视图，
//! 否则 32 位进程看到的是重定向后的视图，会漏掉一半条目。
//!
//! 本文件分两层：
//! - **与 winreg 无关的纯逻辑**（过滤 / 去重 / 大小换算 / UninstallString 解析）
//!   不依赖 Windows，`cargo test` 在 macOS 上也能跑；
//! - 真正读注册表、起卸载程序的部分用 `#[cfg(target_os = "windows")]` 隔离。

#[cfg(target_os = "windows")]
use super::models::{CleanerProgress, InstalledApp, LeftoverItem, UninstallOutcome};

/// 注册表根（枚举顺序即去重优先级：HKLM 64 位 > Wow6432Node > HKCU）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hive {
    Hklm,
    HklmWow64,
    Hkcu,
}

impl Default for Hive {
    fn default() -> Self {
        Self::Hklm
    }
}

impl Hive {
    /// app id / 残留路径里使用的令牌
    pub fn token(self) -> &'static str {
        match self {
            Hive::Hklm => crate::cleaner::trash_guard::HIVE_HKLM,
            Hive::HklmWow64 => crate::cleaner::trash_guard::HIVE_HKLM_WOW64,
            Hive::Hkcu => crate::cleaner::trash_guard::HIVE_HKCU,
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            value if value == crate::cleaner::trash_guard::HIVE_HKLM => Some(Hive::Hklm),
            value if value == crate::cleaner::trash_guard::HIVE_HKLM_WOW64 => Some(Hive::HklmWow64),
            value if value == crate::cleaner::trash_guard::HIVE_HKCU => Some(Hive::Hkcu),
            _ => None,
        }
    }
}

/// 注册表里的一条 Uninstall 记录（私有，只在 Windows 枚举与纯逻辑之间传递）
#[derive(Debug, Clone, Default)]
pub struct UninstallEntry {
    pub display_name: String,
    pub display_version: String,
    pub publisher: String,
    pub install_location: String,
    pub display_icon: String,
    pub uninstall_string: String,
    pub quiet_uninstall_string: String,
    pub estimated_size: Option<String>,
    pub install_date: String,
    pub system_component: Option<String>,
    pub parent_key_name: String,
    pub release_type: String,
    pub key_path: String,
}

// ============================== 纯逻辑（可跨平台单测） ==============================

/// 是否把这条记录展示给用户（设计 4.1 的过滤规则）。
///
/// 只排除系统组件、补丁、更新、KB 条目：这些在「应用和功能」里也不该出现，
/// 列出来会让用户误以为可以卸载系统更新。
pub fn should_show(entry: &UninstallEntry) -> bool {
    let name = entry.display_name.trim();
    if name.is_empty() {
        return false;
    }
    let system_component = entry
        .system_component
        .as_deref()
        .map(str::trim)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if system_component {
        return false;
    }
    if !entry.parent_key_name.trim().is_empty() {
        return false;
    }
    let release = entry.release_type.to_ascii_lowercase();
    if release.contains("update") || release.contains("hotfix") {
        return false;
    }
    if is_kb_display_name(name) {
        return false;
    }
    true
}

/// DisplayName 形如 `KB5034441`（或 `KB5034441 x64`）的系统补丁
fn is_kb_display_name(name: &str) -> bool {
    let upper = name.trim().to_ascii_uppercase();
    let Some(rest) = upper.strip_prefix("KB") else {
        return false;
    };
    let digits: String = rest.chars().take_while(|char| char.is_ascii_digit()).collect();
    if digits.is_empty() {
        return false;
    }
    // 后面的部分只允许空白/括号/语言描述，不能再是纯字母单词（避免误杀 "KBox"）
    !rest[digits.len()..]
        .chars()
        .any(|char| char.is_ascii_alphabetic() && char.is_ascii_uppercase())
}

/// 同名同版本只留一条，HKLM 优先（设计 4.1）。
pub fn dedupe(entries: Vec<(Hive, UninstallEntry)>) -> Vec<(Hive, UninstallEntry)> {
    use std::collections::HashMap;

    let mut kept: Vec<(Hive, UninstallEntry)> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for (hive, entry) in entries {
        let key = (
            entry.display_name.trim().to_lowercase(),
            entry.display_version.trim().to_lowercase(),
        );
        match index.get(&key).copied() {
            Some(position) => {
                if hive < kept[position].0 {
                    kept[position] = (hive, entry);
                }
            }
            None => {
                index.insert(key, kept.len());
                kept.push((hive, entry));
            }
        }
    }
    kept
}

/// `EstimatedSize` 单位是 KB，换算成字节（设计 4.1）。解析失败按 0（未知）。
pub fn parse_estimated_size(raw: Option<&str>) -> u64 {
    raw.map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| text.parse::<u64>().ok())
        .map(|kilobytes| kilobytes.saturating_mul(1024))
        .unwrap_or(0)
}

/// 注册表 `InstallDate` 是 `YYYYMMDD`，转成 UI 用的 `YYYY-MM-DD`；非法返回空串。
pub fn format_install_date(raw: &str) -> String {
    let digits: String = raw.trim().chars().filter(|char| char.is_ascii_digit()).collect();
    if digits.len() != 8 {
        return String::new();
    }
    let (year, rest) = digits.split_at(4);
    let (month, day) = rest.split_at(2);
    let month_number: u32 = month.parse().unwrap_or(0);
    let day_number: u32 = day.parse().unwrap_or(0);
    if !(1..=12).contains(&month_number) || !(1..=31).contains(&day_number) {
        return String::new();
    }
    format!("{}-{}-{}", year, month, day)
}

/// 卸载类型（设计 3.1 的 uninstall_kind）
pub fn uninstall_kind(entry: &UninstallEntry) -> &'static str {
    let raw = uninstall_string_of(entry);
    if raw.trim().is_empty() {
        return "force";
    }
    match parse_uninstall_command(raw, false) {
        Some(command) if is_msiexec(&command.program) => "msi",
        _ => "exe",
    }
}

/// 优先用 `UninstallString`；它缺失时退到 `QuietUninstallString`。
fn uninstall_string_of(entry: &UninstallEntry) -> &str {
    if !entry.uninstall_string.trim().is_empty() {
        &entry.uninstall_string
    } else {
        &entry.quiet_uninstall_string
    }
}

fn is_msiexec(program: &str) -> bool {
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program);
    let name = name.to_ascii_lowercase();
    name == "msiexec" || name == "msiexec.exe"
}

/// 一条可执行的卸载命令
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallCommand {
    pub program: String,
    pub args: Vec<String>,
    pub needs_elevation: bool,
}

impl UninstallCommand {
    /// 给用户看的命令行（引号只在有空格时补）
    pub fn display(&self) -> String {
        let quote = |value: &str| {
            if value.contains(' ') {
                format!("\"{}\"", value)
            } else {
                value.to_string()
            }
        };
        let mut parts = vec![quote(&self.program)];
        parts.extend(self.args.iter().map(|arg| quote(arg)));
        parts.join(" ")
    }
}

/// 解析 `UninstallString`（含 msiexec `/I` → `/X` 转换）。
///
/// 返回 None 表示没有可执行的命令（空串）；`rundll32` 等保持原样，交给
/// 系统卸载器自己处理。
pub fn parse_uninstall_command(raw: &str, needs_elevation: bool) -> Option<UninstallCommand> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.trim().is_empty() {
        return None;
    }
    let tokens = split_windows_args(raw.trim());
    let mut iter = tokens.into_iter();
    let program = iter.next().filter(|program| !program.is_empty())?;
    let mut args: Vec<String> = iter.collect();

    // MsiExec.exe /I{GUID} 是「安装/修复」语义，卸载要转成 /X{GUID}
    if is_msiexec(&program) {
        for arg in args.iter_mut() {
            if arg.eq_ignore_ascii_case("/i") {
                *arg = "/X".to_string();
            } else if arg.len() > 2 && arg[..2].eq_ignore_ascii_case("/i") {
                let rest = arg[2..].to_string();
                *arg = format!("/X{}", rest);
            }
        }
    }

    Some(UninstallCommand {
        program,
        args,
        needs_elevation,
    })
}

/// 按 `CommandLineToArgvW` 的规则拆分 Windows 命令行。
///
/// 规则要点（和 POSIX shell 不同）：
/// - 引号成对出现只用来分组，本身不进参数；
/// - 反斜杠只有紧邻引号时才有转义含义（`2n` 个反斜杠 + 引号 → `n` 个反斜杠 + 引号）；
/// - 引号对之外的空白才是分隔符。
///
/// 不用「按空格 split 再去引号」的偷懒写法：`"C:\Program Files\Foo\uninst.exe" /S`
/// 里带空格的路径会被拆成两个参数，卸载程序直接启动失败。
pub fn split_windows_args(line: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    let mut chars = line.chars().peekable();

    while let Some(char) = chars.next() {
        match char {
            '\\' => {
                let mut backslashes = 1;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    backslashes += 1;
                }
                if chars.peek() == Some(&'"') {
                    // 2n 个反斜杠 + 引号 → n 个反斜杠 + 普通引号
                    for _ in 0..backslashes / 2 {
                        current.push('\\');
                    }
                    if backslashes % 2 == 1 {
                        current.push('"');
                    }
                    chars.next();
                } else {
                    for _ in 0..backslashes {
                        current.push('\\');
                    }
                }
                has_token = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            char if char.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            char => {
                current.push(char);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, sys: Option<&str>, parent: &str, release: &str) -> UninstallEntry {
        UninstallEntry {
            display_name: name.into(),
            system_component: sys.map(|value| value.into()),
            parent_key_name: parent.into(),
            release_type: release.into(),
            ..Default::default()
        }
    }

    #[test]
    fn filter_skips_system_and_updates() {
        assert!(!should_show(&entry("KB5034441", None, "", "")));
        assert!(!should_show(&entry("Google Chrome", Some("1"), "", "")));
        assert!(!should_show(&entry("Update for Office", None, "Office", "")));
        assert!(!should_show(&entry("Security Update", None, "", "Security Update")));
        assert!(!should_show(&entry("", None, "", "")));
        assert!(should_show(&entry("Google Chrome", None, "", "")));
        assert!(should_show(&entry("KBox App", None, "", "")));
    }

    #[test]
    fn dedupe_prefers_hklm() {
        // 同名同版本：HKLM 在前、HKCU 在后，去重后保留 HKLM
        let kept = dedupe(vec![
            (Hive::Hklm, entry("Foo", None, "", "")),
            (Hive::Hkcu, entry("Foo", None, "", "")),
        ]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, Hive::Hklm);

        // 即使 HKCU 先出现，HKLM 也应当替换它
        let kept = dedupe(vec![
            (Hive::Hkcu, entry("Foo", None, "", "")),
            (Hive::HklmWow64, entry("Foo", None, "", "")),
        ]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, Hive::HklmWow64);
    }

    #[test]
    fn estimated_size_is_kilobytes() {
        assert_eq!(parse_estimated_size(Some("1024")), 1024 * 1024);
        assert_eq!(parse_estimated_size(None), 0);
        assert_eq!(parse_estimated_size(Some("not a number")), 0);
    }

    #[test]
    fn converts_msiexec_i_to_x() {
        let cmd = parse_uninstall_command(
            r#"MsiExec.exe /I{90160000-008C-0000-1000-0000000FF1CE}"#,
            false,
        );
        assert!(cmd.unwrap().args.join(" ").contains("/X{90160000"));
    }

    #[test]
    fn splits_quoted_exe_argument() {
        let cmd =
            parse_uninstall_command(r#""C:\Program Files\Foo\uninst.exe" /S /L=0"#, false).unwrap();
        assert_eq!(cmd.program, r#"C:\Program Files\Foo\uninst.exe"#);
        assert_eq!(cmd.args, vec!["/S", "/L=0"]);
    }

    #[test]
    fn empty_command_is_none() {
        assert!(parse_uninstall_command("", false).is_none());
    }

    #[test]
    fn rundll32_kept() {
        let cmd = parse_uninstall_command(r#"rundll32.exe "C:\a.dll",Uninstall"#, false).unwrap();
        assert_eq!(cmd.program, "rundll32.exe");
        // CommandLineToArgvW 规则：引号只分组、不进参数，所以是 `C:\a.dll,Uninstall`
        assert_eq!(cmd.args, vec![r#"C:\a.dll,Uninstall"#]);
    }

    #[test]
    fn split_keeps_spaces_inside_quotes_and_backslash_escapes() {
        assert_eq!(
            split_windows_args(r#""C:\Program Files\a b.exe" /S"#),
            vec![r#"C:\Program Files\a b.exe"#, "/S"]
        );
        // 结尾反斜杠要写成两个才能在引号内保留：`"C:\dir\\"` → `C:\dir\`
        assert_eq!(split_windows_args(r#""C:\dir\\""#), vec![r#"C:\dir\"#]);
        // 单个反斜杠 + 引号是「字面引号」，不结束分组
        assert_eq!(split_windows_args(r#""C:\dir\""#), vec![r#"C:\dir""#]);
        assert_eq!(split_windows_args("  a   b  "), vec!["a", "b"]);
        assert!(split_windows_args("   ").is_empty());
    }

    #[test]
    fn install_date_is_formatted_or_empty() {
        assert_eq!(format_install_date("20240105"), "2024-01-05");
        assert_eq!(format_install_date("2024-01-05"), "2024-01-05");
        assert_eq!(format_install_date(""), "");
        assert_eq!(format_install_date("20241305"), "");
    }

    #[test]
    fn uninstall_kind_classifies_entries() {
        let mut msi = entry("Foo", None, "", "");
        msi.uninstall_string = r#"MsiExec.exe /X{GUID}"#.to_string();
        assert_eq!(uninstall_kind(&msi), "msi");

        let mut exe = entry("Bar", None, "", "");
        exe.uninstall_string = r#""C:\Bar\uninst.exe" /S"#.to_string();
        assert_eq!(uninstall_kind(&exe), "exe");

        // 没有卸载命令 → 强制移除（删注册表条目）
        assert_eq!(uninstall_kind(&entry("Baz", None, "", "")), "force");
    }
}

// ============================== Windows 实现（注册表 / 执行） ==============================

#[cfg(target_os = "windows")]
mod win_impl {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };
    use winreg::RegKey;

    use super::*;

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

    fn read_string(key: &RegKey, name: &str) -> String {
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
    pub fn parse_app_id(app_id: &str) -> Result<(Hive, &str), String> {
        let rest = app_id
            .strip_prefix("win:")
            .ok_or_else(|| format!("无效的应用标识: {}", app_id))?;
        let (hive_token, key_path) = rest
            .split_once('|')
            .ok_or_else(|| format!("无效的应用标识: {}", app_id))?;
        let hive = Hive::from_token(hive_token)
            .ok_or_else(|| format!("未知的注册表根: {}", hive_token))?;
        if key_path.trim().is_empty() {
            return Err(format!("无效的应用标识: {}", app_id));
        }
        Ok((hive, key_path))
    }

    /// 按 hive + 键路径打开注册表项（卸载/残留扫描都要用）
    fn open_entry_key(hive: Hive, key_path: &str) -> Result<RegKey, String> {
        let (root, view) = hive_root(hive);
        root.open_subkey_with_flags(key_path, KEY_READ | view)
            .map_err(|err| format!("打开注册表项失败: {}", err))
    }

    /// `DisplayIcon` 里可能带 `,0` 图标索引和引号，取真正的文件路径
    fn display_icon_path(raw: &str) -> Option<PathBuf> {
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
        apps.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
        });
        emit(CleanerProgress::new(
            "list_apps",
            "done",
            total,
            total,
            format!("已加载 {} 个应用", total),
        ));
        Ok(apps)
    }

    /// 生成 `Start-Process` 提权/普通启动脚本
    fn start_process_script(command: &UninstallCommand) -> String {
        let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
        let args = command
            .args
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "$p = Start-Process -FilePath {} -ArgumentList @({}) -Wait -PassThru{}; \
             if ($null -ne $p) {{ $p.ExitCode }} else {{ 0 }}",
            quote(&command.program),
            args,
            if command.needs_elevation {
                " -Verb RunAs"
            } else {
                ""
            }
        )
    }

    /// 卸载（设计 4.2 的 `cleaner_uninstall_app`）。
    ///
    /// 没有 `UninstallString` 的条目走「强制移除」：删掉注册表条目后由前端继续
    /// 残留清理。HKLM 条目需要一次 UAC，HKCU 条目普通权限即可。
    pub fn uninstall(
        app_id: &str,
        hint: Option<&InstalledApp>,
        emit: &dyn Fn(CleanerProgress),
    ) -> Result<UninstallOutcome, String> {
        let (hive, key_path) = parse_app_id(app_id)?;
        let key = open_entry_key(hive, key_path)?;
        let display_name = {
            let name = read_string(&key, "DisplayName");
            if name.trim().is_empty() {
                hint.map(|app| app.name.clone()).unwrap_or_default()
            } else {
                name
            }
        };
        let entry = UninstallEntry {
            display_name: display_name.clone(),
            uninstall_string: read_string(&key, "UninstallString"),
            quiet_uninstall_string: read_string(&key, "QuietUninstallString"),
            ..Default::default()
        };
        let raw = uninstall_string_of(&entry).to_string();

        if raw.trim().is_empty() {
            // 强制移除：只删注册表条目（设计 4.5 明确不清理 COM/服务/驱动）
            emit(CleanerProgress::new(
                "uninstall",
                "running",
                0,
                1,
                "该应用没有卸载命令，正在移除注册表条目…",
            ));
            crate::cleaner::trash_guard::delete_registry_key(hive.token(), key_path)?;
            emit(CleanerProgress::new("uninstall", "done", 1, 1, "已移除注册表条目"));
            return Ok(UninstallOutcome {
                ok: true,
                command: format!("删除注册表条目: {}|{}", hive.token(), key_path),
                exit_code: None,
                elevated: false,
                message: "该应用没有卸载命令，已移除注册表条目；请继续清理残留文件".to_string(),
            });
        }

        let needs_elevation = hive != Hive::Hkcu;
        let command = parse_uninstall_command(&raw, needs_elevation)
            .ok_or_else(|| "无法解析卸载命令".to_string())?;
        emit(CleanerProgress::new(
            "uninstall",
            "running",
            0,
            1,
            format!(
                "正在运行卸载程序{}{}…",
                command.display(),
                if needs_elevation { "（需要管理员授权）" } else { "" }
            ),
        ));

        let script = start_process_script(&command);
        // winps 把「进程不能正常退出」直接映射成 Err（字符串里带 stderr），
        // 这里用 ok 标记区分「PowerShell 自己失败」和「脚本跑完但卸载器非 0」
        let (text, ok) = match crate::winps::run_with_timeout_ms(
            &script,
            std::time::Duration::from_secs(15 * 60),
        ) {
            Ok(text) => (text, true),
            Err(error) => (error, false),
        };
        emit(CleanerProgress::new("uninstall", "done", 1, 1, "卸载程序已结束"));

        let command_text = command.display();
        if !ok {
            let lowered = text.to_ascii_lowercase();
            let canceled = lowered.contains("cancel") || text.contains("已取消") || text.contains("1223");
            let message = if text.contains("超时") {
                "卸载程序超时（15 分钟）".to_string()
            } else if canceled {
                "已取消授权，未执行卸载".to_string()
            } else {
                format!("启动卸载程序失败: {}", text.trim())
            };
            return Ok(UninstallOutcome {
                ok: false,
                command: command_text,
                exit_code: None,
                elevated: needs_elevation,
                message,
            });
        }

        let exit_code = text
            .lines()
            .rev()
            .find_map(|line| line.trim().parse::<i32>().ok());
        let (ok, message) = match exit_code {
            Some(0) => (true, "卸载程序已完成".to_string()),
            Some(code) => (
                false,
                format!("卸载程序退出码 {}，可能未完成卸载", code),
            ),
            None => (
                true,
                "卸载程序已启动并结束（未取到退出码）".to_string(),
            ),
        };
        Ok(UninstallOutcome {
            ok,
            command: command_text,
            exit_code,
            elevated: needs_elevation,
            message,
        })
    }

    /// 一个文件/目录残留（存在才算）
    fn file_leftover(
        path: &Path,
        kind: &str,
        confidence: &str,
        selected: bool,
        requires_elevation: bool,
    ) -> Option<LeftoverItem> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        let is_dir = meta.is_dir();
        let size_bytes = if is_dir {
            crate::cleaner::dir_size_limited(path, 4, 200_000)
        } else {
            meta.len()
        };
        Some(LeftoverItem {
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            size_bytes,
            is_dir,
            confidence: confidence.to_string(),
            selected,
            requires_elevation,
            exists: true,
        })
    }

    /// 注册表残留：`SOFTWARE\<Publisher>\<Name>`（HKLM 64/32 视图 + HKCU）
    fn registry_candidates(publisher: &str, name: &str) -> Vec<(Hive, String)> {
        let publisher = publisher.trim();
        let name = name.trim();
        if publisher.is_empty() || name.is_empty() {
            return Vec::new();
        }
        let base_path = format!(r"SOFTWARE\{}\{}", publisher, name);
        let mut out: Vec<(Hive, String)> = Vec::new();
        let mut seen: HashSet<(Hive, String)> = HashSet::new();
        for hive in [Hive::Hklm, Hive::HklmWow64, Hive::Hkcu] {
            if !seen.insert((hive, base_path.clone())) {
                continue;
            }
            if open_entry_key(hive, &base_path).is_ok() {
                out.push((hive, base_path.clone()));
            }
        }
        out
    }

    /// `%APPDATA%` / `%LOCALAPPDATA%` / `%PROGRAMDATA%` 下的同名目录
    fn appdata_candidates(name: &str, publisher: &str) -> Vec<LeftoverItem> {
        let name = name.trim();
        if name.is_empty() {
            return Vec::new();
        }
        let publisher = publisher.trim();
        let mut items: Vec<LeftoverItem> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for var in ["APPDATA", "LOCALAPPDATA", "PROGRAMDATA"] {
            let Some(root) = std::env::var_os(var).map(PathBuf::from) else {
                continue;
            };
            let mut push = |path: PathBuf,
                            kind: &str,
                            confidence: &str,
                            selected: bool,
                            items: &mut Vec<LeftoverItem>| {
                if !seen.insert(path.clone()) {
                    return;
                }
                if let Some(item) = file_leftover(&path, kind, confidence, selected, false) {
                    items.push(item);
                }
            };
            push(
                root.join(name),
                "app_data",
                "medium",
                false,
                &mut items,
            );
            if !publisher.is_empty() {
                push(
                    root.join(publisher).join(name),
                    "app_data",
                    "medium",
                    false,
                    &mut items,
                );
            }
        }
        // %LOCALAPPDATA%\Programs\<Name> 是 per-user 安装位置，属于 high
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            if let Some(item) = file_leftover(
                &local.join("Programs").join(name),
                "install_dir",
                "high",
                true,
                false,
            ) {
                items.push(item);
            }
        }
        items
    }

    /// 开始菜单 / 桌面里与应用同名的快捷方式
    fn shortcut_candidates(name: &str) -> Vec<LeftoverItem> {
        let name = name.trim();
        if name.is_empty() {
            return Vec::new();
        }
        let mut roots: Vec<PathBuf> = Vec::new();
        for var in ["APPDATA", "PROGRAMDATA"] {
            if let Some(root) = std::env::var_os(var).map(PathBuf::from) {
                roots.push(root.join(r"Microsoft\Windows\Start Menu\Programs"));
            }
        }
        if let Some(home) = crate::cleaner::home_dir() {
            roots.push(home.join("Desktop"));
        }
        if let Some(public) = std::env::var_os("PUBLIC").map(PathBuf::from) {
            roots.push(public.join("Desktop"));
        }

        let mut items: Vec<LeftoverItem> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for root in roots {
            let walker = walkdir::WalkDir::new(&root)
                .follow_links(false)
                .max_depth(3)
                .into_iter()
                .filter_map(|entry| entry.ok());
            for entry in walker {
                let path = entry.path();
                let is_lnk = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"));
                if !is_lnk {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !stem.eq_ignore_ascii_case(name) {
                    continue;
                }
                if !seen.insert(path.to_path_buf()) {
                    continue;
                }
                if let Some(item) = file_leftover(path, "shortcut", "high", true, false) {
                    items.push(item);
                }
            }
        }
        items
    }

    /// 残留扫描（设计 4.3 的 Windows 规则）
    pub fn scan_leftovers(
        app_id: &str,
        hint: Option<&InstalledApp>,
        emit: &dyn Fn(CleanerProgress),
    ) -> Result<Vec<LeftoverItem>, String> {
        let (hive, key_path) = parse_app_id(app_id)?;
        // 卸载完成后卸载器往往把 Uninstall 键一起删了；此时靠列表缓存里的
        // 名字/发布者/安装目录继续扫残留，而不是直接报「打开注册表项失败」
        let (name, publisher, install_location) = match open_entry_key(hive, key_path) {
            Ok(key) => {
                let mut name = read_string(&key, "DisplayName");
                if name.trim().is_empty() {
                    name = hint.map(|app| app.name.clone()).unwrap_or_default();
                }
                (
                    name,
                    read_string(&key, "Publisher"),
                    read_string(&key, "InstallLocation"),
                )
            }
            Err(error) => match hint {
                Some(app) => (
                    app.name.clone(),
                    app.publisher.clone(),
                    app.install_location.clone(),
                ),
                None => return Err(error),
            },
        };

        emit(CleanerProgress::new(
            "scan_leftovers",
            "scanning",
            0,
            0,
            format!("正在扫描 {} 的残留…", name),
        ));
        let mut items: Vec<LeftoverItem> = Vec::new();

        // 1) 安装目录（high）
        let location = install_location.trim().trim_matches('"');
        if !location.is_empty() {
            if let Some(item) = file_leftover(Path::new(location), "install_dir", "high", true, false)
            {
                items.push(item);
            }
        }

        // 2) 注册表残留（只列出，删除需要用户显式勾选并提权）
        for (residual_hive, residual_path) in registry_candidates(&publisher, &name) {
            let requires_elevation = residual_hive != Hive::Hkcu;
            items.push(LeftoverItem {
                path: format!("win:{}|{}", residual_hive.token(), residual_path),
                kind: "registry".to_string(),
                size_bytes: 0,
                is_dir: false,
                confidence: "medium".to_string(),
                selected: false,
                requires_elevation,
                exists: true,
            });
        }

        // 3) AppData / ProgramData 同名目录
        items.extend(appdata_candidates(&name, &publisher));

        // 4) 开始菜单 / 桌面快捷方式
        items.extend(shortcut_candidates(&name));

        emit(CleanerProgress::new(
            "scan_leftovers",
            "done",
            items.len(),
            items.len(),
            format!("找到 {} 条残留", items.len()),
        ));
        Ok(items)
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
}

#[cfg(target_os = "windows")]
pub use win_impl::{install_root_candidates, list_apps, scan_leftovers, uninstall};
