//! VPN 连接器探测
//!
//! 可执行文件的**查找**已经统一交给 [`crate::apps`]，本模块只负责
//! 「拿到候选路径之后，怎么判断它能不能用」：
//!
//! - openconnect 要真正执行一次 `--version`，因为 Wintun 版解压后缺 DLL 是很常见的情况，
//!   光看文件存在会把一个跑不起来的二进制当成可用。
//!
//! 优先级：手动指定 > 程序托管副本 > 内置资源 > 注册表 > 常见安装目录 > PATH，
//! 这些顺序由 `apps::locate` 定义，这里不重复实现。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::apps::{self, AppContext, AppId};
use crate::vpn::models::{ConnectorInfo, ConnectorKind};

/// 探测 openconnect 时的超时。正常几十毫秒，给足余量避免慢机器误判。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// 从 `openconnect --version` 输出里取版本号
fn parse_openconnect_version(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("OpenConnect version ") {
            return rest.trim().to_string();
        }
    }
    String::new()
}

/// 候选来源到展示名的后缀
fn source_suffix(source: &str) -> &'static str {
    match source {
        "custom" => "（手动指定）",
        "managed" => "（已导入）",
        "bundled" => "（内置）",
        _ => "",
    }
}

/// 启动并按版本号判断一个 openconnect 是否真的能用
fn probe_openconnect(path: &Path, source: &str) -> ConnectorInfo {
    let (text, ok) = crate::process::run_with_timeout(path, &["--version"], PROBE_TIMEOUT);
    let version = parse_openconnect_version(&text);
    let available = ok && !version.is_empty();
    let note = if available {
        String::new()
    } else if text.contains("执行超时") {
        "执行超时，可能是缺少依赖 DLL".to_string()
    } else {
        format!(
            "无法执行: {}",
            text.trim().chars().take(160).collect::<String>()
        )
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

/// 探测 openconnect 的全部候选
fn probe_app(id: AppId, ctx: &AppContext, override_path: &str) -> Vec<ConnectorInfo> {
    let location = apps::locate(id, ctx, override_path);
    let kind = "openconnect";

    let probe = |path: &Path, source: &str| probe_openconnect(path, source);

    let mut out: Vec<ConnectorInfo> = Vec::new();
    let mut probed: Vec<String> = Vec::new();

    // 按优先级顺序探测真实存在的候选，第一个可用的就停
    for candidate in location.candidates.iter().filter(|item| item.exists) {
        let info = probe(Path::new(&candidate.path), &candidate.source);
        let available = info.available;
        probed.push(candidate.path.clone());
        out.push(info);
        if available {
            break;
        }
    }

    // 内置副本不在 candidates 里 —— `apps::locate` 把它单独放进 builtin_path，
    // 免得界面上看着像还在搜用户自己的安装目录。所以候选全部不可用时必须补探它，
    // 否则内置 openconnect 永远被判成 missing，「自动」模式会报「没有可用连接器」。
    if !out.iter().any(|item| item.available) {
        let builtin = location.builtin_path.trim();
        if !builtin.is_empty() && !probed.iter().any(|path| path == builtin) {
            out.push(probe(Path::new(builtin), "bundled"));
        }
    }

    // 手动指定但文件不在：明确报出来，否则用户不知道设置没生效
    if let Some(custom) = location
        .candidates
        .iter()
        .find(|item| item.source == "custom" && !item.exists)
    {
        out.push(ConnectorInfo {
            kind: kind.to_string(),
            name: format!("{}（手动指定）", id.display_name()),
            path: custom.path.clone(),
            version: String::new(),
            available: false,
            source: "custom".to_string(),
            note: "设置里指定的路径不存在，请到设置页检查".to_string(),
        });
    }

    if !out.iter().any(|item| item.available) {
        let note = if location.note.trim().is_empty() {
            "没有找到可执行文件".to_string()
        } else {
            location.note.clone()
        };
        out.insert(
            0,
            ConnectorInfo {
                kind: kind.to_string(),
                name: id.display_name().to_string(),
                path: String::new(),
                version: String::new(),
                available: false,
                source: "missing".to_string(),
                note,
            },
        );
    }

    out
}

/// 探测连接器候选
pub fn detect_all(ctx: &AppContext, custom_openconnect: &str) -> Vec<ConnectorInfo> {
    probe_app(AppId::Openconnect, ctx, custom_openconnect)
}

/// 按用户指定的连接器偏好，从探测结果里挑一个可用的
///
/// 现在只剩 openconnect 一条通道，`auto` 与 `openconnect` 的挑选结果完全一致；
/// 保留 `want` 是为了让枚举匹配穷尽，将来再加通道时编译器会强制这里处理。
pub fn pick_connector<'a>(
    candidates: &'a [ConnectorInfo],
    want: ConnectorKind,
) -> Option<&'a ConnectorInfo> {
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

/// 把一份已有的 openconnect 目录导入到程序数据目录并托管，返回版本号。
///
/// 复用通用的 `apps::finder::import_from_dir`（会连同同目录的 DLL 一起复制），
/// 这里额外做一次 `--version` 校验：缺 DLL 的残包必须在导入时就拦下来。
pub fn import_openconnect(data_dir: &Path, source_dir: &Path) -> Result<String, String> {
    apps::finder::import_from_dir(AppId::Openconnect, source_dir, data_dir)?;

    let target_dir = apps::managed_dir(AppId::Openconnect, data_dir);
    let candidates = [target_dir.join("openconnect.exe"), target_dir.join("openconnect")];
    let binary = candidates
        .iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "导入后没有找到 openconnect 可执行文件".to_string())?;

    let (text, ok) = crate::process::run_with_timeout(binary, &["--version"], PROBE_TIMEOUT);
    let version = parse_openconnect_version(&text);
    if !ok || version.is_empty() {
        return Err(format!(
            "导入后校验失败，可能缺少依赖 DLL: {}",
            text.trim().chars().take(200).collect::<String>()
        ));
    }
    Ok(version)
}

