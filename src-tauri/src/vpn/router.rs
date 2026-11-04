#![cfg(target_os = "macos")]
//! 出口选择与双向字节搬运。
//!
//! 每条应用侧连接被 netstack 终结后，这里按规则决定出口：
//! - DIRECT：出站 socket `IP_BOUND_IF` 钉物理网卡；
//! - VPN：出站 socket 钉隧道网卡，并加一条 `/32 -> 隧道` 流路由。
//! 随后在两个方向搬运字节。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::os::unix::io::{FromRawFd, RawFd};
use std::time::Duration;

use super::platform::{Egress, FlowKey, IfBinder, RouteTable};
use super::rule_engine::{CompiledRuleSet, MatchInput, Policy};

pub const CONNECT_TIMEOUT_SECS: u64 = 10;

impl From<Policy> for Egress {
    fn from(policy: Policy) -> Self {
        match policy {
            Policy::Vpn => Egress::Vpn,
            Policy::Direct => Egress::Direct,
        }
    }
}

/// DNS 端口：目的端口为它的 UDP 数据报在本地应答，不出站。
pub const DNS_PORT: u16 = 53;

/// 判断一个目的端口是不是 DNS。
pub fn is_dns_port(port: u16) -> bool {
    port == DNS_PORT
}

/// 每条流的出站状态。
struct FlowEgress {
    stream: Option<TcpStream>,
    /// VPN 出口为这条流加的 /32 路由（断开时需要回收）
    installed_route: Option<Ipv4Addr>,
}

/// 一条 UDP 流的出站状态。
struct UdpEgress {
    socket: std::net::UdpSocket,
    /// VPN 出口为这条流加的 /32 路由（断开时需要回收）
    installed_route: Option<Ipv4Addr>,
}

/// 出口资源持有者。物理网卡索引与隧道网卡索引在连接建立时确定。
pub struct Router {
    route: Box<dyn RouteTable>,
    binder: Box<dyn IfBinder>,
    physical_if: u32,
    vpn_if: u32,
    flows: HashMap<FlowKey, FlowEgress>,
    udp_flows: HashMap<FlowKey, UdpEgress>,
}

impl Router {
    pub fn new(
        route: Box<dyn RouteTable>,
        binder: Box<dyn IfBinder>,
        physical_if: u32,
        vpn_if: u32,
    ) -> Self {
        Router {
            route,
            binder,
            physical_if,
            vpn_if,
            flows: HashMap::new(),
            udp_flows: HashMap::new(),
        }
    }

    /// 为一条终结后的应用连接建立出站 TCP。
    pub fn open_egress(&mut self, key: &FlowKey, egress: Egress) -> std::io::Result<()> {
        if self.flows.contains_key(key) {
            return Ok(());
        }
        let if_index = match egress {
            Egress::Direct => self.physical_if,
            Egress::Vpn => self.vpn_if,
        };
        let stream = connect_bound(key.remote, if_index, &*self.binder)?;

        let mut installed_route = None;
        if egress == Egress::Vpn {
            let remote_ip = *key.remote.ip();
            self.route
                .add_flow_route(remote_ip, self.vpn_if)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            installed_route = Some(remote_ip);
        }

        self.flows.insert(key.to_owned(), FlowEgress { stream: Some(stream), installed_route });
        Ok(())
    }

    /// 一条 TCP 流是否已经建立出站。
    pub fn is_open(&self, key: &FlowKey) -> bool {
        self.flows.contains_key(key)
    }

    /// 把应用侧数据写进出站流（非阻塞语义：写不进去就返回已写字节数）。
    pub fn egress_write(&mut self, key: &FlowKey, data: &[u8]) -> std::io::Result<usize> {
        if let Some(f) = self.flows.get_mut(key) {
            if let Some(s) = f.stream.as_mut() {
                return s.write(data);
            }
        }
        Ok(0)
    }

