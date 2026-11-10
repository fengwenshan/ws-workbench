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
use super::{installer, installer_macos, running};

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

/// 子进程守卫：断言失败（panic）或提前 return 时也把测试进程杀掉，避免留下 `sleep 300`。
struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 实机冒烟 2：卸载窗口链路「关联扫描 → 运行检测 → 强制退出 → 卸载 → 白名单删除」。
///
/// 与上面的冒烟一样只写测试唯一名的路径（`~/Applications/<unique>.app` 与
/// `~/Library` 三份残留），结束（含失败）时清理并清掉本次写进废纸篓的条目。
#[test]
#[ignore = "实机冒烟：会真实写入 ~/Applications、~/Library 与废纸篓，需 CLEANER_SMOKE=1"]
fn macos_related_scan_force_quit_smoke() {
    if std::env::var("CLEANER_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("未设置 CLEANER_SMOKE=1，跳过实机冒烟（这是有意的保护）");
        return;
    }
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    let unique = format!(
        "wsworkbench-related-{}-{}",
        std::process::id(),
        unix_seconds()
    );
    let bundle_id = format!("com.wsworkbench.smoke.related.{}", unique);
    let app_name = unique.clone();
    let mut cleanup = Cleanup::new();
    let mut trash_prefixes = vec![
        unique.clone(),
        bundle_id.clone(),
        format!("{}.plist", bundle_id),
    ];

    // ---------- 1) 造假 app：可执行文件用 /bin/sleep 的副本，便于起真实进程 ----------
    let apps_root = home.join("Applications");
    std::fs::create_dir_all(&apps_root).expect("创建 ~/Applications");
    let bundle = apps_root.join(format!("{}.app", unique));
    let contents = bundle.join("Contents");
    let macos_dir = contents.join("MacOS");
    std::fs::create_dir_all(&macos_dir).expect("创建 bundle 目录");
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
    let payload = macos_dir.join("payload");
    std::fs::copy("/bin/sleep", &payload).expect("复制 /bin/sleep 作为 bundle 内可执行文件");
    cleanup.track(&bundle);

    // 三份「标准残留」：缓存目录、偏好 plist、Application Support 目录
    let cache_dir = home.join("Library/Caches").join(&bundle_id);
    let prefs = home
        .join("Library/Preferences")
        .join(format!("{}.plist", bundle_id));
    let support = home.join("Library/Application Support").join(&bundle_id);
    for (path, file) in [(&cache_dir, "cache.bin"), (&support, "data.bin")] {
        std::fs::create_dir_all(path).expect("创建残留目录");
        std::fs::write(path.join(file), b"cleaner-related-smoke").expect("写残留文件");
        cleanup.track(path);
    }
    std::fs::write(&prefs, b"cleaner-related-smoke").expect("写偏好 plist");
    cleanup.track(&prefs);

    // ---------- 2) 关联文件扫描：三组 / 默认勾选 / 命中残留 ----------
    let apps = installer_macos::list_apps(&emit);
    let app_id = format!("mac:{}", bundle.display());
    let listed = apps
        .iter()
        .find(|app| app.id == app_id)
        .unwrap_or_else(|| panic!("列表里没有扫到 {}，共 {} 条", bundle.display(), apps.len()));

    let files =
        installer::related_files(&app_id, Some(listed), &emit).expect("关联文件扫描失败");
    let program = files
        .groups
        .iter()
        .find(|group| group.id == "program")
        .unwrap_or_else(|| panic!("缺少「程序文件」组：{:?}", files.groups));
    let main_item = program
        .items
        .iter()
        .find(|item| Path::new(&item.path) == bundle.as_path())
        .unwrap_or_else(|| panic!("程序文件组里没有测试 app：{:?}", program.items));
    assert_eq!(main_item.kind, "app_bundle");
    assert_eq!(main_item.version, "1.0.0");
    assert!(main_item.selected, "app_bundle 应默认勾选");

    let support_group = files
        .groups
        .iter()
        .find(|group| group.id == "support")
        .unwrap_or_else(|| panic!("缺少「支持文件」组：{:?}", files.groups));
    let prefs_item = support_group
        .items
        .iter()
        .find(|item| Path::new(&item.path) == prefs.as_path())
        .unwrap_or_else(|| panic!("支持文件组里没有偏好 plist：{:?}", support_group.items));
    assert_eq!(prefs_item.kind, "preference");
    assert_eq!(prefs_item.confidence, "high");
    assert!(prefs_item.selected);
    let support_item = support_group
        .items
        .iter()
        .find(|item| Path::new(&item.path) == support.as_path())
        .unwrap_or_else(|| panic!("支持文件组里没有 Application Support：{:?}", support_group.items));
    assert!(support_item.selected, "high app_data 应默认勾选");

    let cache_group = files
        .groups
        .iter()
        .find(|group| group.id == "cache")
        .unwrap_or_else(|| panic!("缺少「缓存文件」组：{:?}", files.groups));
    let cache_item = cache_group
        .items
        .iter()
        .find(|item| Path::new(&item.path) == cache_dir.as_path())
        .unwrap_or_else(|| panic!("缓存文件组里没有缓存目录：{:?}", cache_group.items));
    assert_eq!(cache_item.kind, "cache");
    assert!(cache_item.selected);
    assert!(files.item_count >= 4, "至少 app + 三份残留，实际 {}", files.item_count);
    assert!(files.total_bytes > 0, "合计大小应大于 0");
    eprintln!(
        "[smoke] 关联文件：{} 组 / {} 项 / {} 字节 / last_used={:?}",
        files.groups.len(),
        files.item_count,
        files.total_bytes,
        files.last_used
    );

    // ---------- 3) 起 bundle 内进程 → detect 命中 → 强制退出到归零 ----------
    let mut child = ChildGuard(
        std::process::Command::new(&payload)
            .arg("300")
            .spawn()
            .expect("spawn bundle 内进程"),
    );
    // sysinfo 刷新有延迟：轮询到 detect 命中为止（最多 4 秒）
    let mut detected = running::detect(listed);
    for _ in 0..40 {
        if detected.running {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        detected = running::detect(listed);
    }
    assert!(
        detected.running,
        "detect 应命中 bundle 内进程（exe={}）",
        payload.display()
    );
    assert!(detected.processes >= 1);
    eprintln!(
        "[smoke] 运行检测：{} 个进程 {:?}",
        detected.processes, detected.names
    );

    let quit = running::force_quit(listed, &emit).expect("强制退出命令失败");
    assert!(quit.ok, "强制退出失败：{}", quit.message);
    assert_eq!(quit.processes, 0, "强制退出后应没有进程");
    // 子进程应真的消失（SIGTERM 或 SIGKILL 都算通过）
    let mut status = None;
    for _ in 0..50 {
        status = child.0.try_wait().expect("try_wait 失败");
        if status.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let status = status.unwrap_or_else(|| panic!("bundle 内进程在强制退出后仍在运行"));
    eprintln!("[smoke] 强制退出：进程结束 {:?}", status);

    // ---------- 4) 卸载（app 进废纸篓）→ 白名单删除关联文件 ----------
    let uninstall =
        installer_macos::uninstall(&app_id, Some(listed), &emit).expect("卸载命令失败");
    assert!(uninstall.ok, "卸载失败：{}", uninstall.message);
    assert!(!bundle.exists(), "app 包应已移出 ~/Applications");

    // 白名单 = 第一次扫描时「存在」的路径（含随后会被卸载程序移走的 app 包）
    let allowed: Vec<String> = files
        .groups
        .iter()
        .flat_map(|group| group.items.iter())
        .filter(|item| item.exists)
        .map(|item| item.path.clone())
        .collect();
    // 勾选：三份残留 + app 包（app 包已被卸载程序移走 → 应记 skipped）
    let mut selected: Vec<String> = [&cache_dir, &prefs, &support]
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    selected.push(bundle.to_string_lossy().to_string());

    let outcome = installer::delete_related(&app_id, Some(listed), &allowed, &selected, &emit)
        .expect("删除关联文件失败");
    assert!(
        outcome.failed.is_empty(),
        "删除失败：{:?}",
        outcome.failed
    );
    assert_eq!(outcome.trashed, 3, "三份残留应进废纸篓，实际 {:?}", outcome);
    assert!(
        outcome.skipped >= 1,
        "已随卸载消失的 app 包应记 skipped，实际 {:?}",
        outcome
    );
    for path in [&cache_dir, &prefs, &support] {
        assert!(!path.exists(), "{} 应已移出原位置", path.display());
    }
    check_trash(&home, &bundle_id);
    eprintln!(
        "[smoke] 关联删除：trashed={} skipped={} failed={}",
        outcome.trashed,
        outcome.skipped,
        outcome.failed.len()
    );

    // ---------- 5) 清理：废纸篓里本次测试的条目永久删掉 ----------
    trash_prefixes.sort();
    trash_prefixes.dedup();
    purge_trash_prefixes(&home, &trash_prefixes);
    let _ = std::fs::remove_dir(&apps_root);
    eprintln!("[smoke] 关联文件冒烟完成，测试写入已清理");
}
