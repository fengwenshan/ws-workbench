//! VPN 连接器平台层：跨平台共享的部分
//!
//! 这里只放「macOS 与 Windows 行为完全一致」的东西：
//! - [`probe_openconnect`]：真正执行一次 `--version`，判断二进制是不是真能跑；
//! - 版本号解析（含中文本地化输出）；
//! - 来源展示名、启动失败描述；
//! - [`pick_connector`] / [`fetch_version`] 与 [`ImportedOpenconnect`] 模型。
//!
//! 检测顺序、导入方式、候选来源都属于平台差异，放在 `macos.rs` / `windows.rs`。
//! 本文件不写任何 `#[cfg(target_os = ...)]` 分支。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::vpn::models::{ConnectorInfo, ConnectorKind};

/// 探测 openconnect 时的超时。正常几十毫秒，给足余量避免慢机器误判。
pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// 从 `openconnect --version` 输出里取版本号
///
/// 不能只认英文前缀：openconnect 9.x 会按系统语言翻译输出，中文机器上第一行是
/// 「OpenConnect 版本 v9.21」。从 Finder 启动的 .app 没有 LANG/LC_ALL，gettext
/// 退回系统语言，于是同一份程序在终端里（LANG=C.UTF-8）能识别、打包后却解析不到
/// 版本号，界面就误报「未找到 openconnect」。这里两条路都认：
/// 1. 英文：`OpenConnect version v9.12`
/// 2. 本地化：`OpenConnect <任意语言的 version 字样> v9.21` —— 产品名不会被翻译，
///    从该行里取第一个版本号 token（`v?数字(.数字)+`）
pub(super) fn parse_openconnect_version(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("OpenConnect version ") {
            return rest.trim().to_string();
        }
        if line.starts_with("OpenConnect") {
            if let Some(version) = line.split_whitespace().find_map(leading_version) {
                return version;
            }
        }
    }
    String::new()
}

/// 从 token 里取开头的版本号：`v9.21` / `9.12` / `9.12.1`。
///
/// 本地化行里版本号后面可能紧跟全角标点或其它文字（`v9.21（GnuTLS…`），
/// 所以只认前缀，不要求整个 token 都是版本号；至少有 `数字.数字` 才算，
/// 免得把 `PKCS#11` 这类数字当成版本。
fn leading_version(token: &str) -> Option<String> {
    let trimmed = token.trim_start_matches(|c: char| {
        !c.is_ascii_alphanumeric() && c != '.' && c != 'v' && c != 'V'
    });
    let (prefix, digits) = if let Some(rest) = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
    {
        ("v", rest)
    } else {
        ("", trimmed)
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(digits.len());
    let candidate = digits[..end].trim_end_matches('.');
    if candidate.is_empty() || !candidate.contains('.') {
        return None;
    }
    if candidate
        .split('.')
        .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(format!("{}{}", prefix, candidate))
}

/// 候选来源到展示名的后缀
fn source_suffix(source: &str) -> &'static str {
    match source {
        "custom" => "（手动指定）",
        "managed" => "（已指定）",
        "bundled" => "（内置）",
        _ => "",
    }
}

/// 探测/登记失败时给出准确的病因，别再一律说成「缺依赖 / 没执行权限」。
///
/// 现场踩过：openconnect 明明能跑，只是 `--version` 输出被系统语言翻译、
/// 旧解析没认出中文版本行，界面却报「这份 openconnect 跑不起来（缺少依赖库或
/// 没有执行权限）」，把排查方向完全带偏。这里把「跑不起来」和「能跑但认不出
/// 版本号」分开，后者要把原始输出带出来。
pub(super) fn describe_launch_failure(text: &str, ok: bool) -> String {
    let output: String = text.trim().chars().take(200).collect();
    if !ok {
        format!("跑不起来（缺少依赖库或没有执行权限）: {}", output)
    } else if output.is_empty() {
        "能执行，但 --version 没有任何输出，无法确认版本号".to_string()
    } else {
        format!("能执行，但没从 --version 输出里识别出版本号: {}", output)
    }
}

/// 启动并按版本号判断一个 openconnect 是否真的能用
pub(super) fn probe_openconnect(path: &Path, source: &str) -> ConnectorInfo {
    let (text, ok) = crate::process::run_with_timeout(path, &["--version"], PROBE_TIMEOUT);
    let version = parse_openconnect_version(&text);
    let available = ok && !version.is_empty();
    let note = if available {
        String::new()
    } else if text.contains("执行超时") {
        "执行超时，可能是缺少依赖 DLL".to_string()
    } else {
        describe_launch_failure(&text, ok)
    };
    ConnectorInfo {
        kind: "openconnect".to_string(),
        name: format!("OpenConnect{}", source_suffix(source)),
        path: path.to_string_lossy().to_string(),
        version,
        available,
        source: source.to_string(),
        note,
    }
}

