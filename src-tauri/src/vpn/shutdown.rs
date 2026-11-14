//! 关机 / 重启 / 退出前先把 VPN 断开。
//!
//! 现场问题：VPN 连着的时候直接关机，隧道、`/32` 路由、解析器配置（Windows 的
//! NRPT、macOS 的 `/etc/resolver`）、本机转发线程都不会走「断开」流程 —— 进程被
//! 系统结束，`cleanup` 那段代码根本没机会执行。于是下一次开机要么残留一条指向
//! 不存在网卡的规则（内网域名全解析不了），要么 openconnect 的子进程还在跑。
//!
//! 做法是「先同步断开、再放行」。两个平台各有一个位于系统终止流程里的同步点：
//!
//! - **Windows**：给主窗口装子类过程（`SetWindowSubclass`），收到
//!   `WM_QUERYENDSESSION` 时先同步断开再返回 `TRUE` 放行（见 [`install`]）；
//! - **macOS**：系统关机 / 注销 / Cmd+Q 最终都会走
//!   `applicationWillTerminate → LoopDestroyed → RunEvent::Exit`，在
//!   [`before_exit`] 里同步等 worker 撤完布线。loginwindow 会等本进程退出，
//!   所以这段等待等价于「先断开再关机」，不需要额外的关机拦截器。
//!
//! 两个平台都只等 [`SHUTDOWN_BUDGET_MS`]，超时就放行：
//!
//! 1. 往会话目录写 `stop.flag`（worker 每 200ms 看一次）；
//! 2. 最多等 `SHUTDOWN_BUDGET_MS` 让状态落到 `Disconnected`（或本来就是断开/失败）；
//! 3. 让系统继续关机。
//!
//! Windows 上为什么是「等一小会儿再放行」而不是「返回 FALSE 拦住关机」：
//! 返回 `FALSE` 会把用户这次关机**整个取消**，必须由我们自己重新发起关机；
//! 而 `WM_QUERYENDSESSION` 的 `lParam` 只带「注销 / 临界关机」，**区分不出**
//! 用户点的是关机、重启还是休眠 —— 重新发起时选错动作（把重启变成关机）比
//! 少拦几秒更糟。所以这里用「限时收尾 + 放行」，效果是开机后不留脏状态、
//! 隧道进程不残留，而用户点的关机和重启都按原样执行。
//!
//! 兜底：即便这几秒没清完，下次启动时 `split::cleanup_stale` /
//! `fulltunnel::restore_stale` 还会再扫一遍，所以这里**不追求百分百**。

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use crate::vpn::models::VpnPhase;

/// 已经进入关机收尾。状态轮询线程据此不再把「断开」当成用户主动操作去提示。
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// 留给 worker 收尾的时间。
///
/// 系统对终止流程的处理有超时限制（Windows 的 `WM_QUERYENDSESSION`、macOS 的
/// loginwindow 都不会无限等一个应用退出），而一次完整清理要跑几次 PowerShell /
/// `route`（单次 0.3~1s），2.5s 是能覆盖「撤路由 + 还原 DNS」又不至于被系统
/// 判定为无响应的折中值。
#[cfg(any(target_os = "windows", target_os = "macos"))]
const SHUTDOWN_BUDGET_MS: u64 = 2500;

/// 是否正在关机收尾（供状态轮询 / 前端提示使用）。
pub fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Relaxed)
}

/// 装上关机拦截。
///
/// 只有 Windows 需要在 setup 里装东西（子类化主窗口）。macOS 的收尾由 lib.rs 的
/// `RunEvent::Exit` 调 [`before_exit`] 完成，这里保持空实现。
#[cfg(not(target_os = "windows"))]
pub fn install(_app: &tauri::AppHandle) {}

#[cfg(target_os = "windows")]
pub fn install(app: &tauri::AppHandle) {
    use tauri::Manager;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::SetWindowSubclass;

    let Some(window) = app.get_webview_window("main") else {
        eprintln!("[关机] 主窗口还没建好，跳过关机拦截");
        return;
    };
    let hwnd = match window.hwnd() {
        Ok(hwnd) => HWND(hwnd.0),
        Err(err) => {
            eprintln!("[关机] 取主窗口句柄失败，跳过关机拦截: {err}");
            return;
        }
    };
    if APP.set(app.clone()).is_err() {
        // 已经装过：窗口重建时不要重复子类化，否则一次关机事件会触发两遍断开
        return;
    }
    let ok = unsafe { SetWindowSubclass(hwnd, Some(on_window_message), SUBCLASS_ID, 0) };
    if !ok.as_bool() {
        eprintln!("[关机] SetWindowSubclass 失败，本次运行没有关机前收尾");
    }
}

