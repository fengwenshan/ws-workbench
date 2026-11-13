//! 关机 / 重启前先把 VPN 断开。
//!
//! 现场问题：VPN 连着的时候直接关机，隧道、`/32` 路由、NRPT 规则、本机转发线程
//! 都不会走「断开」流程 —— 进程被系统结束，`cleanup` 那段代码根本没机会执行。
//! 于是下一次开机要么残留一条指向不存在网卡的 NRPT（内网域名全解析不了），
//! 要么 openconnect 的子进程还在跑。
//!
//! 做法（Windows）：给主窗口装一个子类过程（`SetWindowSubclass`），收到
//! `WM_QUERYENDSESSION` 时**先同步断开、再放行**：
//!
//! 1. 往会话目录写 `stop.flag`（worker 每 200ms 看一次）；
//! 2. 最多等 [`SHUTDOWN_BUDGET_MS`] 让状态落到 `Disconnected`；
//! 3. 返回 `TRUE`，让系统继续关机。
//!
//! 为什么是「等一小会儿再放行」而不是「返回 FALSE 拦住关机」：
//! 返回 `FALSE` 会把用户这次关机**整个取消**，必须由我们自己重新发起关机；
//! 而 `WM_QUERYENDSESSION` 的 `lParam` 只带「注销 / 临界关机」，**区分不出**
//! 用户点的是关机、重启还是休眠 —— 重新发起时选错动作（把重启变成关机）比
//! 少拦几秒更糟。所以这里用「限时收尾 + 放行」，效果是开机后不留脏状态、
//! 隧道进程不残留，而用户点的关机和重启都按原样执行。
//!
//! 兜底：即便这几秒没清完，下次启动时 `split::cleanup_stale` /
//! `fulltunnel::restore_stale` 还会再扫一遍，所以这里**不追求百分百**。

use std::sync::atomic::{AtomicBool, Ordering};

/// 已经进入关机收尾。状态轮询线程据此不再把「断开」当成用户主动操作去提示。
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// 留给 worker 收尾的时间。系统对 `WM_QUERYENDSESSION` 的处理有超时限制，
/// 而一次完整清理要跑几次 PowerShell（单次 0.3~1s），2.5s 是能覆盖
/// 「撤路由 + 还原 DNS」又不至于被系统判定为无响应的折中值。
#[cfg(target_os = "windows")]
const SHUTDOWN_BUDGET_MS: u64 = 2500;

/// 是否正在关机收尾（供状态轮询 / 前端提示使用）。
pub fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Relaxed)
}

/// 装上关机拦截。非 Windows 平台是空实现。
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

/// 关机前的收尾：写停止请求，限时等 worker 把布线撤干净。
#[cfg(target_os = "windows")]
fn before_session_end(lparam: windows::Win32::Foundation::LPARAM) {
    use std::time::{Duration, Instant};

    use tauri::Manager;

    use crate::vpn::commands::VpnState;
    use crate::vpn::models::VpnPhase;
    use crate::vpn::session::VpnSession;

    // 已经收过一轮就别再来一遍：关机流程可能对同一个窗口发多次询问
    if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
        return;
    }

    let Some(app) = APP.get() else { return };
    let Some(state) = app.try_state::<VpnState>() else {
        return;
    };
    // 只把目录复制出来就用，别在等待期间一直攥着锁：状态轮询线程也在抢它
    let session = {
        let Ok(guard) = state.0.lock() else {
            return;
        };
        VpnSession::new(guard.session.dir.clone())
    };

    let phase = session
        .read_status()
        .map(|status| status.phase)
        .unwrap_or(VpnPhase::Disconnected);
    if !matches!(
        phase,
        VpnPhase::Preparing
            | VpnPhase::Elevating
            | VpnPhase::Connecting
            | VpnPhase::ConfiguringRoutes
            | VpnPhase::Connected
            | VpnPhase::Disconnecting
    ) {
        return;
    }

    // lParam 的 bit0 = 注销而不是关机，只影响日志措辞
    let kind = if lparam.0 & 0x1 != 0 { "注销" } else { "关机/重启" };
    session.append_log(&format!(
        "[关机] 系统请求{}，先断开 VPN（阶段 {}）再放行",
        kind,
        phase.as_str()
    ));

    session.request_stop();
    let deadline = Instant::now() + Duration::from_millis(SHUTDOWN_BUDGET_MS);
    let mut cleaned = false;
    while Instant::now() < deadline {
        let now_disconnected = session
            .read_status()
            .map(|status| status.phase == VpnPhase::Disconnected)
            .unwrap_or(true);
        if now_disconnected {
            cleaned = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }

    session.append_log(if cleaned {
        "[关机] VPN 已断开，放行关机"
    } else {
        "[关机] 等待断开超时，先把关机放行；残留的 NRPT / 路由由下次启动时清理"
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_flag_starts_clear() {
        assert!(!is_shutting_down());
    }
}