/// 删除程序托管的 openconnect
pub fn remove_managed_openconnect(data_dir: &Path) -> Result<(), String> {
    apps::finder::remove_managed(AppId::Openconnect, data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn parse_openconnect_version_missing() {
        assert_eq!(parse_openconnect_version("whatever"), "");
    }

    #[test]
    fn source_suffix_labels_are_distinct() {
        assert_eq!(source_suffix("custom"), "（手动指定）");
        assert_eq!(source_suffix("managed"), "（已导入）");
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
            pick_connector(&list, ConnectorKind::Openconnect).unwrap().kind,
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

    #[test]
    fn detect_all_returns_openconnect_channel() {
        // 空上下文下也要有结果（哪怕是 missing 占位），界面才能明确告诉用户「没找到」
        let ctx = AppContext::default();
        let found = detect_all(&ctx, "");
        assert!(found.iter().any(|item| item.kind == "openconnect"));
        // 只有一条通道，不该再冒出别的 kind
        assert!(found.iter().all(|item| item.kind == "openconnect"));
    }

    #[test]
    fn bundled_openconnect_is_probed_not_skipped() {
        // `apps::locate` 会把内置副本从 candidates 里摘出去、单独放进 builtin_path，
        // 所以 probe_app 必须补探一次。漏了的话内置 openconnect 永远判成 missing，
        // 「自动」模式会直接报「没有可用连接器」—— 用户现场就是这么发生的。
        let bundled = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("apps")
            .join("openconnect")
            .join("openconnect.exe");
        if !bundled.is_file() {
            // 内置二进制体积大，不进版本库；本地没放就跳过，别把测试搞成环境依赖
            return;
        }

        let found = detect_all(&AppContext::default(), "");
        assert!(
            found
                .iter()
                .any(|item| item.kind == "openconnect" && item.source == "bundled"),
            "内置 openconnect 没有被探测，实际结果: {:?}",
            found
                .iter()
                .map(|item| format!("{}:{}", item.kind, item.source))
                .collect::<Vec<_>>()
        );

        // 真正的行为断言：内置副本装好了，「自动」就该选它
        let picked = pick_connector(&found, ConnectorKind::Auto)
            .expect("至少要能选出一个连接器");
        assert_eq!(
            picked.kind, "openconnect",
            "自动模式没选内置 openconnect，而是选了 {}（{}）",
            picked.name, picked.note
        );
    }

    #[test]
    fn missing_custom_path_is_reported_for_openconnect() {
        let ctx = AppContext::default();
        let found = detect_all(&ctx, r"Z:\nope\openconnect.exe");
        let custom = found
            .iter()
            .find(|item| item.source == "custom")
            .expect("应该保留手动指定的记录");
        assert!(!custom.available);
        assert!(custom.note.contains("不存在"), "实际: {}", custom.note);
    }

    #[test]
    fn import_rejects_directory_without_binary() {
        let source = std::env::temp_dir().join(format!("ws-oc-empty-{}", std::process::id()));
        let data = std::env::temp_dir().join(format!("ws-oc-data-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&source);
        let _ = std::fs::create_dir_all(&data);

        let error = import_openconnect(&data, &source).unwrap_err();
        assert!(error.contains("没有找到"), "实际: {}", error);

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&data);
    }
}
