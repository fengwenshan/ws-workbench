//! 卸载前的运行状态检测与优雅退出。
//!
//! 检测只读、不需要提权；退出先走平台原生「正常退出」通道，
//! 5 秒内退不干净就失败，由卸载流程中止，不强制杀进程。
//!
//! 平台差异与 `installer_*` 保持同一套约定：公共逻辑（进程去重、路径匹配、
//! 安装根校验、轮询）在 test cfg 下也会编进 macOS 测试，便于在 macOS 上验证
//! Windows 专用纯逻辑。

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::models::{CleanerProgress, InstalledApp, RunningState};

pub(crate) const QUIT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const QUIT_POLL: Duration = Duration::from_millis(250);

/// 一个「属于该应用」的进程。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub exe: Option<PathBuf>,
}

// ============================== 纯逻辑（可跨平台单测） ==============================

/// 把进程列表整理成对外状态：按 pid 去重、名字去重并保持 pid 顺序。
pub(crate) fn running_state_from(mut processes: Vec<RunningProcess>) -> RunningState {
    processes.sort_by_key(|process| process.pid);
    processes.dedup_by_key(|process| process.pid);
    let mut names: Vec<String> = Vec::new();
    for process in &processes {
        let name = process.name.trim();
        if !name.is_empty() && !names.iter().any(|seen| seen == name) {
            names.push(name.to_string());
        }
    }
    RunningState { running: !processes.is_empty(), processes: processes.len(), names }
}

/// 路径是否严格位于 root 之下（组件级比较，root 自身不算）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn path_within(path: &Path, root: &Path) -> bool {
    path != root && path.starts_with(root)
}

/// Windows 路径归一化：去掉引号与命令行尾巴、统一反斜杠、小写、去尾随分隔符。
///
/// 输入可能来自注册表（`"C:\\Program Files\\Foo" /s`）或进程 exe，
/// 输出用于大小写不敏感的前缀比较。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn normalize_win_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // 带引号的命令：只取第一对引号里的内容，忽略后面的参数
    let text = if let Some(rest) = raw.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else {
        raw
    };
    let mut text = text.trim().replace('/', "\\").to_ascii_lowercase();
    while text.ends_with('\\') && text.len() > 3 {
        text.pop();
    }
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// Windows 路径是否严格位于 root 之下（大小写不敏感、目录边界安全）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn win_path_within(exe: &str, root: &str) -> bool {
    exe == root || exe.starts_with(&format!("{root}\\"))
}

/// 安装根是否可用来判定「进程属于该应用」。
///
/// 拒绝空串、盘符根（`c:`）和公共父目录（Program Files / AppData / ProgramData /
/// 用户主目录等）—— 否则会把公共目录下所有进程都算成该应用。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_usable_install_root(normalized: &str, public_roots: &[String]) -> bool {
    if normalized.is_empty() || normalized.len() <= 2 {
        return false;
    }
    if !normalized.contains('\\') {
        return false;
    }
    if public_roots.iter().any(|root| root == normalized) {
        return false;
    }
    normalized.split('\\').filter(|segment| !segment.is_empty()).count() >= 2
}

/// 轮询直到进程清零；返回 true = 已全部退出。
pub(crate) fn wait_until_gone<F: FnMut() -> usize>(remaining: F) -> bool {
    wait_until_gone_with_limits(remaining, QUIT_TIMEOUT, QUIT_POLL)
}

/// 与 `wait_until_gone` 相同，但超时/间隔可注入（单测用短时长，避免真等 5 秒）。
pub(crate) fn wait_until_gone_with_limits<F: FnMut() -> usize>(
    mut remaining: F,
    timeout: Duration,
    poll: Duration,
) -> bool {
    let mut waited = Duration::ZERO;
    loop {
        if remaining() == 0 {
            return true;
        }
        if waited >= timeout {
            return false;
        }
        std::thread::sleep(poll);
        waited += poll;
    }
}

/// 检测应用是否仍在运行（只读）。
pub fn detect(app: &InstalledApp) -> RunningState {
    running_state_from(platform_detect(app))
}

