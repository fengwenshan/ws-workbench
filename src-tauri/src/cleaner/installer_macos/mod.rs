//! macOS 侧安装列表、卸载与残留规则。
//!
//! 数据源：`/Applications`、`~/Applications`（含一层子目录，如 `Utilities`）。
//! `/System/Applications` 不扫 —— 系统应用不可卸载，扫出来只会让列表变长。
//!
//! 残留按设计 4.3 用「bundle id / 应用名精确匹配」，不做模糊包含：宁可漏，
//! 不能误删用户别的东西。所有候选路径在这里只负责「生成」，是否真的存在由
//! 扫描阶段用 `symlink_metadata` 判定（`exists=false` 的不返回给前端）。
//!
//! 平台实现按子职责拆在 `bundle` / `list` / `leftovers` / `related` 子模块。

use std::path::{Path, PathBuf};

use super::models::{CleanerProgress, InstalledApp, UninstallOutcome};

mod bundle;
mod list;
mod leftovers;
mod related;

pub use bundle::{is_uninstallable, read_bundle_info};
pub use leftovers::scan_leftovers;
pub use list::list_apps;
pub(crate) use related::related_files;
#[cfg(test)]
pub(crate) use related::app_bundle_copies_in;

// ============================== 自身识别 ==============================

/// 当前进程所在的 `.app` 包。
///
/// 发布版从 `.app/Contents/MacOS/<bin>` 启动，向上一定能找到 `.app`；
/// dev 构建是 `target/debug/ws-workbench-dev` 这样的裸二进制，返回 None
/// —— 此时「不能卸载自己」的过滤不生效（列表里也不会有裸二进制）。
pub(crate) fn current_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    app_bundle_from_exe(&exe)
}

/// 从可执行文件路径向上找第一个 `.app` 目录祖先（单测注入 exe 用）。
///
/// 要求是**目录**且后缀为 `.app`，避免把恰好以 `.app` 结尾的可执行文件误判成包。
pub(crate) fn app_bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.ancestors().find(|ancestor| {
        ancestor.extension().and_then(|ext| ext.to_str()) == Some("app") && ancestor.is_dir()
    })?;
    Some(bundle.canonicalize().unwrap_or_else(|_| bundle.to_path_buf()))
}

