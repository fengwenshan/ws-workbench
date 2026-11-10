#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// `--vpn-worker <会话目录>`：以管理员权限运行的 VPN worker。
///
/// 由 Windows 计划任务或 macOS 的 osascript 拉起，和 GUI 是两个进程，
/// 因此这里必须在启动 Tauri 之前就拦下来。
const VPN_WORKER_FLAG: &str = "--vpn-worker";

/// 调试构建是控制台子系统（为了在终端里看日志），提权拉起 worker 时 Windows
/// 会给它单独开一个控制台窗口；worker 要活到 VPN 断开，这个黑窗口就一直挂着。
/// worker 的日志本来就写会话目录，不需要控制台，启动后直接脱离即可。
/// 发布构建是 windows 子系统，本来就没有控制台，调用它是无害的空操作。
#[cfg(target_os = "windows")]
fn detach_worker_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
    }
    unsafe {
        FreeConsole();
    }
}

#[cfg(not(target_os = "windows"))]
fn detach_worker_console() {}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if let Some(index) = args.iter().position(|arg| arg == VPN_WORKER_FLAG) {
        let dir = args.get(index + 1).cloned().unwrap_or_default();
        if dir.is_empty() {
            eprintln!("[vpn-worker] 缺少会话目录参数");
            std::process::exit(ws_workbench_lib::vpn::worker::EXIT_NO_REQUEST);
        }
        detach_worker_console();
        let code = ws_workbench_lib::vpn::worker::run(std::path::Path::new(&dir));
        std::process::exit(code);
    }

    ws_workbench_lib::run();
}
