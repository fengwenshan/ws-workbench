//! macOS 运行状态检测、优雅退出与强杀实现。
//!
//! 平台实现文件：只在 macOS target 编译，禁止引用 `running::windows`。

use std::path::PathBuf;

use super::super::models::InstalledApp;
use super::{path_within, sysinfo_processes, RunningProcess};

pub(super) fn mac_bundle_path(app: &InstalledApp) -> Option<PathBuf> {
    app.id.strip_prefix("mac:").map(PathBuf::from)
}

/// macOS 检测：sysinfo 里 exe 在 `.app` 内的进程（覆盖 helper / XPC / 扩展）
/// + `NSRunningApplication` 按 bundle id 找到的主应用，按 pid 去重。
pub(super) fn detect(app: &InstalledApp) -> Vec<RunningProcess> {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    use std::collections::HashMap;

    let Some(bundle) = mac_bundle_path(app) else {
        return Vec::new();
    };
    let mut found: HashMap<u32, RunningProcess> = HashMap::new();
    for process in sysinfo_processes() {
        if let Some(exe) = &process.exe {
            if path_within(exe, &bundle) {
                found.insert(process.pid, process);
            }
        }
    }
    if let Some(info) = crate::cleaner::installer_macos::read_bundle_info(&bundle) {
        if let Some(bundle_id) = info.bundle_id {
            let ns_id = NSString::from_str(&bundle_id);
            let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns_id);
            for application in running.iter() {
                let pid = application.processIdentifier() as u32;
                let name = application
                    .localizedName()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| app.name.clone());
                found.entry(pid).or_insert(RunningProcess {
                    pid,
                    name,
                    exe: None,
                });
            }
        }
    }
    found.into_values().collect()
}

/// macOS 退出：优先按 bundle id 拿 `NSRunningApplication`，再按 pid 补 helper 实例，
/// 逐个 `terminate()`（等价于点 Dock 菜单里的「退出」，应用可以弹保存确认框）。
pub(super) fn terminate(app: &InstalledApp, processes: &[RunningProcess]) {
    use objc2::rc::Retained;
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    use std::collections::HashMap;

    let mut handles: HashMap<libc::pid_t, Retained<NSRunningApplication>> = HashMap::new();
    if let Some(bundle) = mac_bundle_path(app) {
        if let Some(info) = crate::cleaner::installer_macos::read_bundle_info(&bundle) {
            if let Some(bundle_id) = info.bundle_id {
                let ns_id = NSString::from_str(&bundle_id);
                let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns_id);
                for application in running.iter() {
                    handles.insert(application.processIdentifier(), application);
                }
            }
        }
    }
    for process in processes {
        let pid = process.pid as libc::pid_t;
        if let Some(application) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        {
            handles.insert(pid, application);
        }
    }
    for application in handles.values() {
        let _ = application.terminate();
    }
}

/// macOS 强杀：逐 pid `SIGKILL`（`kill -9` 的等价调用）。
///
/// `kill` 失败（进程已退出 / 无权限）不影响后续轮询：下一轮 `detect` 会给出
/// 真实状态，退不掉的进程最终由超时判定报出。
pub(super) fn kill(processes: &[RunningProcess]) {
    for process in processes {
        unsafe {
            libc::kill(process.pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