/// 按用户指定的连接器偏好，从探测结果里挑一个可用的
///
/// 现在只剩 openconnect 一条通道，`auto` 与 `openconnect` 的挑选结果完全一致；
/// 保留 `want` 是为了让枚举匹配穷尽，将来再加通道时编译器会强制这里处理。
pub fn pick_connector(candidates: &[ConnectorInfo], want: ConnectorKind) -> Option<&ConnectorInfo> {
    match want {
        ConnectorKind::Auto | ConnectorKind::Openconnect => candidates
            .iter()
            .find(|item| item.kind == "openconnect" && item.available && !item.path.is_empty()),
    }
}

/// 懒加载连接器版本号
pub fn fetch_version(info: &ConnectorInfo) -> String {
    if !info.version.is_empty() || !info.available || info.path.is_empty() {
        return info.version.clone();
    }
    let path = PathBuf::from(&info.path);
    let (text, _) = crate::process::run_with_timeout(&path, &["--version"], PROBE_TIMEOUT);
    parse_openconnect_version(&text)
}

/// 登记结果：版本号 + 实际记住的 openconnect 位置。
///
/// 位置要回显到界面上 —— 用户专门强调过「hb 是自定义目录，名字可能是别的」，
/// 只回一句「已记住」等于没说，得让他看见程序到底认的是哪个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedOpenconnect {
    pub version: String,
    /// 真正被记住的可执行文件（Cellar 版本目录会折算成稳定的 `prefix/bin`）
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::models::ConnectorInfo;

    fn info(kind: &str, available: bool, path: &str) -> ConnectorInfo {
        ConnectorInfo {
            kind: kind.to_string(),
            name: kind.to_string(),
            path: path.to_string(),
            version: String::new(),
            available,
            source: "system".to_string(),
            note: String::new(),
        }
    }

    #[test]
    fn parse_openconnect_version_line() {
        let text = "OpenConnect version v9.12\nUsing GnuTLS 3.8.3\n";
        assert_eq!(parse_openconnect_version(text), "v9.12");
    }

    /// 现场原文：中文系统上 openconnect 把 `version` 翻译成「版本」
    /// （macOS 从 Finder 启动的 .app 没有 LANG，gettext 退回系统语言）
    #[test]
    fn parse_openconnect_version_localized_chinese() {
        let text = "OpenConnect 版本 v9.21\n\
                    正使用 GnuTLS 3.8.13。当前功能： PKCS#11, RSA software token, HOTP software token\n\
                    支持的协议： anyconnect（默认）, nc, gp, pulse, f5, fortinet, array\n";
        assert_eq!(parse_openconnect_version(text), "v9.21");
    }

    /// 其它语言同理：产品名不变，版本号前缀解析，不能依赖英文的 version 单词
    #[test]
    fn parse_openconnect_version_localized_other_languages() {
        assert_eq!(
            parse_openconnect_version("OpenConnect версия v9.21\nUsing GnuTLS\n"),
            "v9.21"
        );
        assert_eq!(
            parse_openconnect_version("OpenConnect Version 9.12\n"),
            "9.12"
        );
        // 版本号后面紧跟全角标点/说明文字也要能取出来
        assert_eq!(
            parse_openconnect_version("OpenConnect 版本 v9.21（GnuTLS 3.8.13）\n"),
            "v9.21"
        );
    }

    /// 行里没有真正版本号时不能瞎认（PKCS#11 这类数字不是版本）
    #[test]
    fn parse_openconnect_version_rejects_non_version_numbers() {
        assert_eq!(parse_openconnect_version("OpenConnect PKCS#11\n"), "");
        assert_eq!(parse_openconnect_version("whatever"), "");
    }

    #[test]
    fn source_suffix_labels_are_distinct() {
        assert_eq!(source_suffix("custom"), "（手动指定）");
        assert_eq!(source_suffix("managed"), "（已指定）");
        assert_eq!(source_suffix("bundled"), "（内置）");
        assert_eq!(source_suffix("system"), "");
        assert_eq!(source_suffix("path"), "");
    }

    #[test]
    fn picks_openconnect_for_both_preferences() {
        let list = vec![info("openconnect", true, "x")];
        assert_eq!(
            pick_connector(&list, ConnectorKind::Auto).unwrap().kind,
            "openconnect"
        );
        assert_eq!(
            pick_connector(&list, ConnectorKind::Openconnect)
                .unwrap()
                .kind,
            "openconnect"
        );
    }

    #[test]
    fn returns_none_when_nothing_usable() {
        let list = vec![info("openconnect", false, "")];
        assert!(pick_connector(&list, ConnectorKind::Auto).is_none());
        assert!(pick_connector(&list, ConnectorKind::Openconnect).is_none());
    }

    #[test]
    fn unavailable_candidate_with_path_is_not_picked() {
        // 探测失败但路径还在（例如缺 DLL），不能被当成可用
        let list = vec![info("openconnect", false, r"C:\app\openconnect.exe")];
        assert!(pick_connector(&list, ConnectorKind::Auto).is_none());
        // 可用但路径为空（missing 占位）同样不能选
        let list = vec![info("openconnect", true, "")];
        assert!(pick_connector(&list, ConnectorKind::Auto).is_none());
    }
}
