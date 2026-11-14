//! 网络出口变更通知（换 Wi-Fi、拔网线、改 IP、连上热点）。
//!
//! 为什么需要这一层：分流布下的路由是按「当时的默认网关 + 接口索引」算出来的，
//! openconnect 的 TCP 连接也绑在当时的物理地址上。换网之后两边都不会自愈 ——
//! openconnect 空闲时察觉不到链路已断，隧道网卡还在，界面依旧显示「已连接」，
//! 但内网一个都打不开（现场：切到另一个无线网，状态一直显示已连接，域名全解析不了）。
//!
//! 做法分平台：
//! - **Windows**：用 `NotifyUnicastIpAddressChange` 注册系统回调（事件驱动，不轮询、
//!   不占线程）。回调里只把代号 +1 —— 它跑在系统线程上，不能做重活。
//! - **macOS**：没有等价的轻量回调（SCDynamicStore 要额外 FFI），改为每 2 秒
//!   `getifaddrs` 取一次「接口名 + IPv4 地址」指纹，变了就把代号 +1。只是系统调用，
//!   不起子进程，代价可以忽略；换来的是换 Wi-Fi / 拔网线后 worker 能自动重连。
//!
//! worker 主循环发现代号变了，再去做代价高的快照比对（macOS 要起 `route` / `ifconfig`）。
//!
//! 隧道网卡自己上下线也会让指纹变化，所以调用方不能拿「代号变了」当结论，
//! 必须再比一次物理默认出口（见 `worker::network_exit_changed`）。

// 代号由 Windows 回调 / macOS 轮询线程读写；其它平台恒为 0，
// 不加 cfg 会在这些平台留下 unused import / dead_code 警告。
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::sync::atomic::{AtomicU64, Ordering};

/// 单调递增的变更代号。Windows / macOS 有真实值，其它平台恒为 0。
#[cfg(any(target_os = "windows", target_os = "macos"))]
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 变更通知句柄。
///
/// 注册后不必反注册：它的生命周期就是进程的剩余生命周期，进程退出时系统回收。
/// Windows 上保留这个名字是为了将来真需要 `CancelMibChangeNotify2` 时有地方拿句柄；
/// macOS 的轮询线程随进程退出，不需要回收。
pub struct Watcher {
    _private: (),
}

impl Watcher {
    /// 当前代号。变了就代表「至少有一次地址变更事件」，不代表出口一定变了。
    pub fn generation(&self) -> u64 {
        current_generation()
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn current_generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
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

/// macOS 上多久取一次接口地址指纹。
///
/// 换 Wi-Fi 时至少要等系统把新地址配上；2 秒的检测延迟再加上 worker 侧 1.2 秒的
/// 事件聚合窗口，用户看到的是「网络已切换，正在重连」。轮询频率低到不影响功耗。
#[cfg(target_os = "macos")]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// macOS 的变更通知：后台线程轮询 `getifaddrs` 的「接口名 + IPv4 地址」指纹。
///
/// 为什么不用 SCDynamicStore：要多引一层 SystemConfiguration 的 FFI，收益只是把
/// 2 秒延迟变成事件驱动；而轮询本身是纯系统调用，不起子进程、不碰 PowerShell，
/// 对提权 worker 的负担可以忽略。线程随进程退出，不需要显式回收。
#[cfg(target_os = "macos")]
pub fn spawn() -> Option<Watcher> {
    std::thread::Builder::new()
        .name("vpn-netwatch".to_string())
        .spawn(|| {
            let mut last = macos_interface_fingerprint();
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let current = macos_interface_fingerprint();
                if current != last {
                    last = current;
                    GENERATION.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
        .ok()
        .map(|_| Watcher { _private: () })
}

/// 当前所有网卡的「接口名|IPv4」指纹（排序后用逗号拼起来）。
///
/// 只取 IPv4：分流和默认出口都按 IPv4 工作。回环 / 隧道网卡也算在内 ——
/// 隧道上下线会让指纹变化，worker 拿到代号后会再比物理默认出口把这类噪音过滤掉。
#[cfg(target_os = "macos")]
fn macos_interface_fingerprint() -> String {
    use std::ffi::CStr;
    use std::net::Ipv4Addr;

    let mut entries: Vec<String> = Vec::new();
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return String::new();
        }
        let mut cursor = list;
        while !cursor.is_null() {
            let item = &*cursor;
            if !item.ifa_addr.is_null() && (*item.ifa_addr).sa_family as i32 == libc::AF_INET {
                let addr = &*(item.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
                let name = if item.ifa_name.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(item.ifa_name).to_string_lossy().into_owned()
                };
                entries.push(format!("{name}|{ip}"));
            }
            cursor = item.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    entries.sort();
    entries.join(",")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn spawn() -> Option<Watcher> {
    None
}

#[cfg(all(test, not(any(target_os = "windows", target_os = "macos"))))]
mod no_watcher_tests {
    use super::*;

    #[test]
    fn no_watcher_on_this_platform() {
        assert!(
            spawn().is_none(),
            "既没有 Windows 地址变更回调、也没有 macOS 轮询的平台没有变更通知"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn watcher_is_available_on_macos() {
        assert!(spawn().is_some(), "macOS 用轮询实现，必须返回 watcher");
    }

    #[test]
    fn interface_fingerprint_includes_loopback() {
        let fingerprint = macos_interface_fingerprint();
        assert!(
            fingerprint.contains("lo0|127.0.0.1"),
            "指纹里应该至少包含回环地址：{fingerprint}"
        );
    }
}
