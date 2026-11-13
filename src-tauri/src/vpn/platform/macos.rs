//! VPN 连接器平台层：macOS 实现
//!
//! 这个文件只在 macOS 目标下编译，因此内部不再写 `#[cfg(target_os = "macos")]`。
//!
//! macOS 现在和 Windows 一样：**只认安装包内置的 openconnect**。
//! - 唯一候选是 `<资源目录>/apps/openconnect/openconnect`（安装包资源映射见
//!   `tauri.macos.conf.json`，源码放在 `src-tauri/resources/openconnect-macos/`）；
//! - 不读 Homebrew、不扫标准前缀、不看 PATH，也不允许导入外部副本 ——
//!   「本机装过别的版本」不该把连接行为带偏；
//! - 开发构建（debug）在资源还没拷进 target 时回退到源码树的
//!   `resources/openconnect-macos/openconnect`，没有它 VPN 页直接报「安装包缺少内置」；
//! - 旧版本留下的 `<data>/vpn/openconnect-path.txt`、副本/软链在检测时清理掉。

use std::path::{Path, PathBuf};

use crate::apps::{self, AppContext, AppId};
use crate::vpn::models::ConnectorInfo;

use super::common::{probe_openconnect, ImportedOpenconnect};

/// 安装包内置 openconnect 缺失时的排查说明。
const MISSING_BUNDLED_NOTE: &str =
    "安装包缺少内置 openconnect：resources/openconnect-macos/openconnect";

/// 安装包内置的 openconnect 位置。
///
/// 发布构建由 `tauri.macos.conf.json` 把 `resources/openconnect-macos` 映射到
/// `Resources/apps/openconnect`，落在 `<资源目录>/apps/openconnect/openconnect`；
/// 开发构建资源不在 target 里，直接指回源码树（`cfg(test)` 下不启用，单测自己造资源目录）。
///
/// 这里不用 `apps::bundled_binary`：它的候选名来自 `AppId::Openconnect.executable_names()`，
/// 包含 Windows 的 `openconnect.exe`，在 macOS 上会把 `resources/apps/openconnect/openconnect.exe`
/// 误当成内置副本（那是给 Windows 包准备的）。macOS 只认无后缀的 `openconnect`。
fn bundled_openconnect(ctx: &AppContext) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for base in [ctx.resource_dir.as_ref(), ctx.exe_dir.as_ref()]
        .into_iter()
        .flatten()
    {
        dirs.push(base.join(apps::finder::APPS_DIR).join("openconnect"));
    }

    #[cfg(all(debug_assertions, not(test)))]
    dirs.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("openconnect-macos"),
    );

    dirs.into_iter()
        .map(|dir| dir.join("openconnect"))
        .find(|path| path.is_file())
}

