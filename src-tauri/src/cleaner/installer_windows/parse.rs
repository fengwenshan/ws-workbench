//! Windows 注册表卸载条目的纯逻辑（不碰 winreg）。
//!
//! 过滤 / 去重 / 大小换算 / `UninstallString` 解析 / 自身安装目录判定都在这里，
//! 不依赖 Windows API；`cleaner/mod.rs` 用 `#[cfg(any(target_os = "windows", test))]`
//! 编进 macOS 的测试构建，所以这些单测在 macOS 上也能跑。
//!
//! 真正读写注册表、启动卸载程序的代码在 `registry.rs` / `uninstall.rs` /
//! `leftovers.rs` / `related.rs`，它们只在 `target_os = "windows"` 下编译。

use crate::cleaner::registry_id::{HIVE_HKCU, HIVE_HKLM, HIVE_HKLM_WOW64};

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
            Hive::Hklm => HIVE_HKLM,
            Hive::HklmWow64 => HIVE_HKLM_WOW64,
            Hive::Hkcu => HIVE_HKCU,
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            value if value == HIVE_HKLM => Some(Hive::Hklm),
            value if value == HIVE_HKLM_WOW64 => Some(Hive::HklmWow64),
            value if value == HIVE_HKCU => Some(Hive::Hkcu),
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
    let digits: String = rest
        .chars()
        .take_while(|char| char.is_ascii_digit())
        .collect();
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
    let digits: String = raw
        .trim()
        .chars()
        .filter(|char| char.is_ascii_digit())
        .collect();
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
///
/// `uninstall.rs` 判断「没有卸载命令 → 强制移除」时也走这里，保持一致。
pub(super) fn uninstall_string_of(entry: &UninstallEntry) -> &str {
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

/// 当前进程是否就装在这个安装目录里（Windows 上的「不能卸载自己」判定）。
///
/// 两边都按 Windows 规则归一化（小写、反斜杠、去引号/尾随分隔符）后做目录边界
/// 前缀比较，所以 `C:\Program Files\Foo` 不会把 `C:\Program Files\Foobar` 算成
/// 自己，大小写差异也不影响。空/盘符根级的 `install_location` 一律不认。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_self_install(current_exe: &std::path::Path, install_location: &str) -> bool {
    let Some(root) = crate::cleaner::running::normalize_win_path(install_location) else {
        return false;
    };
    // 盘符根（`c:` / `c:\`）或空串：几乎任何进程都在它下面，不能用来判定归属
    if root.len() <= 2 {
        return false;
    }
    let Some(exe) = crate::cleaner::running::normalize_win_path(&current_exe.to_string_lossy())
    else {
        return false;
    };
    crate::cleaner::running::win_path_within(&exe, &root)
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
        assert!(!should_show(&entry(
            "Update for Office",
            None,
            "Office",
            ""
        )));
        assert!(!should_show(&entry(
            "Security Update",
            None,
            "",
            "Security Update"
        )));
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

    // ============================== 自身识别（Windows） ==============================

    #[test]
    fn is_self_install_is_case_insensitive_and_boundary_safe() {
        let exe = std::path::Path::new(r"C:\Program Files\ws-workbench\ws-workbench.exe");
        assert!(is_self_install(exe, r"C:\Program Files\ws-workbench"));
        // 大小写、斜杠、尾随分隔符都归一化
        assert!(is_self_install(exe, "c:/program files/WS-WORKBENCH//"));
        // 同前缀的兄弟目录不是自己
        assert!(!is_self_install(exe, r"C:\Program Files\ws-workbench-dev"));
        // 空安装位置 / 盘符根不能用来判定
        assert!(!is_self_install(exe, ""));
        assert!(!is_self_install(exe, r"C:\"));
        assert!(!is_self_install(exe, "C:"));
        // 完全不相关的安装位置
        assert!(!is_self_install(exe, r"D:\Other"));
    }
}