#[cfg(target_os = "windows")]
static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// 子类化 ID，随便取一个本程序内唯一的数。
#[cfg(target_os = "windows")]
const SUBCLASS_ID: usize = 0x5753_5642; // "WSVB"

/// `WM_QUERYENDSESSION`：系统要关机 / 重启 / 注销，问这个窗口同不同意。
#[cfg(target_os = "windows")]
const WM_QUERYENDSESSION: u32 = 0x0011;

#[cfg(target_os = "windows")]
unsafe extern "system" fn on_window_message(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    if message == WM_QUERYENDSESSION {
        before_session_end(lparam);
        // TRUE = 同意关机。收尾已经在上面同步做完了。
        return windows::Win32::Foundation::LRESULT(1);
    }
    windows::Win32::UI::Shell::DefSubclassProc(hwnd, message, wparam, lparam)
}

/// 退出/关机前是否需要先断开：只有活动阶段需要等。
///
/// `Disconnecting` 也算活动阶段 —— 这时 worker 正在撤路由，等它收完比
/// 直接让进程退出更安全。`Disconnected` / `Failed` 没有要回收的东西。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn phase_needs_disconnect(phase: VpnPhase) -> bool {
    matches!(
        phase,
        VpnPhase::Preparing
            | VpnPhase::Elevating
            | VpnPhase::Connecting
            | VpnPhase::ConfiguringRoutes
            | VpnPhase::Connected
            | VpnPhase::Disconnecting
    )
}

/// 从主进程状态里取当前会话目录（worker 与主进程共用这个目录）。
///
/// 只把目录复制出来就用，别在等待期间一直攥着锁：状态轮询线程也在抢它。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn current_session(app: &tauri::AppHandle) -> Option<crate::vpn::session::VpnSession> {
    use tauri::Manager;

    let state = app.try_state::<crate::vpn::commands::VpnState>()?;
    let guard = state.0.lock().ok()?;
    Some(crate::vpn::session::VpnSession::new(
        guard.session.dir.clone(),
    ))
}

/// 限时断开：写停止请求，最多等 [`SHUTDOWN_BUDGET_MS`] 让 worker 把布线撤干净。
///
/// 返回 `true` 表示已确认断开（或本来就没有活动连接）。日志写进会话目录的
/// worker.log，用户下次连上后能在日志里看到这次关机做了什么。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn disconnect_before_exit(app: &tauri::AppHandle, kind: &str) -> bool {
    let Some(session) = current_session(app) else {
        return true;
    };
    disconnect_session_with_budget(&session, kind, Duration::from_millis(SHUTDOWN_BUDGET_MS))
}

/// [`disconnect_before_exit`] 的会话级实现；预算可注入，单测用短预算跑超时分支。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn disconnect_session_with_budget(
    session: &crate::vpn::session::VpnSession,
    kind: &str,
    budget: Duration,
) -> bool {
    let phase = session
        .read_status()
        .map(|status| status.phase)
        .unwrap_or(VpnPhase::Disconnected);
    if !phase_needs_disconnect(phase) {
        session.append_log(&format!("[关机] 系统请求{kind}，VPN 未连接，直接放行"));
        return true;
    }

    session.append_log(&format!(
        "[关机] 系统请求{}，先断开 VPN（阶段 {}）再放行",
        kind,
        phase.as_str()
    ));

    session.request_stop();
    let deadline = Instant::now() + budget;
    let mut cleaned = false;
    while Instant::now() < deadline {
        let now = session
            .read_status()
            .map(|status| status.phase)
            .unwrap_or(VpnPhase::Disconnected);
        if !phase_needs_disconnect(now) {
            cleaned = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }

    session.append_log(if cleaned {
        "[关机] VPN 已断开，放行关机"
    } else {
        "[关机] 等待断开超时，先把退出/关机放行；残留的路由/解析器配置由下次启动时清理"
    });
    cleaned
}

/// macOS：系统关机 / 注销 / Cmd+Q 的同步收尾点。
///
/// 由 lib.rs 在 `tauri::RunEvent::Exit` 时调用。此时 tao 的
/// `applicationWillTerminate` 正在执行、进程还没退出；在这里等 worker 断开，
/// loginwindow 会一直等到本进程退出 —— 效果就是「先断开再关机」，和 Windows
/// 的 `WM_QUERYENDSESSION` 收尾同一条路线。
#[cfg(target_os = "macos")]
pub fn before_exit(app: &tauri::AppHandle) {
    // 关机流程可能对同一个进程发多次终止请求，只收一轮
    if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
        return;
    }
    disconnect_before_exit(app, "退出/关机");
}

