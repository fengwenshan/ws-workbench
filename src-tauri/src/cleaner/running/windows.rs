//! Windows 运行状态检测、优雅退出与强杀实现。
//!
//! 平台实现文件：只在 Windows target 编译，禁止引用 `running::macos`。

use super::super::models::InstalledApp;
use super::{
    is_usable_install_root, normalize_win_path, sysinfo_processes, win_path_within, RunningProcess,
};

/// Windows 公共父目录：安装位置写成这些目录时不能用来判定进程归属。
pub(super) fn public_roots() -> Vec<String> {
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
pub(super) fn install_roots(app: &InstalledApp) -> Vec<String> {
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
    for candidate in crate::cleaner::installer_windows::install_root_candidates(&app.id) {
        push(&candidate.to_string_lossy());
    }
    roots
}

/// Windows 检测：exe 在安装根内的进程都算该应用（主程序 + 辅助进程）。
pub(super) fn detect(app: &InstalledApp) -> Vec<RunningProcess> {
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
pub(super) fn terminate(_app: &InstalledApp, processes: &[RunningProcess]) {
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

/// Windows 强杀：`taskkill /PID <pid> /T /F`（含子进程树）。
pub(super) fn kill(processes: &[RunningProcess]) {
    for process in processes {
        let pid = process.pid.to_string();
        let _ = crate::process::hidden("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .output();
    }
}
