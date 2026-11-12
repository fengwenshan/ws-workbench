//! VPN 连接器平台层：Windows 实现
//!
//! 这个文件只在 Windows 目标下编译，因此内部不再写 `#[cfg(target_os = "windows")]`。
//!
//! Windows 的模型和 macOS 完全不同：**只认安装包内置的 openconnect**。
//! 不读注册表、不扫 Program Files、不看 PATH，也不允许导入外部副本 ——
//! 「本机装过别的版本」不该把连接行为带偏。候选来源只有一条：
//! `resources/apps/openconnect/openconnect.exe`。

use std::path::Path;

use crate::apps::{self, AppContext, AppId};
use crate::vpn::models::ConnectorInfo;

use super::common::{probe_openconnect, ImportedOpenconnect};

/// 安装包内置 openconnect 缺失时的排查说明。
const MISSING_BUNDLED_NOTE: &str =
    "安装包缺少内置 openconnect：resources/apps/openconnect/openconnect.exe";

/// 探测连接器候选
///
/// Windows 只认安装包内置的 openconnect：不去注册表、常见安装目录或 PATH 里
/// 找第二份，避免「本机装过别的版本」把连接行为带偏。
pub fn detect_all(ctx: &AppContext, custom_openconnect: &str) -> Vec<ConnectorInfo> {
    // Windows 不支持外部副本，调用方传进来的路径直接忽略。
    let _ = custom_openconnect;
    match apps::bundled_binary(AppId::Openconnect, ctx) {
        Some(path) => vec![probe_openconnect(&path, "bundled")],
        None => vec![ConnectorInfo {
            kind: "openconnect".to_string(),
            name: "OpenConnect（内置）".to_string(),
            path: String::new(),
            version: String::new(),
            available: false,
            source: "missing".to_string(),
            note: MISSING_BUNDLED_NOTE.to_string(),
        }],
    }
}

/// Windows 上只认安装包内置的那份，不允许登记外部副本。
pub fn import_openconnect(
    _data_dir: &Path,
    _source_dir: &Path,
) -> Result<ImportedOpenconnect, String> {
    Err("Windows 版只使用安装包内置的 openconnect，不支持导入外部副本".to_string())
}

/// Windows 没有「程序托管的连接器」这个概念：没有登记可移除。
///
/// 保留这个入口是为了让主进程侧命令在不同平台上有统一签名；
/// 内置副本属于安装包资源，不在用户数据目录里，不能被这个命令删掉。
pub fn remove_managed_openconnect(_data_dir: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::models::ConnectorKind;

    // 内置 openconnect 只随 Windows 包分发，也只有 Windows 的 detect_all 会探测 bundled。
    #[test]
    fn bundled_openconnect_is_probed_not_skipped() {
        // 内置副本默认不在 candidates 里，必须单独探测；否则「自动」模式会直接报
        // 「没有可用连接器」—— 用户现场就是这么发生的。
        let ctx = AppContext::default();
        let builtin = apps::bundled_binary(AppId::Openconnect, &ctx);
        if builtin.is_none() {
            // 内置二进制体积大，不进版本库；本地没放就跳过，别把测试搞成环境依赖
            return;
        }

        let probed = detect_all(&ctx, "");
        assert!(
            probed.iter().any(|item| item.source == "bundled"),
            "内置 openconnect 没有被探测，实际结果: {:?}",
            probed
                .iter()
                .map(|item| format!("{}:{}", item.kind, item.source))
                .collect::<Vec<_>>()
        );

        // 真正的行为断言：内置副本装好了，「自动」就该选它
        let picked = crate::vpn::platform::pick_connector(&probed, ConnectorKind::Auto)
            .expect("至少要能选出一个连接器");
        assert_eq!(picked.source, "bundled");
    }

    #[test]
    fn windows_never_falls_back_to_external_openconnect() {
        // 手动指定一份不存在的路径时，Windows 也只能返回内置副本/缺失占位，
        // 不能把系统里装的 openconnect 混进候选。
        let ctx = AppContext::default();
        let found = detect_all(&ctx, r"Z:\nope\openconnect.exe");
        assert!(found.iter().all(|item| item.source != "custom"));
        assert!(found
            .iter()
            .all(|item| item.source == "bundled" || item.source == "missing"));
    }

    #[test]
    fn import_rejects_directory_without_binary() {
        let source = std::env::temp_dir().join(format!("ws-oc-empty-{}", std::process::id()));
        let data = std::env::temp_dir().join(format!("ws-oc-data-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&source);
        let _ = std::fs::create_dir_all(&data);

        let error = import_openconnect(&data, &source).unwrap_err();
        assert!(error.contains("只使用安装包内置"), "实际: {}", error);

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&data);
    }
}