    /// 从出站流读数据交给应用侧（超时按「暂无数据」处理，返回 0）。
    pub fn egress_read(&mut self, key: &FlowKey, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(f) = self.flows.get_mut(key) {
            if let Some(s) = f.stream.as_mut() {
                match s.read(buf) {
                    Ok(n) => return Ok(n),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        return Ok(0)
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(0)
    }

    /// 为一条 UDP 流建立出站 socket：钉网卡，必要时装流路由。
    pub fn open_udp_egress(&mut self, key: &FlowKey, egress: Egress) -> std::io::Result<()> {
        if self.udp_flows.contains_key(key) {
            return Ok(());
        }
        let if_index = match egress {
            Egress::Direct => self.physical_if,
            Egress::Vpn => self.vpn_if,
        };
        let fd: RawFd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if let Err(e) = self.binder.bind_fd_to_if(fd, if_index) {
            unsafe { libc::close(fd) };
            return Err(e);
        }
        let socket = unsafe { std::net::UdpSocket::from_raw_fd(fd) };
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let mut installed_route = None;
        if egress == Egress::Vpn {
            let ip = *key.remote.ip();
            self.route
                .add_flow_route(ip, self.vpn_if)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            installed_route = Some(ip);
        }
        self.udp_flows.insert(key.to_owned(), UdpEgress { socket, installed_route });
        Ok(())
    }

    /// 把应用侧的 UDP 数据报发出站。
    pub fn udp_sendto(&self, key: &FlowKey, payload: &[u8]) -> std::io::Result<usize> {
        match self.udp_flows.get(key) {
            Some(f) => f.socket.send_to(payload, std::net::SocketAddr::V4(key.remote)),
            None => Ok(0),
        }
    }

    /// 读出站 UDP 回包（超时按「暂无数据」处理，返回 0）。
    pub fn udp_recv(&self, key: &FlowKey, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.udp_flows.get(key) {
            Some(f) => match f.socket.recv(buf) {
                Ok(n) => Ok(n),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    Ok(0)
                }
                Err(e) => Err(e),
            },
            None => Ok(0),
        }
    }

    /// 当前所有 UDP 流的五元组（供回程遍历）。
    pub fn udp_flow_keys(&self) -> Vec<FlowKey> {
        self.udp_flows.keys().copied().collect()
    }

    /// 把出站侧可读到的数据搬进应用侧（非阻塞语义，超时返回 0）。
    pub fn pump_out_to_app(&mut self, key: &FlowKey, app_tx: &mut dyn std::io::Write) -> std::io::Result<usize> {
        let flow = match self.flows.get_mut(key) {
            Some(f) => f,
            None => return Ok(0),
        };
        let stream = match flow.stream.as_mut() {
            Some(s) => s,
            None => return Ok(0),
        };
        let mut buf = [0u8; 16 * 1024];
        match stream.read(&mut buf) {
            Ok(0) => Ok(0),
            Ok(n) => {
                app_tx.write_all(&buf[..n])?;
                Ok(n)
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }

    /// 关闭一条流并回收其 VPN 流路由（TCP 与 UDP 都覆盖）。
    pub fn close_flow(&mut self, key: &FlowKey) {
        if let Some(flow) = self.flows.remove(key) {
            if let Some(ip) = flow.installed_route {
                let _ = self.route.del_flow_route(ip);
            }
        }
        if let Some(flow) = self.udp_flows.remove(key) {
            if let Some(ip) = flow.installed_route {
                let _ = self.route.del_flow_route(ip);
            }
        }
    }

    pub fn open_flow_count(&self) -> usize {
        self.flows.len()
    }

    pub fn flow_keys(&self) -> Vec<FlowKey> {
        self.flows.keys().copied().collect()
    }
}

/// 手工建连：必须在 connect **之前** 用 `IP_BOUND_IF` 把 socket 钉到网卡，
/// 标准库的 `TcpStream::connect_timeout` 无法满足，所以走 libc。
fn connect_bound(
    remote: SocketAddrV4,
    if_index: u32,
    binder: &dyn IfBinder,
) -> std::io::Result<TcpStream> {
    let fd: RawFd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let guard = FdGuard(fd);

    binder.bind_fd_to_if(fd, if_index)?;
    set_nonblocking(fd, true)?;

    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    addr.sin_family = libc::AF_INET as libc::sa_family_t;
    addr.sin_port = remote.port().to_be();
    addr.sin_addr.s_addr = u32::from(*remote.ip()).to_be();

    let rc = unsafe {
        libc::connect(
            fd,
            &addr as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(err);
        }
        wait_writable(fd, CONNECT_TIMEOUT_SECS)?;
        let mut soerr: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                &mut soerr as *mut _ as *mut libc::c_void,
                &mut len,
            );
        }
        if soerr != 0 {
            return Err(std::io::Error::from_raw_os_error(soerr));
        }
    }
    set_nonblocking(fd, false)?;
    let stream = unsafe { TcpStream::from_raw_fd(fd) };
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    stream.set_nodelay(true)?;
    std::mem::forget(guard);
    Ok(stream)
}

struct FdGuard(RawFd);
impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn set_nonblocking(fd: RawFd, on: bool) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let new = if on { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, new) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn wait_writable(fd: RawFd, timeout_secs: u64) -> std::io::Result<()> {
    let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
    let rc = unsafe { libc::poll(&mut pfd, 1, (timeout_secs * 1000) as libc::c_int) };
    if rc <= 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"));
    }
    Ok(())
}

/// 查询一条已终结连接应走的出口（按 IP/端口/进程匹配）。
pub fn decide_egress(rules: &CompiledRuleSet, key: &FlowKey, process: Option<&str>) -> Egress {
    let ip = key.remote.ip().to_string();
    let input = MatchInput {
        domain: None,
        ip: Some(&ip),
        port: Some(key.remote.port()),
        process,
    };
    Egress::from(rules.evaluate(&input))
}

/// 按域名构造一次出口判定（供 netstack 在 accept 时用 fake-ip 反查出的域名调用）。
pub fn decide_domain(rules: &CompiledRuleSet, domain: &str, key: &FlowKey, process: Option<&str>) -> Egress {
    let ip = key.remote.ip().to_string();
    let input = MatchInput {
        domain: Some(domain),
        ip: Some(&ip),
        port: Some(key.remote.port()),
        process,
    };
    Egress::from(rules.evaluate(&input))
}

use super::fakedns::FakeIpPool;
use super::netstack::{Netstack, NetstackConfig, TunPhy};
use super::tun_device::Utun;
// `ProcLookup` 在 proc_lookup 里只是私有再导入，这里从定义处 platform 取
use super::platform::ProcLookup;

/// 引擎：持有 netstack、出口 Router、规则集与进程反查，供 worker 主循环驱动。
pub struct Engine {
    pub router: Router,
    pub netstack: Netstack<Utun>,
    pub rules: CompiledRuleSet,
    proc: Box<dyn ProcLookup>,
    #[allow(dead_code)]
    default_policy: Policy,
    /// fake-ip 分配池（DNS 本地应答时用），由 worker 装配后注入
    fake_pool: Option<FakeIpPool>,
    /// 已分配的「假 IP(u32) -> 域名」，供命中假地址的连接做域名规则匹配
    fake_domains: HashMap<u32, String>,
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        capture: Utun,
        netstack_cfg: NetstackConfig,
        rules: CompiledRuleSet,
        route: Box<dyn RouteTable>,
        binder: Box<dyn IfBinder>,
        capture_if: u32,
        vpn_if: u32,
        proc: Box<dyn ProcLookup>,
        default_policy: &str,
    ) -> Result<Self, String> {
        let mtu = if netstack_cfg.mtu == 0 { 1500 } else { netstack_cfg.mtu };
        let phy = TunPhy::new(capture, mtu);
        let ns = Netstack::new(phy, netstack_cfg.local_ip, [0x02, 0, 0, 0, 0, 1]);
        let router = Router::new(route, binder, capture_if, vpn_if);
        Ok(Engine {
            router,
            netstack: ns,
            rules,
            proc,
            default_policy: Policy::parse(default_policy).unwrap_or(Policy::Direct),
            fake_pool: None,
            fake_domains: netstack_cfg.fake_domains,
        })
    }

