//! macOS 实机冒烟测试（默认 `#[ignore]`）。
//!
//! 单测只覆盖纯逻辑（过滤/规则/解析），但「列表 → 卸载 → 残留扫描 → 删进废纸篓」
//! 这条真实链路涉及 Info.plist 解析、sips 图标、`trash` crate 等只有真机才能验证的行为。本文件把这条链路
//! 跑一遍，且**所有写入都限定在测试自己创建的临时名字下**，结束（含失败）时清理。
//!
//! 手动执行：
//! ```text
//! CLEANER_SMOKE=1 CARGO_HOME="$PWD/.tmp/cargo-home" \
//!   cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored cleaner::macos_smoke
//! ```

use std::path::{Path, PathBuf};

use super::models::CleanerProgress;
use super::{installer, installer_macos};

fn emit(_progress: CleanerProgress) {}

/// 测试期间创建的所有路径，Drop 时（含断言失败）逐条清掉，避免污染本机。
struct Cleanup(Vec<PathBuf>);

impl Cleanup {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn track(&mut self, path: impl Into<PathBuf>) {
        self.0.push(path.into());
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// 把 `~/.Trash` 里本次测试产生（按唯一前缀）的条目永久删掉。
fn purge_trash_prefixes(home: &Path, prefixes: &[String]) {
    let trash = home.join(".Trash");
    let Ok(entries) = std::fs::read_dir(&trash) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if prefixes.iter().any(|prefix| name.starts_with(prefix.as_str())) {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// `~/.Trash` 在 macOS 上属于受保护目录：没有「完全磁盘访问」权限的进程读取会被
/// TCC 拒绝（本机实测 EPERM）。所以这里只做「尽力检查」：能读就断言，读不到就
/// 打印说明跳过 —— 不能把系统权限限制误判成功能失败。
fn trash_contains(home: &Path, name: &str) -> Option<bool> {
    let entries = std::fs::read_dir(home.join(".Trash")).ok()?;
    Some(entries.flatten().any(|entry| {
        entry.file_name().to_string_lossy().starts_with(name)
    }))
}

fn check_trash(home: &Path, name: &str) {
    match trash_contains(home, name) {
        Some(true) => eprintln!("[smoke] 废纸篓确认：{}", name),
        Some(false) => panic!("{} 应已移入废纸篓，但废纸篓里没找到", name),
        None => eprintln!(
            "[smoke] 无法读取 ~/.Trash（macOS TCC 限制），跳过废纸篓内容检查：{}",
            name
        ),
    }
}

#[test]
#[ignore = "实机冒烟：会真实写入 ~/Applications、~/Library 与废纸篓，需 CLEANER_SMOKE=1"]
fn macos_uninstall_leftover_smoke() {
    if std::env::var("CLEANER_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("未设置 CLEANER_SMOKE=1，跳过实机冒烟（这是有意的保护）");
        return;
    }
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    // 用 pid + 时间戳保证唯一，避免和上次失败残留/用户真实应用撞名
    let unique = format!(
        "wsworkbench-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let bundle_id = format!("com.wsworkbench.smoke.{}", unique);
    let app_name = unique.clone();
    let mut cleanup = Cleanup::new();
    let mut trash_prefixes = vec![
        unique.clone(),
        bundle_id.clone(),
        format!("{}.plist", bundle_id),
    ];

    let apps_root = home.join("Applications");
    std::fs::create_dir_all(&apps_root).expect("创建 ~/Applications");
    let bundle = apps_root.join(format!("{}.app", unique));
    let contents = bundle.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS")).expect("创建 bundle 目录");
    std::fs::write(
        contents.join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{bundle_id}</string>
<key>CFBundleName</key><string>{app_name}</string>
<key>CFBundleDisplayName</key><string>{app_name}</string>
<key>CFBundleShortVersionString</key><string>1.0.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleExecutable</key><string>payload</string>
</dict></plist>"#
        ),
    )
    .expect("写 Info.plist");
    std::fs::write(contents.join("MacOS/payload"), "#!/bin/sh\nexit 0\n")
        .expect("写可执行文件");
    cleanup.track(&bundle);

    // 造三份「标准残留」：缓存目录、偏好 plist、Application Support 目录
    let cache_dir = home.join("Library/Caches").join(&bundle_id);
    let prefs = home
        .join("Library/Preferences")
        .join(format!("{}.plist", bundle_id));
    let support = home.join("Library/Application Support").join(&bundle_id);
    for (path, file) in [
        (&cache_dir, "cache.bin"),
        (&support, "data.bin"),
    ] {
        std::fs::create_dir_all(path).expect("创建残留目录");
        std::fs::write(path.join(file), b"cleaner-smoke").expect("写残留文件");
        cleanup.track(path);
    }
    std::fs::write(&prefs, b"cleaner-smoke").expect("写偏好 plist");
    cleanup.track(&prefs);

    // 1) 列表：必须能扫到刚造的 app，且可卸载
    let apps = installer_macos::list_apps(&emit);
    let app_id = format!("mac:{}", bundle.display());
    let listed = apps
        .iter()
        .find(|app| app.id == app_id)
        .unwrap_or_else(|| panic!("列表里没有扫到 {}，共 {} 条", bundle.display(), apps.len()));
    assert!(listed.can_uninstall, "测试 app 应为可卸载");
    assert_eq!(listed.name, app_name);
    assert_eq!(listed.version, "1.0.0");
    eprintln!("[smoke] 列表扫描：{} 条，测试 app name={}", apps.len(), listed.name);

    // 2) 残留扫描：命中三份标准残留，且 high 默认勾选
    let leftovers = installer_macos::scan_leftovers(&app_id, Some(listed), &emit).expect("残留扫描");
    for expected in [&cache_dir, &prefs, &support] {
        let item = leftovers
            .iter()
            .find(|item| Path::new(&item.path) == expected.as_path())
            .unwrap_or_else(|| panic!("残留扫描漏了 {}", expected.display()));
        assert_eq!(item.confidence, "high", "{} 应为 high", expected.display());
        assert!(item.selected, "{} 应默认勾选", expected.display());
        assert!(item.exists);
    }
    eprintln!("[smoke] 残留扫描：{} 条", leftovers.len());

    // 3) 删除：真实进废纸篓，路径消失
    let selected: Vec<String> = [&cache_dir, &prefs, &support]
        .iter()
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    assert_eq!(selected.len(), 3, "三份残留都应存在");
    let outcome = installer::delete_leftovers(&app_id, Some(listed), &selected, &emit).expect("删除残留");
    assert!(
        outcome.failed.is_empty(),
        "删除失败：{:?}",
        outcome.failed
    );
    assert_eq!(outcome.trashed, selected.len());
    for path in [&cache_dir, &prefs, &support] {
        assert!(!path.exists(), "{} 应已移出原位置", path.display());
    }
    check_trash(&home, &bundle_id);
    eprintln!("[smoke] 删除：{} 项进废纸篓", outcome.trashed);

    // 4) 卸载：app 包真实移入废纸篓
    let uninstall = installer_macos::uninstall(&app_id, Some(listed), &emit).expect("卸载命令");
    assert!(uninstall.ok, "卸载失败：{}", uninstall.message);
    assert!(!bundle.exists(), "app 包应已移出 ~/Applications");
    check_trash(&home, &unique);
    eprintln!("[smoke] 卸载：{}", uninstall.command);

    // 5) 清理：废纸篓里本次测试的条目也只是我们造的，永久删掉
    trash_prefixes.sort();
    trash_prefixes.dedup();
    purge_trash_prefixes(&home, &trash_prefixes);
    // 清掉空掉的 ~/Applications（只在它本来就为空时才删，避免动用户目录）
    let _ = std::fs::remove_dir(&apps_root);
    eprintln!("[smoke] 完成，测试写入已清理");
}