/// 目标 bundle 是否就是当前运行的应用本体（canonical 路径比较，软链也能识别）。
pub(crate) fn is_self_bundle(bundle: &Path) -> bool {
    let Some(self_bundle) = current_app_bundle() else {
        return false;
    };
    same_path(&self_bundle, bundle)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

/// 卸载：先结束运行中的实例，再把 bundle 移入废纸篓。
///
/// 返回 `UninstallOutcome` 而不是 Err：SIP 保护 / 应用正被占用都是「用户需要
/// 看到原因」的正常失败，不是命令层异常。
pub fn uninstall(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<UninstallOutcome, String> {
    let bundle = bundle_from_id(app_id)?;
    if !is_uninstallable(&bundle) {
        return Err(format!(
            "{} 是系统应用（受 SIP 保护），不支持卸载",
            hint.map(|app| app.name.as_str()).unwrap_or("该应用")
        ));
    }
    if !bundle.exists() {
        return Err(format!("应用不存在: {}", bundle.display()));
    }

    emit(CleanerProgress::new(
        "uninstall",
        "running",
        0,
        1,
        "正在移入废纸篓…",
    ));
    let command = format!("移入废纸篓: {}", bundle.display());
    let outcome = match super::trash_guard::trash_path(&bundle) {
        Ok(()) => UninstallOutcome {
            ok: true,
            command,
            exit_code: None,
            elevated: false,
            message: "已移入废纸篓".to_string(),
        },
        Err(error) => UninstallOutcome {
            ok: false,
            command,
            exit_code: None,
            elevated: false,
            message: error,
        },
    };
    emit(CleanerProgress::new("uninstall", "done", 1, 1, "卸载结束"));
    Ok(outcome)
}

/// 从 app id（`mac:/Applications/Foo.app`）解析 bundle 路径
fn bundle_from_id(app_id: &str) -> Result<PathBuf, String> {
    let path = app_id
        .strip_prefix("mac:")
        .ok_or_else(|| format!("无效的应用标识: {}", app_id))?;
    let bundle = PathBuf::from(path);
    if bundle.as_os_str().is_empty() {
        return Err(format!("无效的应用标识: {}", app_id));
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::leftovers::{mac_leftover_candidates, mac_leftover_candidates_in};
    use super::related::retain_non_trash;
    use super::super::models::{LeftoverItem, RelatedFileItem};
    use super::super::{dir_size_limited, home_dir};
    use std::path::{Path, PathBuf};

    #[test]
    fn bundle_info_from_plist_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Foo.app/Contents");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("Info.plist"),
            r#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.example.foo</string>
<key>CFBundleName</key><string>Foo</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
</dict></plist>"#,
        )
        .unwrap();
        let info = read_bundle_info(&dir.path().join("Foo.app")).unwrap();
        assert_eq!(info.bundle_id.as_deref(), Some("com.example.foo"));
        assert_eq!(info.name, "Foo");
        assert_eq!(info.version, "1.2.3");
    }

    #[test]
    fn leftover_candidates_for_bundle_id() {
        let paths = mac_leftover_candidates("com.example.foo", "Foo");
        assert!(paths
            .iter()
            .any(|p| p.path.ends_with("Library/Caches/com.example.foo")));
        assert!(paths.iter().any(|p| p.kind == "preference"));
        assert!(paths
            .iter()
            .any(|p| p.path.starts_with("/Library/") && p.requires_elevation));
        // 柠檬补充根：按应用名精确匹配
        assert!(paths.iter().any(|p| p.path == "/Library/Extensions/Foo.kext"));
        assert!(paths.iter().any(|p| p.path == "/Library/PreferencePanes/Foo.prefPane"));
        // 废纸篓规则：low 且默认不勾选
        let trash_item = paths
            .iter()
            .find(|p| p.path.contains("/.Trash/"))
            .expect("应该有废纸篓候选");
        assert_eq!(trash_item.confidence, "low");
        assert!(!trash_item.selected);
    }

    #[test]
    fn saved_state_uses_saved_state_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        let saved = paths
            .iter()
            .find(|item| item.path.contains("Saved Application State"))
            .expect("应该有 savedState 候选");
        assert_eq!(saved.kind, "app_data");
        assert_eq!(saved.confidence, "high");
        assert!(
            saved.path.ends_with("Library/Saved Application State/com.example.foo.savedState"),
            "实际路径 {}",
            saved.path
        );
    }

    #[test]
    fn byhost_and_crash_report_rules_match_real_names() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for rel in [
            "Library/Preferences/ByHost",
            "Library/Application Support/CrashReporter",
            "Library/Logs/DiagnosticReports",
        ] {
            std::fs::create_dir_all(home.join(rel)).unwrap();
        }
        std::fs::write(
            home.join("Library/Preferences/ByHost/com.example.foo.AAAA.plist"),
            b"x",
        )
        .unwrap();
        std::fs::write(
            home.join("Library/Application Support/CrashReporter/Foo_AAAA.plist"),
            b"x",
        )
        .unwrap();
        std::fs::write(
            home.join("Library/Logs/DiagnosticReports/Foo_2026-11-09.ips"),
            b"x",
        )
        .unwrap();
        std::fs::write(
            home.join("Library/Logs/DiagnosticReports/com.example.foo_2026.ips"),
            b"x",
        )
        .unwrap();

        let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("ByHost/com.example.foo.AAAA.plist")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("CrashReporter/Foo_AAAA.plist")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("DiagnosticReports/Foo_2026-11-09.ips")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("DiagnosticReports/com.example.foo_2026.ips")));
    }

    #[test]
    fn binarycookie_candidates_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("HTTPStorages/com.example.foo.binarycookies")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("Cookies/com.example.foo.binarycookies")));
    }

    #[test]
    fn deep_scan_finds_helper_containers_and_group_containers() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for rel in [
            "Library/Containers/com.example.foo.MiniProgram",
            "Library/Group Containers/TEAM.com.example.foo",
            "Library/Application Scripts/com.example.foo.MiniProgram",
        ] {
            std::fs::create_dir_all(home.join(rel)).unwrap();
        }
        let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        for expected in [
            "Library/Containers/com.example.foo.MiniProgram",
            "Library/Group Containers/TEAM.com.example.foo",
            "Library/Application Scripts/com.example.foo.MiniProgram",
        ] {
            let item = items
                .iter()
                .find(|item| item.path == home.join(expected).to_string_lossy())
                .unwrap_or_else(|| panic!("深扫漏了 {}", expected));
            // Group Containers 已有固定 high 规则；两个 helper 容器来自深扫
            if expected.contains("Group Containers") {
                assert_eq!(item.confidence, "high");
            } else {
                assert_eq!(item.confidence, "low");
                assert!(!item.selected);
            }
        }
    }

    #[test]
    fn deep_scan_finds_vendor_dirs_by_bundle_id_last_segment() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for rel in [
            "Library/Application Support/Google/Chrome",
            "Library/Caches/Google/Chrome",
        ] {
            std::fs::create_dir_all(home.join(rel)).unwrap();
        }
        let items = mac_leftover_candidates_in(home, "com.google.Chrome", "Google Chrome");
        for expected in [
            "Library/Application Support/Google/Chrome",
            "Library/Caches/Google/Chrome",
        ] {
            let item = items
                .iter()
                .find(|item| item.path == home.join(expected).to_string_lossy())
                .unwrap_or_else(|| panic!("深扫漏了 {}", expected));
            assert_eq!(item.confidence, "low");
            assert!(!item.selected);
        }
    }

    #[test]
    fn deep_scan_skips_hidden_dirs_and_dedupes_fixed_rules() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("Library/.hidden/com.example.foo")).unwrap();
        std::fs::create_dir_all(home.join("Library/Preferences/ByHost")).unwrap();
        std::fs::write(
            home.join("Library/Preferences/ByHost/com.example.foo.ShipIt.AAAA.plist"),
            b"x",
        )
        .unwrap();
        let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        assert!(!items.iter().any(|item| item.path.contains("/.hidden/")));
        let shipit: Vec<&LeftoverItem> = items
            .iter()
            .filter(|item| item.path.ends_with("com.example.foo.ShipIt.AAAA.plist"))
            .collect();
        assert_eq!(shipit.len(), 1, "固定规则与深扫不应重复");
        assert_eq!(shipit[0].confidence, "high");
        assert!(shipit[0].selected);
    }

    #[test]
    fn deep_scan_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for index in 0..250 {
            std::fs::create_dir_all(
                home.join("Library/Containers").join(format!("com.example.foo.ext{index}")),
            )
            .unwrap();
        }
        let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        let deep = items
            .iter()
            .filter(|item| {
                item.confidence == "low" && item.path.contains("Library/Containers/")
            })
            .count();
        assert!(deep > 0, "应该扫到低置信度条目");
        assert!(deep <= 200, "深扫应截断到 200 条，实际 {deep}");
    }

    #[test]
    fn system_app_is_not_uninstallable() {
        assert!(!is_uninstallable(Path::new("/System/Applications/Safari.app")));
        // 非系统路径（不存在也只是「非系统」，卸载阶段会报应用不存在）
        assert!(is_uninstallable(Path::new("/Applications/Foo.app")));
    }

    #[test]
    fn size_counting_does_not_follow_symlink_loops() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Loop.app/Contents/Resources");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("data.bin"), b"0123456789").unwrap();
        // 指回上层的符号链接：跟随就会无限递归
        std::os::unix::fs::symlink(dir.path(), app.join("loop")).unwrap();
        assert_eq!(dir_size_limited(&dir.path().join("Loop.app"), 64, 10_000), 10);
    }

    // ============================== 自身识别 ==============================

    #[test]
    fn current_app_bundle_is_none_for_bare_binary() {
        // cargo test 的二进制在 target/debug/deps 下，不在 .app 内：
        // dev 构建必须返回 None，否则「不能卸载自己」会把整个列表清空
        assert!(
            current_app_bundle().is_none(),
            "测试二进制不应被识别成 .app 内的进程"
        );
    }

    #[test]
    fn app_bundle_from_exe_walks_up_to_dot_app() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Foo.app");
        let exe = bundle.join("Contents/MacOS/foo");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();

        let found = app_bundle_from_exe(&exe).expect("应能向上找到 .app");
        assert_eq!(found.file_name().unwrap(), "Foo.app");
        // canonicalize 之后应等于 bundle 的真实路径（/var → /private/var 也一致）
        assert_eq!(found, bundle.canonicalize().unwrap());

        // 不在 bundle 内的普通二进制返回 None
        let plain = dir.path().join("plain-bin");
        std::fs::write(&plain, b"x").unwrap();
        assert!(app_bundle_from_exe(&plain).is_none());
    }

    #[test]
    fn is_self_bundle_is_false_for_other_bundles() {
        // 测试进程不在 .app 内 → 任何 bundle 都不算自己
        assert!(!is_self_bundle(Path::new("/Applications/Foo.app")));
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Foo.app");
        std::fs::create_dir_all(&bundle).unwrap();
        assert!(!is_self_bundle(&bundle));
    }

    // ============================== 关联文件（卸载窗口） ==============================

    fn write_bundle(root: &Path, name: &str, bundle_id: &str, version: &str) -> PathBuf {
        let bundle = root.join(format!("{}.app", name));
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{bundle_id}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        bundle
    }

    fn related_item(path: &str, kind: &str, confidence: &str) -> RelatedFileItem {
        RelatedFileItem {
            path: path.to_string(),
            name: Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            kind: kind.to_string(),
            size_bytes: 1,
            is_dir: true,
            version: String::new(),
            confidence: confidence.to_string(),
            requires_elevation: false,
            exists: true,
            selected: false,
        }
    }

    #[test]
    fn related_list_never_contains_trash_entries() {
        let home = home_dir().expect("测试环境应该有 HOME");
        let mut items = vec![
            related_item("/Applications/Foo.app", "app_bundle", "high"),
            // `mac_leftover_candidates` 会给的两条废纸篓 low 条目（支持/缓存也要过滤）
            related_item(
                &home.join(".Trash").join("Foo").to_string_lossy(),
                "cache",
                "low",
            ),
            related_item(
                &home.join(".Trash").join("com.example.foo").to_string_lossy(),
                "cache",
                "low",
            ),
            // 非废纸篓的 low 条目不受影响：照常列出、但默认不勾选
            related_item(
                "/Users/me/Library/Caches/com.example.foo/vendor",
                "cache",
                "low",
            ),
        ];
        retain_non_trash(&mut items);
        let files = super::super::related::build_related_files(
            "mac:/Applications/Foo.app",
            items,
            String::new(),
        );
        let paths: Vec<&str> = files
            .groups
            .iter()
            .flat_map(|group| group.items.iter().map(|item| item.path.as_str()))
            .collect();
        assert!(
            paths.iter().all(|path| !path.contains("/.Trash/")),
            "关联文件清单里不应出现废纸篓条目: {:?}",
            paths
        );
        assert!(
            paths.iter().any(|path| path.ends_with("vendor")),
            "非废纸篓的 low 条目照常列出: {:?}",
            paths
        );
        let low = files
            .groups
            .iter()
            .flat_map(|group| group.items.iter())
            .find(|item| item.confidence == "low")
            .expect("应有 low 条目");
        assert!(!low.selected, "low 默认不勾选；全选是用户显式动作");
    }

    #[test]
    fn app_bundle_copies_in_finds_copies_and_skips_trash() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("Applications");
        let trash = dir.path().join(".Trash");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&trash).unwrap();

        let main = write_bundle(&apps, "Main", "com.example.foo", "1.0.0");
        let _copy = write_bundle(&apps, "Copy", "com.example.foo", "1.1.0");
        // 不同 bundle id 的 .app 不能算副本
        let _other = write_bundle(&apps, "Other", "com.example.bar", "2.0.0");
        // 废纸篓里的同名副本不列（删它等于永久删除）
        let _trashed = write_bundle(&trash, "OldCopy", "com.example.foo", "0.9.0");

        let copies = app_bundle_copies_in(
            &main,
            "com.example.foo",
            &[apps.clone(), trash.clone()],
        );
        assert_eq!(copies.len(), 1, "实际副本: {:?}", copies);
        assert_eq!(
            copies[0].file_name().unwrap().to_string_lossy(),
            "Copy.app",
            "应只保留同 bundle id 且非废纸篓的副本"
        );
        // 主 bundle 自己不会出现在副本列表里
        assert!(!copies.iter().any(|path| path.file_name().unwrap() == "Main.app"));
        // 不同 id 的不会出现
        assert!(!copies.iter().any(|path| path.file_name().unwrap() == "Other.app"));
    }
}