    /// 注入 fake-ip 池（装配后由 worker 调用）。
    pub fn set_fake_pool(&mut self, pool: FakeIpPool) {
        self.fake_pool = Some(pool);
    }

    /// 一轮收发：推进协议栈 -> 为新流选出口建出站 -> 双向搬运 -> UDP/DNS。
    pub fn poll(&mut self) -> Result<(), String> {
        let now = smoltcp::time::Instant::now();
        self.netstack.poll(now);

        // 1) 新 TCP 流 -> 选出口 -> 建出站
        for flow in self.netstack.take_new_tcp_flows() {
            let domain = self.fake_domains.get(&u32::from(*flow.key.remote.ip())).cloned();
            let process = self
                .proc
                .pid_for(&flow.key)
                .and_then(|pid| self.proc.name_for_pid(pid));
            let egress = match domain.as_deref() {
                Some(d) => decide_domain(&self.rules, d, &flow.key, process.as_deref()),
                None => decide_egress(&self.rules, &flow.key, process.as_deref()),
            };
            if self.router.open_egress(&flow.key, egress).is_err() {
                self.netstack.abort_tcp(&flow.key);
            }
        }
        // 2) TCP 双向搬运
        self.netstack.pump_tcp(&mut self.router);

        // 3) UDP：DNS 本地应答，其余按规则转发
        for dg in self.netstack.take_udp_datagrams() {
            // 目的端口即真实远端端口（key.remote.port()）
            if is_dns_port(dg.key.remote.port()) {
                if let Some(resp) = self.dns_respond(&dg.payload) {
                    self.netstack.send_udp_to_app(&dg.key, &resp);
                }
                continue;
            }
            let process = self
                .proc
                .pid_for(&dg.key)
                .and_then(|pid| self.proc.name_for_pid(pid));
            let egress = decide_egress(&self.rules, &dg.key, process.as_deref());
            if self.router.open_udp_egress(&dg.key, egress).is_ok() {
                let _ = self.router.udp_sendto(&dg.key, &dg.payload);
            }
        }
        // 4) UDP 回程：把出站回包写回应用侧
        for key in self.router.udp_flow_keys() {
            let mut buf = vec![0u8; 64 * 1024];
            if let Ok(n) = self.router.udp_recv(&key, &mut buf) {
                if n > 0 {
                    buf.truncate(n);
                    self.netstack.send_udp_to_app(&key, &buf);
                }
            }
        }
        Ok(())
    }

