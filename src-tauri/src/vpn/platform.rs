//! 平台抽象：TUN 设备、路由表、接口绑定、进程反查。
//!
//! macOS 与 Windows 各有一套实现，`netstack`/`router` 只依赖这里的 trait。
//! 两边的差异集中在「出站 socket 怎么钉到指定网卡」：
//! - macOS：`IP_BOUND_IF`；
//! - Windows：`IP_UNICAST_IF` 钉 socket，VPN 流再补一条 `/32` 路由指向隧道
//!   （隧道网卡通常没有目标网段的路由，只绑 socket 会 `WSAENETUNREACH`）。

use std::net::{Ipv4Addr, SocketAddrV4};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::route;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowKey {
    pub transport: Transport,
    pub local: SocketAddrV4,
    pub remote: SocketAddrV4,
}

/// 把单个 IPv4 表示成 /32 的主机路由目标。
pub fn host_route_cidr(ip: &str) -> String {
    format!("{}/32", ip)
}

/// 把接口索引解析成接口名（macOS 的 route 命令只认名字）。
#[cfg(target_os = "macos")]
fn if_name_for_index(index: u32) -> Result<String, String> {
    let mut buf = [0i8; libc::IF_NAMESIZE];
    let ptr = unsafe { libc::if_indextoname(index as libc::c_uint, buf.as_mut_ptr()) };
    if ptr.is_null() {
        return Err(format!("找不到接口索引 {} 对应的接口名", index));
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Ok(cstr.to_string_lossy().to_string())
}

/// 应用侧连接终结后，出站 socket 钉哪张网卡。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    /// 直连：钉物理网卡
    Direct,
    /// 走隧道：钉隧道网卡（openconnect 建立的 utun / Wintun）
    Vpn,
}

/// TUN 设备的最小能力。读写的都是一个完整 IP 包。
pub trait TunDevice: Send {
    fn name(&self) -> &str;
    fn index(&self) -> u32;
    fn read_packet(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_packet(&mut self, buf: &[u8]) -> std::io::Result<usize>;
}

/// 让 `Box<dyn TunDevice>` 也能直接交给 `Netstack`，避免引擎为平台写两份泛型。
impl TunDevice for Box<dyn TunDevice> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn index(&self) -> u32 {
        (**self).index()
    }
    fn read_packet(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (**self).read_packet(buf)
    }
    fn write_packet(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        (**self).write_packet(buf)
    }
}

/// 路由表操作。macOS 走 `/sbin/route`，Windows 走 PowerShell / route.exe。
pub trait RouteTable: Send + Sync {
    fn add_flow_route(&self, ip: Ipv4Addr, if_index: u32) -> Result<(), String>;
    fn del_flow_route(&self, ip: Ipv4Addr) -> Result<(), String>;
    fn add_capture_routes(&self, if_index: u32) -> Result<(), String>;
    fn del_capture_routes(&self, if_index: u32) -> Result<(), String>;
    fn add_server_host_route(&self, ip: Ipv4Addr, gateway: &str) -> Result<(), String>;
    fn del_server_host_route(&self, ip: Ipv4Addr) -> Result<(), String>;
}

/// 把某个 fd 的出口钉到指定网卡。
pub trait IfBinder: Send + Sync {
    fn bind_fd_to_if(&self, fd: i32, if_index: u32) -> std::io::Result<()>;
}

/// 由五元组反查发起进程。
pub trait ProcLookup: Send {
    fn pid_for(&mut self, key: &FlowKey) -> Option<u32>;
    fn name_for_pid(&mut self, pid: u32) -> Option<String>;
}

#[cfg(target_os = "macos")]
pub struct MacRouteTable;

#[cfg(target_os = "macos")]
impl MacRouteTable {
    pub fn new() -> Self {
        MacRouteTable
    }
}

#[cfg(target_os = "macos")]
impl RouteTable for MacRouteTable {
    fn add_flow_route(&self, ip: Ipv4Addr, if_index: u32) -> Result<(), String> {
        let name = if_name_for_index(if_index)?;
        let target = route::RouteTarget::new(0, &name, "");
        let cidr = host_route_cidr(&ip.to_string());
        route::add_route(&cidr, &target, 1).map(|_| ())
    }

    fn del_flow_route(&self, ip: Ipv4Addr) -> Result<(), String> {
        // 删的是 /32 主机路由，用 -host 形式即可（route::delete_route 需要
        // 一个 RouteTarget，而这里只有 ip，用 delete_host_route 语义更贴切）。
        route::delete_host_route(&ip.to_string())
    }

