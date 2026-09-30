#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// `--vpn-worker <会话目录>`：以管理员权限运行的 VPN worker。
///
/// 由 Windows 计划任务或 macOS 的 osascript 拉起，和 GUI 是两个进程，
/// 因此这里必须在启动 Tauri 之前就拦下来。
const VPN_WORKER_FLAG: &str = "--vpn-worker";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if let Some(index) = args.iter().position(|arg| arg == VPN_WORKER_FLAG) {
        let dir = args.get(index + 1).cloned().unwrap_or_default();
        if dir.is_empty() {
            eprintln!("[vpn-worker] 缺少会话目录参数");
            std::process::exit(ws_tools_lib::vpn::worker::EXIT_NO_REQUEST);
        }
        let code = ws_tools_lib::vpn::worker::run(std::path::Path::new(&dir));
        std::process::exit(code);
    }

    ws_tools_lib::run();
}