/// 清理旧版本「选择安装目录」留下的登记与副本/软链。
///
/// 旧版本把用户指定的 openconnect 路径记在 `<data>/vpn/openconnect-path.txt`，
/// 更早的版本还会把二进制复制/软链到 `<data>/apps/openconnect/` 或
/// `<data>/connectors/openconnect/`。改成只认内置副本后这些记录没有意义，
/// 留着只会让人误以为程序还在用外置那份。
fn cleanup_legacy_registration(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join("vpn").join("openconnect-path.txt"));
    for dir in [
        apps::finder::managed_dir(AppId::Openconnect, data_dir),
        data_dir.join(apps::finder::APPS_DIR).join("openconnect"),
    ] {
        if dir.is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// 探测连接器候选
///
/// macOS 只认安装包内置的 openconnect：不去 Homebrew、常见安装目录或 PATH 里
/// 找第二份，也忽略调用方传进来的手动路径。
pub fn detect_all(ctx: &AppContext, custom_openconnect: &str) -> Vec<ConnectorInfo> {
    // macOS 不支持外部副本，调用方传进来的路径直接忽略。
    let _ = custom_openconnect;
    if let Some(data_dir) = ctx.data_dir.as_ref() {
        cleanup_legacy_registration(data_dir);
    }

    match bundled_openconnect(ctx) {
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

/// macOS 上只认安装包内置的那份，不允许登记外部副本。
pub fn import_openconnect(
    _data_dir: &Path,
    _source_dir: &Path,
) -> Result<ImportedOpenconnect, String> {
    Err("macOS 版只使用安装包内置的 openconnect，不支持导入外部副本".to_string())
}

/// macOS 没有「程序托管的连接器」这个概念：顺带把旧版本留下的登记/副本清掉。
///
/// 保留这个入口是为了让主进程侧命令在不同平台上有统一签名；
/// 内置副本属于安装包资源，不在用户数据目录里，不能被这个命令删掉。
pub fn remove_managed_openconnect(data_dir: &Path) -> Result<(), String> {
    cleanup_legacy_registration(data_dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::models::ConnectorKind;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ws-oc-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// 造一个能跑 `--version` 的假 openconnect（真跑一次，不依赖本机装没装）
    fn fake_openconnect(dir: &Path, version: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("openconnect");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho 'OpenConnect version {}'\n", version),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// 内置副本的发布布局：`<资源目录>/apps/openconnect/openconnect`
    fn ctx_with_bundled(root: &Path, version: &str) -> AppContext {
        let resource = root.join("resources");
        fake_openconnect(&resource.join("apps/openconnect"), version);
        AppContext {
            resource_dir: Some(resource),
            ..Default::default()
        }
    }

    fn ctx_without_bundled(root: &Path) -> AppContext {
        let resource = root.join("resources");
        std::fs::create_dir_all(&resource).unwrap();
        AppContext {
            resource_dir: Some(resource),
            ..Default::default()
        }
    }

    #[test]
    fn bundled_openconnect_is_detected_with_version() {
        let root = temp_root("detect-bundled");
        let ctx = ctx_with_bundled(&root, "v9.21");

        let found = detect_all(&ctx, "");
        let picked = found
            .iter()
            .find(|item| item.available)
            .unwrap_or_else(|| panic!("内置副本必须被识别，实际: {:?}", found));
        assert_eq!(picked.source, "bundled");
        assert_eq!(picked.version, "v9.21");
        assert_eq!(picked.name, "OpenConnect（内置）");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_bundled_copy_is_reported() {
        let root = temp_root("detect-missing");
        let ctx = ctx_without_bundled(&root);

        let found = detect_all(&ctx, "");
        assert!(!found.iter().any(|item| item.available));
        let missing = found
            .iter()
            .find(|item| item.source == "missing")
            .expect("内置副本缺失时必须给出 missing 占位");
        assert!(
            missing
                .note
                .contains("resources/openconnect-macos/openconnect"),
            "缺失提示要指出资源路径，实际: {}",
            missing.note
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只认内置：调用方传进来的手动路径（老配置里的 connector_path）不能生效。
    #[test]
    fn custom_path_is_ignored() {
        let root = temp_root("custom-ignored");
        let ctx = ctx_with_bundled(&root, "v9.21");

        let found = detect_all(&ctx, "Z:\\nope\\openconnect.exe");
        assert!(found.iter().all(|item| item.source != "custom"));
        assert_eq!(found.len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 升级到「只认内置」后，旧版本登记的路径与副本/软链要在检测时清掉。
    #[test]
    fn detection_cleans_legacy_registration() {
        let root = temp_root("cleanup-legacy");
        let data = root.join("data");
        let ctx = ctx_with_bundled(&root, "v9.21");
        let ctx = AppContext {
            data_dir: Some(data.clone()),
            ..ctx
        };

        std::fs::create_dir_all(data.join("vpn")).unwrap();
        std::fs::write(
            data.join("vpn").join("openconnect-path.txt"),
            "/opt/homebrew/bin/openconnect\n",
        )
        .unwrap();
        let legacy_apps = data.join(apps::finder::APPS_DIR).join("openconnect");
        std::fs::create_dir_all(&legacy_apps).unwrap();
        std::fs::write(legacy_apps.join("openconnect"), b"old copied binary").unwrap();
        let legacy_managed = apps::finder::managed_dir(AppId::Openconnect, &data);
        std::fs::create_dir_all(&legacy_managed).unwrap();
        std::fs::write(legacy_managed.join("openconnect"), b"old symlink").unwrap();

        let found = detect_all(&ctx, "");
        assert_eq!(found[0].source, "bundled");
        assert!(
            !data.join("vpn").join("openconnect-path.txt").exists(),
            "旧路径登记要删掉"
        );
        assert!(!legacy_apps.exists(), "旧副本目录要删掉");
        assert!(!legacy_managed.exists(), "旧托管目录要删掉");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_rejects_any_directory() {
        let source = std::env::temp_dir().join(format!("ws-oc-empty-{}", std::process::id()));
        let data = std::env::temp_dir().join(format!("ws-oc-data-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&source);
        let _ = std::fs::create_dir_all(&data);

        let error = import_openconnect(&data, &source).unwrap_err();
        assert!(error.contains("只使用安装包内置"), "实际: {}", error);

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn remove_managed_cleans_legacy_but_keeps_bundle() {
        let root = temp_root("remove-managed");
        let data = root.join("data");
        std::fs::create_dir_all(data.join("vpn")).unwrap();
        std::fs::write(data.join("vpn").join("openconnect-path.txt"), "/tmp/x\n").unwrap();
        let ctx = ctx_with_bundled(&root, "v9.21");

        remove_managed_openconnect(&data).unwrap();
        assert!(!data.join("vpn").join("openconnect-path.txt").exists());
        assert_eq!(detect_all(&ctx, "")[0].source, "bundled");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 探测子进程必须带 LC_ALL=C：否则中文系统上 `--version` 输出
    /// 「OpenConnect 版本 v9.21」，解析不到英文前缀就判成不可用。
    #[test]
    fn probe_runs_openconnect_under_c_locale() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("probe-locale");
        let dir = root.join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("openconnect");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"OpenConnect version ${LC_ALL:-unset}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let info = probe_openconnect(&fake, "bundled");
        assert!(info.available, "note: {}", info.note);
        assert_eq!(info.version, "C", "探测必须把子进程 locale 钉成 C");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 探测结果的备注要区分「跑不起来」和「能跑但认不出版本号」
    #[test]
    fn probe_note_separates_parse_failure_from_launch_failure() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("probe-note");
        let dir = root.join("hb/bin");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("openconnect");
        std::fs::write(&fake, "#!/bin/sh\necho 'no version here'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let info = probe_openconnect(&fake, "bundled");
        assert!(!info.available);
        assert!(
            info.note.contains("没从 --version 输出里识别出版本号"),
            "实际: {}",
            info.note
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 连接器偏好只认 openconnect：这一层与平台无关，但放在 mac 用例里做一次端到端确认
    #[test]
    fn detect_all_result_can_be_picked() {
        let root = temp_root("pick");
        let ctx = ctx_with_bundled(&root, "v9.21");
        let found = detect_all(&ctx, "");
        let picked = crate::vpn::platform::pick_connector(&found, ConnectorKind::Auto);
        assert_eq!(picked.is_some(), found.iter().any(|item| item.available));
        assert_eq!(picked.unwrap().source, "bundled");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 内置运行时是本功能的核心：源码树里必须有一份能跑的 openconnect。
    ///
    /// 这个用例刻意依赖 `src-tauri/resources/openconnect-macos/openconnect` 存在，
    /// 用来拦住「忘了提交内置产物 / 提交了跑不起来的二进制」。
    #[test]
    fn source_tree_bundled_runtime_is_present_and_runnable() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("openconnect-macos")
            .join("openconnect");
        assert!(
            path.is_file(),
            "源码树里缺少内置 openconnect：{}（见 resources/openconnect-macos/README.md 的构建步骤）",
            path.display()
        );

        let info = probe_openconnect(&path, "bundled");
        assert!(
            info.available,
            "内置 openconnect 跑不起来：{}（运行 `otool -L` 确认没有 Homebrew 动态库依赖）",
            info.note
        );
        assert_eq!(info.version, "v9.21", "内置 openconnect 版本不对");
    }
}