    fn add_capture_routes(&self, if_index: u32) -> Result<(), String> {
        let name = if_name_for_index(if_index)?;
        let target = route::RouteTarget::new(0, &name, "");
        route::add_route("0.0.0.0/1", &target, 1)?;
        route::add_route("128.0.0.0/1", &target, 1)?;
        Ok(())
    }

    fn del_capture_routes(&self, if_index: u32) -> Result<(), String> {
        let name = if_name_for_index(if_index)?;
        let target = route::RouteTarget::new(0, &name, "");
        route::delete_route("0.0.0.0/1", &target)?;
        route::delete_route("128.0.0.0/1", &target)?;
        Ok(())
    }

    fn add_server_host_route(&self, ip: Ipv4Addr, gateway: &str) -> Result<(), String> {
        let target = route::RouteTarget::new(0, "", gateway);
        route::add_route(&host_route_cidr(&ip.to_string()), &target, 1).map(|_| ())
    }

    fn del_server_host_route(&self, ip: Ipv4Addr) -> Result<(), String> {
        route::delete_host_route(&ip.to_string())
    }
}

#[cfg(target_os = "macos")]
pub struct MacIfBinder;

#[cfg(target_os = "macos")]
impl MacIfBinder {
    pub fn new() -> Self {
        MacIfBinder
    }
}

