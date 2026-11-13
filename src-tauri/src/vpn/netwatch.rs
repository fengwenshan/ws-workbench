//! 网络出口变更通知（换 Wi-Fi、拔网线、改 IP、连上热点）。
//!
//! 为什么需要这一层：分流布下的路由是按「当时的默认网关 + 接口索引」算出来的，
//! openconnect 的 TCP 连接也绑在当时的物理地址上。换网之后两边都不会自愈 ——
//! openconnect 空闲时察觉不到链路已断，隧道网卡还在，界面依旧显示「已连接」，
//! 但内网一个都打不开（现场：切到另一个无线网，状态一直显示已连接，域名全解析不了）。
//!
//! 做法：用 `NotifyUnicastIpAddressChange` 注册一个系统回调（事件驱动，不轮询
//! PowerShell，也不占线程）。回调里只把代号 +1 —— 它跑在系统线程上，不能做重活；
//! worker 主循环发现代号变了，再去做代价高的快照比对（要起 PowerShell）。
//!
//! 隧道网卡自己上下线也会触发回调，所以调用方不能拿「代号变了」当结论，
//! 必须再比一次物理默认出口（见 `worker::Worker::network_exit_changed`）。

use std::sync::atomic::{AtomicU64, Ordering};

/// 单调递增的变更代号。只在 Windows 上有真实值，其它平台恒为 0。
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 变更通知句柄。
///
/// 注册后不必反注册：它的生命周期就是进程的剩余生命周期，进程退出时系统回收。
/// 保留这个名字是为了将来真需要 `CancelMibChangeNotify2` 时有地方拿句柄。
pub struct Watcher {
    _private: (),
}

impl Watcher {
    /// 当前代号。变了就代表「至少有一次地址变更事件」，不代表出口一定变了。
    pub fn generation(&self) -> u64 {
        current_generation()
    }
}

#[cfg(target_os = "windows")]
fn current_generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

#[cfg(not(target_os = "windows"))]
fn current_generation() -> u64 {
    0
}

/// 注册变更通知。失败时返回 `None`：调用方当作「这个平台没有变更通知」，
/// 保留原来的 15 秒路由守护 —— 少一层自愈，但功能不因此崩溃。
#[cfg(target_os = "windows")]
pub fn spawn() -> Option<Watcher> {
    use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
    use windows::Win32::NetworkManagement::IpHelper::{
        NotifyUnicastIpAddressChange, MIB_UNICASTIPADDRESS_ROW, MibInitialNotification,
        MIB_NOTIFICATION_TYPE,
    };
    use windows::Win32::Networking::WinSock::AF_UNSPEC;

    /// 注册时的第一次回调（`MibInitialNotification`）不是真变化，要跳过；
    /// 其余事件（新增 / 删除地址、参数变化）都当作「可能换网了」。
    unsafe extern "system" fn on_address_change(
        _context: *const core::ffi::c_void,
        _row: *const MIB_UNICASTIPADDRESS_ROW,
        kind: MIB_NOTIFICATION_TYPE,
    ) {
        if kind == MibInitialNotification {
            return;
        }
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }

    let mut handle = HANDLE::default();
    let status = unsafe {
        NotifyUnicastIpAddressChange(
            AF_UNSPEC,
            Some(on_address_change),
            None,
            false,
            &mut handle,
        )
    };
    if status != ERROR_SUCCESS {
        eprintln!("[网络] 注册地址变更通知失败: {:?}", status);
        return None;
    }
    Some(Watcher { _private: () })
}

#[cfg(not(target_os = "windows"))]
pub fn spawn() -> Option<Watcher> {
    None
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn no_watcher_on_this_platform() {
        assert!(spawn().is_none(), "非 Windows 平台没有变更通知");
    }
}