/// 先优雅退出应用，再轮询到进程清零；退不干净返回错误（由调用方转成失败结果）。
pub fn quit(app: &InstalledApp, emit: &dyn Fn(CleanerProgress)) -> Result<(), String> {
    let processes = platform_detect(app);
    if processes.is_empty() {
        return Ok(());
    }
    emit(CleanerProgress::new(
        "uninstall",
        "waiting",
        0,
        1,
        format!("正在退出「{}」（{} 个进程）…", app.name, processes.len()),
    ));
    platform_terminate(app, &processes);
    if wait_until_gone(|| platform_detect(app).len()) {
        return Ok(());
    }
    let remaining = platform_detect(app).len();
    Err(format!(
        "「{}」仍有 {} 个进程未退出（可能正在等待保存），请手动退出后重试；本次未执行卸载",
        app.name, remaining
    ))
}

/// 枚举进程（pid / 显示名 / exe）。exe 读不到时保留 None，由平台规则决定是否使用。
fn sysinfo_processes() -> Vec<RunningProcess> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let Ok(mut system) = System::new() else {
        return Vec::new();
    };
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    system
        .processes()
        .iter()
        .map(|(pid, process)| RunningProcess {
            pid: pid.as_u32(),
            name: process.name().to_string_lossy().to_string(),
            exe: process.exe().map(Path::to_path_buf),
        })
        .collect()
}

// ============================== macOS 平台 ==============================

#[cfg(target_os = "macos")]
fn mac_bundle_path(app: &InstalledApp) -> Option<PathBuf> {
    app.id.strip_prefix("mac:").map(PathBuf::from)
}

/// macOS 检测：sysinfo 里 exe 在 `.app` 内的进程（覆盖 helper / XPC / 扩展）
/// + `NSRunningApplication` 按 bundle id 找到的主应用，按 pid 去重。
#[cfg(target_os = "macos")]
fn platform_detect(app: &InstalledApp) -> Vec<RunningProcess> {
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
    if let Some(info) = super::installer_macos::read_bundle_info(&bundle) {
        if let Some(bundle_id) = info.bundle_id {
            let ns_id = NSString::from_str(&bundle_id);
            let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns_id);
            for application in running.iter() {
                let pid = application.processIdentifier() as u32;
                let name = application
                    .localizedName()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| app.name.clone());
                found.entry(pid).or_insert(RunningProcess { pid, name, exe: None });
            }
        }
    }
    found.into_values().collect()
}

/// macOS 退出：优先按 bundle id 拿 `NSRunningApplication`，再按 pid 补 helper 实例，
/// 逐个 `terminate()`（等价于点 Dock 菜单里的「退出」，应用可以弹保存确认框）。
#[cfg(target_os = "macos")]
fn platform_terminate(app: &InstalledApp, processes: &[RunningProcess]) {
    use objc2::rc::Retained;
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    use std::collections::HashMap;

    let mut handles: HashMap<libc::pid_t, Retained<NSRunningApplication>> = HashMap::new();
    if let Some(bundle) = mac_bundle_path(app) {
        if let Some(info) = super::installer_macos::read_bundle_info(&bundle) {
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
        if let Some(application) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            handles.insert(pid, application);
        }
    }
    for application in handles.values() {
        let _ = application.terminate();
    }
}

// ============================== Windows 平台 ==============================

/// Windows 公共父目录：安装位置写成这些目录时不能用来判定进程归属。
#[cfg(target_os = "windows")]
fn public_roots() -> Vec<String> {
    [
        "PROGRAMFILES",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "PROGRAMDATA",
        "APPDATA",
        "LOCALAPPDATA",
        "USERPROFILE",
        "SystemDrive",
    ]
    .iter()
    .filter_map(|name| std::env::var(name).ok())
    .filter_map(|value| normalize_win_path(&value))
    .collect()
}

/// 安装根候选：缓存里的 InstallLocation + 注册表里的 InstallLocation / DisplayIcon 目录。
#[cfg(target_os = "windows")]
fn install_roots(app: &InstalledApp) -> Vec<String> {
    let public = public_roots();
    let mut roots: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let Some(root) = normalize_win_path(raw) else {
            return;
        };
        if is_usable_install_root(&root, &public) && !roots.contains(&root) {
            roots.push(root);
        }
    };
    push(&app.install_location);
    for candidate in super::installer_windows::install_root_candidates(&app.id) {
        push(&candidate.to_string_lossy());
    }
    roots
}