#[cfg(target_os = "macos")]
impl IfBinder for MacIfBinder {
    fn bind_fd_to_if(&self, fd: i32, if_index: u32) -> std::io::Result<()> {
        let idx = if_index as libc::c_uint;
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_IP,
                libc::IP_BOUND_IF,
                &idx as *const libc::c_uint as *const libc::c_void,
                std::mem::size_of::<libc::c_uint>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

// ============================== Windows ==============================

/// 提权进程里不要依赖 PATH / 当前目录查找系统工具，避免被同名 exe 劫持。
#[cfg(windows)]
pub(crate) fn system32_tool(name: &str) -> std::path::PathBuf {
    std::env::var_os("SystemRoot")
        .map(|root| std::path::PathBuf::from(root).join("System32").join(name))
        .unwrap_or_else(|| std::path::PathBuf::from(name))
}

#[cfg(windows)]
pub struct WindowsRouteTable;

#[cfg(windows)]
impl WindowsRouteTable {
    pub fn new() -> Self {
        WindowsRouteTable
    }
}

#[cfg(windows)]
impl RouteTable for WindowsRouteTable {
    fn add_flow_route(&self, ip: Ipv4Addr, if_index: u32) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // 隧道网卡不一定有到达该地址的网段路由，先补一条 /32 再 connect。
        // 用 route.exe 而不是 PowerShell：每条新流都要走这里，PowerShell 启动太慢。
        let route_exe = system32_tool("route.exe");
        let _ = Command::new(&route_exe)
            .args(["delete", &ip.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        let status = Command::new(&route_exe)
            .args([
                "add",
                &ip.to_string(),
                "mask",
                "255.255.255.255",
                "0.0.0.0",
                "metric",
                "1",
                "if",
                &if_index.to_string(),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("调用 route.exe 失败: {}", e))?;
        if !status.success() {
            return Err(format!(
                "添加 VPN 流路由失败: {}/32 -> 接口 {}",
                ip, if_index
            ));
        }
        Ok(())
    }

    fn del_flow_route(&self, ip: Ipv4Addr) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new(system32_tool("route.exe"))
            .args(["delete", &ip.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        Ok(())
    }

    fn add_capture_routes(&self, if_index: u32) -> Result<(), String> {
        if if_index == 0 {
            return Err("捕获网卡接口索引为 0".to_string());
        }
        // 上次异常退出可能留下同样的半默认路由，先按前缀清干净。
        let _ = route::remove_route_prefix("0.0.0.0/1");
        let _ = route::remove_route_prefix("128.0.0.0/1");
        let target = route::RouteTarget::new(if_index, "", "");
        route::add_route("0.0.0.0/1", &target, 1)?;
        route::add_route("128.0.0.0/1", &target, 1)?;
        Ok(())
    }

    fn del_capture_routes(&self, if_index: u32) -> Result<(), String> {
        let _ = if_index;
        let _ = route::remove_route_prefix("0.0.0.0/1");
        let _ = route::remove_route_prefix("128.0.0.0/1");
        Ok(())
    }

    fn add_server_host_route(&self, ip: Ipv4Addr, gateway: &str) -> Result<(), String> {
        if gateway.trim().is_empty() {
            // 没有物理网关就装不了防环路由；必须让上层知道失败，不能静默放行。
            return Err("物理默认网关为空，无法添加服务器防环路由".to_string());
        }
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // 重连时旧的主机路由可能还在；先删掉再添，避免 `route add` 因
        // “对象已存在”返回失败，把已经正确的路由误判成添加失败。
        let route_exe = system32_tool("route.exe");
        let _ = Command::new(&route_exe)
            .args(["delete", &ip.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        // route.exe 按网关自动选物理网卡，不需要先知道接口索引。
        let status = Command::new(&route_exe)
            .args([
                "add",
                &ip.to_string(),
                "mask",
                "255.255.255.255",
                gateway,
                "metric",
                "1",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("调用 route.exe 失败: {}", e))?;
        if !status.success() {
            return Err(format!(
                "添加服务器主机路由失败: {}/32 -> {}",
                ip, gateway
            ));
        }
        Ok(())
    }

    fn del_server_host_route(&self, ip: Ipv4Addr) -> Result<(), String> {
        route::delete_host_route(&ip.to_string())
    }
}

#[cfg(windows)]
pub struct WindowsIfBinder;

#[cfg(windows)]
impl WindowsIfBinder {
    pub fn new() -> Self {
        WindowsIfBinder
    }
}

#[cfg(windows)]
mod win_sock {
    #[link(name = "ws2_32")]
    extern "system" {
        pub fn setsockopt(
            socket: usize,
            level: i32,
            optname: i32,
            optval: *const u8,
            optlen: i32,
        ) -> i32;
    }

    /// Windows 的 IP_UNICAST_IF。
    pub const IP_UNICAST_IF: i32 = 31;
}

/// 把原生 SOCKET 的出口钉到指定接口（供 socket2 创建的 socket 复用）。
#[cfg(windows)]
pub fn bind_raw_socket_to_if(raw: usize, if_index: u32) -> std::io::Result<()> {
    // IP_UNICAST_IF 的接口索引要求网络字节序。
    let value = if_index.to_be();
    let rc = unsafe {
        win_sock::setsockopt(
            raw,
            0, // IPPROTO_IP
            win_sock::IP_UNICAST_IF,
            &value as *const u32 as *const u8,
            std::mem::size_of::<u32>() as i32,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
impl IfBinder for WindowsIfBinder {
    fn bind_fd_to_if(&self, fd: i32, if_index: u32) -> std::io::Result<()> {
        bind_raw_socket_to_if(fd as usize, if_index)
    }
}

#[cfg(windows)]
pub struct WindowsProcLookup;

#[cfg(windows)]
impl WindowsProcLookup {
    pub fn new() -> Self {
        WindowsProcLookup
    }
}

#[cfg(windows)]
impl ProcLookup for WindowsProcLookup {
    fn pid_for(&mut self, _key: &FlowKey) -> Option<u32> {
        // PROCESS-NAME 规则在 Windows 上暂不生效：逐连接调用 IP Helper 需要
        // 额外的 Win32 表结构绑定，本期先把域名 / 端口 / IP 规则打穿。
        None
    }

    fn name_for_pid(&mut self, _pid: u32) -> Option<String> {
        None
    }
}

// ============================== 平台别名 ==============================

#[cfg(target_os = "macos")]
pub type SystemRouteTable = MacRouteTable;
#[cfg(windows)]
pub type SystemRouteTable = WindowsRouteTable;

#[cfg(target_os = "macos")]
pub type SystemIfBinder = MacIfBinder;
#[cfg(windows)]
pub type SystemIfBinder = WindowsIfBinder;

#[cfg(windows)]
pub type SystemProcLookup = WindowsProcLookup;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_key_is_hashable_and_distinguishes_transport() {
        use std::collections::HashSet;
        let a = FlowKey {
            transport: Transport::Tcp,
            local: "10.0.0.2:5000".parse::<SocketAddrV4>().unwrap(),
            remote: "1.1.1.1:443".parse::<SocketAddrV4>().unwrap(),
        };
        let b = FlowKey { transport: Transport::Udp, ..a };
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn cidr_to_single_host_route_target_is_slash32() {
        assert_eq!(host_route_cidr("1.2.3.4"), "1.2.3.4/32");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn if_name_lookup_rejects_bogus_index() {
        // 极大索引必然查不到名字
        assert!(if_name_for_index(9_999_999).is_err());
    }
}