/// 关机前的收尾：写停止请求，限时等 worker 把布线撤干净。
#[cfg(target_os = "windows")]
fn before_session_end(lparam: windows::Win32::Foundation::LPARAM) {
    // 已经收过一轮就别再来一遍：关机流程可能对同一个窗口发多次询问
    if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(app) = APP.get() else { return };
    // lParam 的 bit0 = 注销而不是关机，只影响日志措辞
    let kind = if lparam.0 & 0x1 != 0 {
        "注销"
    } else {
        "关机/重启"
    };
    disconnect_before_exit(app, kind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_flag_starts_clear() {
        assert!(!is_shutting_down());
    }

    /// 退出前置判断：只有活动阶段需要等断开，已经断开/失败的不再等。
    #[test]
    fn only_active_phases_need_disconnect_before_exit() {
        use crate::vpn::models::VpnPhase;
        for phase in [
            VpnPhase::Preparing,
            VpnPhase::Elevating,
            VpnPhase::Connecting,
            VpnPhase::ConfiguringRoutes,
            VpnPhase::Connected,
            VpnPhase::Disconnecting,
        ] {
            assert!(phase_needs_disconnect(phase), "{phase:?} 属于活动状态");
        }
        for phase in [VpnPhase::Disconnected, VpnPhase::Failed] {
            assert!(!phase_needs_disconnect(phase), "{phase:?} 不需要再断");
        }
    }

    /// 造一个带状态文件的临时会话；`phase` 就是 worker 最后写下的阶段。
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn session_with_phase(
        dir: &std::path::Path,
        phase: crate::vpn::models::VpnPhase,
    ) -> crate::vpn::session::VpnSession {
        let session = crate::vpn::session::VpnSession::new(dir.to_path_buf());
        session.ensure().unwrap();
        let status = crate::vpn::models::VpnStatus {
            phase,
            ..Default::default()
        };
        session.write_status(&status).unwrap();
        session
    }

    /// 活动阶段：写 stop.flag，等 worker 把状态落到 Disconnected 再返回。
    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn disconnect_waits_for_worker_to_finish() {
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let phase = crate::vpn::models::VpnPhase::Connecting;
        let session = session_with_phase(dir.path(), phase);
        let worker = crate::vpn::session::VpnSession::new(dir.path().to_path_buf());
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let status = crate::vpn::models::VpnStatus {
                phase: crate::vpn::models::VpnPhase::Disconnected,
                ..Default::default()
            };
            let _ = worker.write_status(&status);
        });

        let started = Instant::now();
        let cleaned = disconnect_session_with_budget(&session, "测试", Duration::from_millis(1200));
        let elapsed = started.elapsed();
        handle.join().unwrap();

        assert!(cleaned, "worker 在预算内断开，应该报告已清理");
        assert!(
            elapsed >= Duration::from_millis(100),
            "要等 worker 真的断开，不能看一眼就放行：{elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(1100),
            "断开之后不该把剩余预算睡满：{elapsed:?}"
        );
        assert!(session.is_stop_requested(), "应该写 stop.flag");
        let log = std::fs::read_to_string(session.log_path()).unwrap_or_default();
        assert!(log.contains("先断开 VPN（阶段 connecting）"), "日志：{log}");
        assert!(log.contains("VPN 已断开，放行关机"), "日志：{log}");
    }

    /// worker 不响应：等满预算就放行，不能拖死关机。
    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn disconnect_gives_up_after_budget() {
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let session = session_with_phase(dir.path(), crate::vpn::models::VpnPhase::Connected);
        let started = Instant::now();
        let cleaned = disconnect_session_with_budget(&session, "测试", Duration::from_millis(200));
        let elapsed = started.elapsed();

        assert!(!cleaned, "worker 没响应就要超时放行");
        assert!(
            elapsed >= Duration::from_millis(150),
            "要等满预算：{elapsed:?}"
        );
        let log = std::fs::read_to_string(session.log_path()).unwrap_or_default();
        assert!(log.contains("等待断开超时"), "日志：{log}");
    }

    /// 本来就断开：不写 stop.flag、不等待，直接放行。
    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn disconnect_returns_immediately_when_already_disconnected() {
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let session = session_with_phase(dir.path(), crate::vpn::models::VpnPhase::Disconnected);
        let started = Instant::now();
        let cleaned = disconnect_session_with_budget(&session, "测试", Duration::from_millis(1200));

        assert!(cleaned);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "已经断开就不该再等"
        );
        let log = std::fs::read_to_string(session.log_path()).unwrap_or_default();
        assert!(log.contains("VPN 未连接，直接放行"), "日志：{log}");
    }
}
