//! macOS 侧内存占用的平台实现：`.app` 归属、图标、优雅/强制退出应用、深度释放。
//!
//! 进程列表来自 `sysinfo`，但「这个进程属于哪个应用」在 macOS 上要靠 exe 路径
//! 向上找最近的 `.app` 祖先（Chrome 的 helper、Electron 的渲染进程都各自是独立
//! 进程，只按 exe 分组会把一个应用拆成一屏）；退出应用用 `NSRunningApplication`，
//! 它才是 AppKit 认可的「正常退出」通道（等价于点 Dock 菜单里的「退出」）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use objc2_app_kit::NSRunningApplication;
use objc2_foundation::NSString;

use super::models::MemoryApp;
use super::memory::ProcSample;

/// 图标缓存：`sips` 转 PNG 一次要几十毫秒，而内存页签每 2 秒采样一次，
/// 按 bundle 路径缓存后只有首屏会真正调用 sips。
static ICON_CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// 应用展示名缓存（读 Info.plist 也有 IO 成本）
static NAME_CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// 从 exe 路径向上找最近的 `.app` 祖先
pub fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    let mut current = PathBuf::new();
    for component in exe.components() {
        current.push(component);
        let name = component.as_os_str().to_string_lossy();
        if name.ends_with(".app") {
            return Some(current);
        }
    }
    None
}

/// 应用展示名：优先 Info.plist 的 CFBundleDisplayName/CFBundleName，退化到包名
fn bundle_name(bundle: &Path) -> String {
    if let Some(cache) = NAME_CACHE.lock().ok().and_then(|cache| cache.as_ref().cloned()) {
        if let Some(name) = cache.get(&bundle.to_string_lossy().to_string()) {
            return name.clone();
        }
    }
    let name = super::installer_macos::read_bundle_info(bundle)
        .map(|info| info.name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            bundle
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| "未知应用".to_string())
        });
    if let Ok(mut guard) = NAME_CACHE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(bundle.to_string_lossy().to_string(), name.clone());
    }
    name
}

/// 进程 → 应用身份（id, 展示名）。
///
/// id 规则：能找到 `.app` 祖先时用 bundle 路径（同应用的所有 helper 进程因此
/// 聚到一起）；找不到时退化成 exe 路径单独成项。
pub fn identity(sample: &ProcSample) -> (String, String) {
    let exe = Path::new(&sample.exe);
    match app_bundle_of(exe) {
        Some(bundle) => {
            let name = bundle_name(&bundle);
            (bundle.to_string_lossy().to_string(), name)
        }
        None => (sample.exe.clone(), sample.name.clone()),
    }
}

/// 应用图标（data URL），按 bundle 路径缓存
pub fn icon_for(id: &str) -> Option<String> {
    if !id.ends_with(".app") {
        return None;
    }
    if let Some(cache) = ICON_CACHE.lock().ok().and_then(|cache| cache.as_ref().cloned()) {
        if let Some(icon) = cache.get(id) {
            return icon.clone();
        }
    }
    let icon = crate::commands::openwith::mac_icon(Path::new(id));
    if let Ok(mut guard) = ICON_CACHE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(id.to_string(), icon.clone());
    }
    icon
}

/// 按 bundle id 优雅/强制退出应用。
///
/// 注意 `terminate()` 只是「请求退出」（应用可以弹保存确认框），返回值表示请求
/// 是否发出，不代表进程已经没了；前端等 3 秒后会重新采样，必要时再调强制结束。
pub fn end_running_apps(bundle_ids: &[String], force: bool) -> (usize, usize, Vec<String>) {
    let mut count = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for bundle_id in bundle_ids {
        let ns_id = NSString::from_str(bundle_id);
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns_id);
        if apps.count() == 0 {
            // 应用已经不在运行：不算失败，前端只是少结束一个
            continue;
        }
        for app in apps.iter() {
            let sent = if force {
                app.forceTerminate()
            } else {
                app.terminate()
            };
            if sent {
                count += 1;
            } else {
                failed.push(bundle_id.clone());
            }
        }
    }
    if force {
        (0, count, failed)
    } else {
        (count, 0, failed)
    }
}

/// 按 pid 优雅/强制退出进程（内存列表里按 exe 单独成项的那些）。
pub fn end_pids(pids: &[u32], force: bool) -> (usize, usize, Vec<String>) {
    let mut count = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for pid in pids {
        let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(*pid as libc::pid_t)
        else {
            failed.push(format!("pid {}", pid));
            continue;
        };
        let sent = if force {
            app.forceTerminate()
        } else {
            app.terminate()
        };
        if sent {
            count += 1;
        } else {
            failed.push(format!("pid {}", pid));
        }
    }
    if force {
        (0, count, failed)
    } else {
        (count, 0, failed)
    }
}

/// 汇总一次退出操作的结果
pub fn quit_apps(apps: &[MemoryApp], force: bool) -> super::models::QuitOutcome {
    let mut outcome = super::models::QuitOutcome::default();
    // bundle 应用走 bundle id（能覆盖 helper 进程的父应用），其他按 pid 走
    let mut bundle_ids: Vec<String> = Vec::new();
    let mut pids: Vec<u32> = Vec::new();
    for app in apps {
        if app.kind == "app" && app.id.ends_with(".app") {
            if let Some(info) = super::installer_macos::read_bundle_info(Path::new(&app.id)) {
                if let Some(bundle_id) = info.bundle_id {
                    bundle_ids.push(bundle_id);
                    continue;
                }
            }
        }
        pids.extend(app.pids.iter().copied());
    }

    let (quit, forced, mut failed) = end_running_apps(&bundle_ids, force);
    outcome.quit += quit;
    outcome.forced += forced;
    outcome.failed.append(&mut failed);

    let (quit, forced, mut failed) = end_pids(&pids, force);
    outcome.quit += quit;
    outcome.forced += forced;
    outcome.failed.append(&mut failed);
    outcome
}

/// 深度释放：一次系统授权跑 `/usr/sbin/purge`。
///
/// purge 只回收可回收的文件缓存（inactive/可重读的页），不会释放应用已 malloc
/// 的内存，所以收益有限 —— 这条说明由 `memory::deep_release` 直接展示给用户。
pub fn deep_release() -> Result<(), String> {
    let apple_script = "do shell script \"/usr/sbin/purge\" with administrator privileges";
    let (text, ok) = crate::process::run_with_timeout(
        Path::new("/usr/bin/osascript"),
        &["-e", apple_script],
        std::time::Duration::from_secs(120),
    );
    if ok {
        return Ok(());
    }
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("cancel") || text.contains("已取消") || text.contains("-128") {
        return Err("已取消授权".to_string());
    }
    Err(format!("purge 执行失败: {}", text.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_app_bundle_ancestor() {
        let bundle = app_bundle_of(Path::new(
            "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper",
        ));
        assert_eq!(
            bundle.as_deref(),
            Some(Path::new("/Applications/Google Chrome.app"))
        );
        assert!(app_bundle_of(Path::new("/usr/bin/ssh")).is_none());
    }

    #[test]
    fn identity_groups_all_helpers_under_the_app() {
        let sample = ProcSample {
            pid: 42,
            name: "Google Chrome Helper".to_string(),
            exe: "/Applications/Google Chrome.app/Contents/Frameworks/Helper".to_string(),
            memory: 1024,
        };
        let (id, name) = identity(&sample);
        assert_eq!(id, "/Applications/Google Chrome.app");
        // 本机没装 Chrome 时名字会退化成包名，但绝不能是空的
        assert!(!name.is_empty());
    }
}