/// Windows 检测：exe 在安装根内的进程都算该应用（主程序 + 辅助进程）。
#[cfg(target_os = "windows")]
fn platform_detect(app: &InstalledApp) -> Vec<RunningProcess> {
    let roots = install_roots(app);
    if roots.is_empty() {
        return Vec::new();
    }
    sysinfo_processes()
        .into_iter()
        .filter(|process| {
            let Some(exe) = process.exe.as_ref() else {
                return false;
            };
            let Some(exe) = normalize_win_path(&exe.to_string_lossy()) else {
                return false;
            };
            roots.iter().any(|root| win_path_within(&exe, root))
        })
        .collect()
}

/// Windows 退出：对主窗口发 `CloseMainWindow()`（等价于点窗口的关闭按钮），
/// 不强制 `Stop-Process`；没有主窗口的进程会留给超时判定，由卸载流程中止。
#[cfg(target_os = "windows")]
fn platform_terminate(_app: &InstalledApp, processes: &[RunningProcess]) {
    let pids = processes
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "$pids = @({pids}); foreach ($p in $pids) {{ \
         $proc = Get-Process -Id $p -ErrorAction SilentlyContinue; \
         if ($proc) {{ $null = $proc.CloseMainWindow() }} }}"
    );
    let _ = crate::winps::run_with_timeout_ms(&script, std::time::Duration::from_secs(15));
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_detect(_app: &InstalledApp) -> Vec<RunningProcess> {
    Vec::new()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_terminate(_app: &InstalledApp, _processes: &[RunningProcess]) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, exe: Option<&str>) -> RunningProcess {
        RunningProcess { pid, name: name.into(), exe: exe.map(PathBuf::from) }
    }

    #[test]
    fn state_dedupes_pids_and_names() {
        let state = running_state_from(vec![
            proc(9, "Foo", None),
            proc(9, "Foo", None),
            proc(3, "Foo Helper", None),
        ]);
        assert!(state.running);
        assert_eq!(state.processes, 2);
        assert_eq!(state.names, vec!["Foo Helper".to_string(), "Foo".to_string()]);
    }

    #[test]
    fn empty_processes_means_not_running() {
        let state = running_state_from(Vec::new());
        assert!(!state.running);
        assert_eq!(state.processes, 0);
        assert!(state.names.is_empty());
    }

    #[test]
    fn path_within_is_component_aware() {
        assert!(path_within(
            Path::new("/Applications/Foo.app/Contents/MacOS/foo"),
            Path::new("/Applications/Foo.app")
        ));
        assert!(!path_within(
            Path::new("/Applications/Foo.app"),
            Path::new("/Applications/Foo.app")
        ));
        assert!(!path_within(
            Path::new("/Applications/Foobar.app/x"),
            Path::new("/Applications/Foo.app")
        ));
    }

    #[test]
    fn windows_path_within_is_case_insensitive_and_bounded() {
        let root = normalize_win_path(r#""C:\Program Files\Foo" /s"#).unwrap();
        assert_eq!(root, r"c:\program files\foo");
        // 带引号/斜杠/尾随空格的写法会被归一化后再比较
        assert!(win_path_within(r"c:\program files\foo\bar.exe", &root));
        assert!(!win_path_within(r"c:\program files\foobar\x.exe", &root));
    }

    #[test]
    fn wait_until_gone_times_out() {
        let mut calls = 0;
        let gone = wait_until_gone_with_limits(
            || {
                calls += 1;
                2
            },
            Duration::from_millis(30),
            Duration::from_millis(10),
        );
        assert!(!gone);
        assert!(calls >= 2);
    }

    #[test]
    fn wait_until_gone_returns_when_clear() {
        let mut calls = 0;
        let gone = wait_until_gone_with_limits(
            || {
                calls += 1;
                if calls >= 2 {
                    0
                } else {
                    1
                }
            },
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert!(gone);
    }

    #[test]
    fn install_root_rejects_public_dirs() {
        let public = vec![
            r"c:\program files".to_string(),
            r"c:\program files (x86)".to_string(),
            r"c:\programdata".to_string(),
            r"c:\users\me\appdata\local".to_string(),
            r"c:\users\me".to_string(),
        ];
        assert!(!is_usable_install_root("c:", &public));
        assert!(!is_usable_install_root(r"c:\program files", &public));
        assert!(!is_usable_install_root(r"c:\users\me", &public));
        assert!(is_usable_install_root(r"c:\program files\foo", &public));
        assert!(is_usable_install_root(
            r"c:\users\me\appdata\local\programs\foo",
            &public
        ));
    }
}