    /// 本地应答一个 DNS 查询：A 记录分配假地址，其余返回空答案。
    fn dns_respond(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        let query = super::dns_srv::parse_query(payload)?;
        if query.qtype != 1 {
            return Some(super::dns_srv::build_empty_response(payload, &query));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let name = query.name.clone();
        let ip = {
            let pool = self.fake_pool.as_mut()?;
            pool.allocate(&name, now, 300)?
        };
        self.fake_domains.insert(ip, name);
        Some(super::dns_srv::build_a_response(payload, &query, Ipv4Addr::from(ip)))
    }

    pub fn flow_count(&self) -> usize {
        self.router.open_flow_count()
    }

    pub fn shutdown(&mut self) {
        let mut keys = self.router.flow_keys();
        keys.extend(self.router.udp_flow_keys());
        for k in keys {
            self.router.close_flow(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::platform::Transport;
    use crate::vpn::rule_engine::{compile, Policy, Rule, RuleKind};
    use std::collections::HashMap;

    fn decide(rules: Vec<Rule>, default: Policy, domain: &str, ip: &str, port: u16) -> Policy {
        let set = compile(&rules, &HashMap::new(), default).unwrap();
        set.evaluate(&MatchInput {
            domain: Some(domain),
            ip: Some(ip),
            port: Some(port),
            process: None,
        })
    }

    #[test]
    fn first_match_wins_over_later_broader_rule() {
        let rules = vec![
            Rule::new(RuleKind::Domain, "a.com", Policy::Vpn),
            Rule::new(RuleKind::DomainKeyword, "a.com", Policy::Direct),
        ];
        assert_eq!(decide(rules, Policy::Direct, "a.com", "1.2.3.4", 443), Policy::Vpn);
    }

    #[test]
    fn default_policy_applies_when_nothing_matches() {
        let rules = vec![Rule::new(RuleKind::DstPort, "8080", Policy::Vpn)];
        assert_eq!(decide(rules, Policy::Direct, "x.com", "1.2.3.4", 443), Policy::Direct);
    }

    #[test]
    fn egress_maps_policy() {
        assert_eq!(Egress::from(Policy::Vpn), Egress::Vpn);
        assert_eq!(Egress::from(Policy::Direct), Egress::Direct);
    }

    #[test]
    fn tcp_connect_timeout_is_bounded() {
        assert!(CONNECT_TIMEOUT_SECS > 0 && CONNECT_TIMEOUT_SECS <= 30);
    }

    #[test]
    fn dns_destination_is_recognized() {
        assert!(is_dns_port(53));
        assert!(!is_dns_port(443));
    }

    #[test]
    fn udp_flow_key_is_distinct_per_remote() {
        let a = FlowKey {
            transport: Transport::Udp,
            local: "10.0.0.2:5000".parse().unwrap(),
            remote: "1.1.1.1:53".parse().unwrap(),
        };
        let b = FlowKey {
            transport: Transport::Udp,
            local: "10.0.0.2:5000".parse().unwrap(),
            remote: "8.8.8.8:53".parse().unwrap(),
        };
        assert_ne!(a, b);
    }
}
