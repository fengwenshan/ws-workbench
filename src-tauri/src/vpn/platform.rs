//! 平台抽象：TUN 设备、路由表、接口绑定、进程反查。
//!
//! 本竖切只落 macOS 实现；Windows 实现留待下一竖切。trait 先立好，
//! 让 `netstack`/`router` 不直接依赖 `libc`。

use std::net::{Ipv4Addr, SocketAddrV4};

#[cfg(target_os = "macos")]
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
    /// 走隧道：钉隧道网卡（openconnect 建立的 utunZ）
    Vpn,
}

/// TUN 设备的最小能力。读写的都是一个完整 IP 包。
pub trait TunDevice: Send {
    fn name(&self) -> &str;
    fn index(&self) -> u32;
    fn read_packet(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_packet(&mut self, buf: &[u8]) -> std::io::Result<usize>;
}

/// 路由表操作。macOS 走 `/sbin/route`。
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
