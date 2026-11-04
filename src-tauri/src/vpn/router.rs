//! 出口选择与双向字节搬运。
//!
//! 每条应用侧连接被 netstack 终结后，这里按规则决定出口：
//! - DIRECT：出站 socket 钉物理网卡；
//! - VPN：出站 socket 钉隧道网卡。
//!
//! 钉网卡的方式按平台分：macOS 用 `IP_BOUND_IF`，Windows 用 `IP_UNICAST_IF`。
//! 两者都是 socket 级设置，不会像改路由表那样影响其它进程。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::os::unix::io::{FromRawFd, RawFd};

use super::dns_client;
use super::fakedns::FakeIpPool;
use super::netstack::{Netstack, NetstackConfig, TunPhy};
use super::platform::{Egress, FlowKey, IfBinder, ProcLookup, RouteTable, TunDevice};
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
    socket: UdpSocket,
    /// 出站真实目标；fake-ip 连接的目标与五元组里的 remote 不同。
    target: SocketAddrV4,
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
    /// VPN 流路由的引用计数：同一真实 IP 的多条流共用一条 /32
    vpn_routes: HashMap<Ipv4Addr, usize>,
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
            vpn_routes: HashMap::new(),
        }
    }

    /// 为一条 VPN 流保留（必要时新建）到真实 IP 的 /32 路由。
    fn retain_vpn_route(&mut self, ip: Ipv4Addr) -> std::io::Result<()> {
        let count = self.vpn_routes.entry(ip).or_insert(0);
        if *count == 0 {
            self.route
                .add_flow_route(ip, self.vpn_if)
                .map_err(std::io::Error::other)?;
        }
        *count += 1;
        Ok(())
    }

    /// 释放一次引用；最后一条流关闭时才真正删路由。
    fn release_vpn_route(&mut self, ip: Ipv4Addr) {
        if let Some(count) = self.vpn_routes.get_mut(&ip) {
            if *count > 1 {
                *count -= 1;
                return;
            }
        }
        if self.vpn_routes.remove(&ip).is_some() {
            let _ = self.route.del_flow_route(ip);
        }
    }

    /// 为一条终结后的应用连接建立出站 TCP。
    pub fn open_egress(&mut self, key: &FlowKey, egress: Egress) -> std::io::Result<()> {
        self.open_egress_to(key, key.remote, egress)
    }

    /// 同上，但允许出站目标与五元组里的 remote 不同（fake-ip -> 真实 IP）。
    pub fn open_egress_to(
        &mut self,
        key: &FlowKey,
        target: SocketAddrV4,
        egress: Egress,
    ) -> std::io::Result<()> {
        if self.flows.contains_key(key) {
            return Ok(());
        }
        let if_index = match egress {
            Egress::Direct => self.physical_if,
            Egress::Vpn => self.vpn_if,
        };

        // 先把流路由装好再 connect：VPN 场景下真实 IP 可能只有隧道网卡可达，
        // 顺序反过来时 connect 会按旧路由走到物理网卡而失败。
        let mut installed_route = None;
        if egress == Egress::Vpn {
            let remote_ip = *target.ip();
            self.retain_vpn_route(remote_ip)?;
            installed_route = Some(remote_ip);
        }

        let stream = match connect_bound(target, if_index, &*self.binder) {
            Ok(s) => s,
            Err(e) => {
                if let Some(ip) = installed_route {
                    self.release_vpn_route(ip);
                }
                return Err(e);
            }
        };
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
        self.open_udp_egress_to(key, key.remote, egress)
    }

    pub fn open_udp_egress_to(
        &mut self,
        key: &FlowKey,
        target: SocketAddrV4,
        egress: Egress,
    ) -> std::io::Result<()> {
        if self.udp_flows.contains_key(key) {
            return Ok(());
        }
        let if_index = match egress {
            Egress::Direct => self.physical_if,
            Egress::Vpn => self.vpn_if,
        };
        let socket = bind_udp_socket(if_index, &*self.binder)?;
        let mut installed_route = None;
        if egress == Egress::Vpn {
            let ip = *target.ip();
            self.retain_vpn_route(ip)?;
            installed_route = Some(ip);
        }
        self.udp_flows.insert(
            key.to_owned(),
            UdpEgress { socket, target, installed_route },
        );
        Ok(())
    }

    /// 把应用侧的 UDP 数据报发出站。
    pub fn udp_sendto(&self, key: &FlowKey, payload: &[u8]) -> std::io::Result<usize> {
        match self.udp_flows.get(key) {
            Some(f) => f
                .socket
                .send_to(payload, std::net::SocketAddr::V4(f.target)),
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

    /// 用一条临时 UDP 流做一次 DNS 查询，返回原始应答。
    ///
    /// 出站 socket 已经钉在选定网卡上，所以不会再次被捕获网卡抓回来。
    pub fn dns_exchange(
        &mut self,
        server: Ipv4Addr,
        egress: Egress,
        query: &[u8],
        timeout: Duration,
    ) -> Option<Vec<u8>> {
        let key = FlowKey {
            transport: super::platform::Transport::Udp,
            local: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0),
            remote: SocketAddrV4::new(server, DNS_PORT),
        };
        if self.open_udp_egress(&key, egress).is_err() {
            return None;
        }
        let _ = self.udp_sendto(&key, query);
        let deadline = Instant::now() + timeout;
        let mut buf = vec![0u8; 4096];
        let mut answer = None;
        while Instant::now() < deadline {
            match self.udp_recv(&key, &mut buf) {
                Ok(n) if n > 0 => {
                    answer = Some(buf[..n].to_vec());
                    break;
                }
                _ => {}
            }
        }
        self.close_flow(&key);
        answer
    }

    /// 当前所有 UDP 流的五元组（供回程遍历）。
    pub fn udp_flow_keys(&self) -> Vec<FlowKey> {
        self.udp_flows.keys().copied().collect()
    }

    /// 把出站侧可读到的数据搬进应用侧（非阻塞语义，超时返回 0）。
    pub fn pump_out_to_app(
        &mut self,
        key: &FlowKey,
        app_tx: &mut dyn std::io::Write,
    ) -> std::io::Result<usize> {
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
                self.release_vpn_route(ip);
            }
        }
        if let Some(flow) = self.udp_flows.remove(key) {
            if let Some(ip) = flow.installed_route {
                self.release_vpn_route(ip);
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

/// macOS：手工建连，必须在 connect **之前** 用 `IP_BOUND_IF` 把 socket 钉到网卡，
/// 标准库的 `TcpStream::connect_timeout` 无法满足，所以走 libc。
#[cfg(target_os = "macos")]
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

/// macOS：建一个已钉网卡的 UDP socket。
#[cfg(target_os = "macos")]
fn bind_udp_socket(if_index: u32, binder: &dyn IfBinder) -> std::io::Result<UdpSocket> {
    let fd: RawFd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if let Err(e) = binder.bind_fd_to_if(fd, if_index) {
        unsafe { libc::close(fd) };
        return Err(e);
    }
    let socket = unsafe { UdpSocket::from_raw_fd(fd) };
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(socket)
}

/// Windows：socket2 建连，connect 之前用 `IP_UNICAST_IF` 钉网卡。
#[cfg(windows)]
fn connect_bound(
    remote: SocketAddrV4,
    if_index: u32,
    _binder: &dyn IfBinder,
) -> std::io::Result<TcpStream> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::os::windows::io::AsRawSocket;

    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    super::platform::bind_raw_socket_to_if(socket.as_raw_socket() as usize, if_index)?;
    let addr = socket2::SockAddr::from(std::net::SocketAddr::V4(remote));
    socket.connect_timeout(&addr, Duration::from_secs(CONNECT_TIMEOUT_SECS))?;
    let stream: TcpStream = socket.into();
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

#[cfg(windows)]
fn bind_udp_socket(if_index: u32, _binder: &dyn IfBinder) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::os::windows::io::AsRawSocket;

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    super::platform::bind_raw_socket_to_if(socket.as_raw_socket() as usize, if_index)?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(socket)
}

/// 其它平台：能编译，但没有可用的接口钉定。
#[cfg(not(any(target_os = "macos", windows)))]
fn connect_bound(
    remote: SocketAddrV4,
    _if_index: u32,
    _binder: &dyn IfBinder,
) -> std::io::Result<TcpStream> {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::V4(remote),
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
    )
}

#[cfg(not(any(target_os = "macos", windows)))]
fn bind_udp_socket(_if_index: u32, _binder: &dyn IfBinder) -> std::io::Result<UdpSocket> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(socket)
}

#[cfg(target_os = "macos")]
struct FdGuard(RawFd);
#[cfg(target_os = "macos")]
impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

#[cfg(target_os = "macos")]
fn set_nonblocking(fd: RawFd, on: bool) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let new = if on {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, new) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn wait_writable(fd: RawFd, timeout_secs: u64) -> std::io::Result<()> {
    let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
    let rc = unsafe { libc::poll(&mut pfd, 1, (timeout_secs * 1000) as libc::c_int) };
    if rc <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "connect timeout",
        ));
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
pub fn decide_domain(
    rules: &CompiledRuleSet,
    domain: &str,
    key: &FlowKey,
    process: Option<&str>,
) -> Egress {
    decide_domain_at(rules, domain, *key.remote.ip(), key.remote.port(), process)
}

/// 用「域名 + 已解析出的真实 IP」做判定。
///
/// fake-ip 模式下连接刚到达时只有假地址，必须等真实解析完成后再用真实 IP
/// 参与 `IP-CIDR` 规则；否则「只写了网段规则、没写域名规则」的配置会全部漏判。
pub fn decide_domain_at(
    rules: &CompiledRuleSet,
    domain: &str,
    ip: Ipv4Addr,
    port: u16,
    process: Option<&str>,
) -> Egress {
    let ip = ip.to_string();
    let input = MatchInput {
        domain: Some(domain),
        ip: Some(&ip),
        port: Some(port),
        process,
    };
    Egress::from(rules.evaluate(&input))
}

/// 引擎：持有 netstack、出口 Router、规则集与进程反查，供 worker 主循环驱动。
pub struct Engine {
    pub router: Router,
    pub netstack: Netstack<Box<dyn TunDevice>>,
    pub rules: CompiledRuleSet,
    proc: Box<dyn ProcLookup>,
    #[allow(dead_code)]
    default_policy: Policy,
    /// fake-ip 分配池（DNS 本地应答时用），由 worker 装配后注入
    fake_pool: Option<FakeIpPool>,
    /// 已分配的「假 IP(u32) -> 域名」，供命中假地址的连接做域名规则匹配
    fake_domains: HashMap<u32, String>,
    /// 域名 -> 应用查询时用的 DNS 服务器，连接阶段做真实解析时复用
    dns_servers: HashMap<String, Ipv4Addr>,
    /// 服务端下发的 VPN DNS，应用 DNS 拿不到 A 记录时兜底
    vpn_dns: Vec<Ipv4Addr>,
    /// 域名 -> 已解析出的真实 IPv4，连接期间复用
    real_cache: HashMap<String, Ipv4Addr>,
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        capture: Box<dyn TunDevice>,
        netstack_cfg: NetstackConfig,
        rules: CompiledRuleSet,
        route: Box<dyn RouteTable>,
        binder: Box<dyn IfBinder>,
        physical_if: u32,
        vpn_if: u32,
        proc: Box<dyn ProcLookup>,
        default_policy: &str,
    ) -> Result<Self, String> {
        let mtu = if netstack_cfg.mtu == 0 { 1500 } else { netstack_cfg.mtu };
        let phy = TunPhy::new(capture, mtu);
        let ns = Netstack::new(phy, netstack_cfg.local_ip, [0x02, 0, 0, 0, 0, 1]);
        let router = Router::new(route, binder, physical_if, vpn_if);
        Ok(Engine {
            router,
            netstack: ns,
            rules,
            proc,
            default_policy: Policy::parse(default_policy).unwrap_or(Policy::Direct),
            fake_pool: None,
            fake_domains: netstack_cfg.fake_domains,
            dns_servers: HashMap::new(),
            vpn_dns: Vec::new(),
            real_cache: HashMap::new(),
        })
    }

    /// 注入 fake-ip 池（装配后由 worker 调用）。
    pub fn set_fake_pool(&mut self, pool: FakeIpPool) {
        self.fake_pool = Some(pool);
    }

    /// 注入服务端下发的 VPN DNS（可能为空）。
    pub fn set_vpn_dns(&mut self, servers: Vec<Ipv4Addr>) {
        self.vpn_dns = servers;
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
            // 先按域名（必要时再按真实 IP）定出口。fake-ip 刚到达时只有假地址，
            // 先用域名做一次临时判定去解析真实 IP，再拿真实 IP 重判一次。
            let provisional = match domain.as_deref() {
                Some(d) => decide_domain(&self.rules, d, &flow.key, process.as_deref()),
                None => decide_egress(&self.rules, &flow.key, process.as_deref()),
            };
            let (target, egress) = match domain.as_deref() {
                Some(d) => match self.resolve_name(d, provisional) {
                    Some(ip) => {
                        let egress = decide_domain_at(
                            &self.rules,
                            d,
                            ip,
                            flow.key.remote.port(),
                            process.as_deref(),
                        );
                        (SocketAddrV4::new(ip, flow.key.remote.port()), egress)
                    }
                    None => {
                        self.netstack.abort_tcp(&flow.key);
                        continue;
                    }
                },
                None => (flow.key.remote, provisional),
            };
            if self.router.open_egress_to(&flow.key, target, egress).is_err() {
                self.netstack.abort_tcp(&flow.key);
            }
        }
        // 2) TCP 双向搬运
        self.netstack.pump_tcp(&mut self.router);

        // 3) UDP：DNS 本地应答，其余按规则转发
        for dg in self.netstack.take_udp_datagrams() {
            // 目的端口即真实远端端口（key.remote.port()）
            if is_dns_port(dg.key.remote.port()) {
                if let Some(resp) = self.dns_respond(dg.key.remote, &dg.payload) {
                    self.netstack.send_udp_to_app(&dg.key, &resp);
                }
                continue;
            }
            let domain = self.fake_domains.get(&u32::from(*dg.key.remote.ip())).cloned();
            let process = self
                .proc
                .pid_for(&dg.key)
                .and_then(|pid| self.proc.name_for_pid(pid));
            let provisional = match domain.as_deref() {
                Some(d) => decide_domain(&self.rules, d, &dg.key, process.as_deref()),
                None => decide_egress(&self.rules, &dg.key, process.as_deref()),
            };
            let (target, egress) = match domain.as_deref() {
                Some(d) => match self.resolve_name(d, provisional) {
                    Some(ip) => {
                        let egress = decide_domain_at(
                            &self.rules,
                            d,
                            ip,
                            dg.key.remote.port(),
                            process.as_deref(),
                        );
                        (SocketAddrV4::new(ip, dg.key.remote.port()), egress)
                    }
                    None => continue,
                },
                None => (dg.key.remote, provisional),
            };
            if self.router.open_udp_egress_to(&dg.key, target, egress).is_ok() {
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
    ///
    /// `fake_ip_filter` 命中的域名不伪造，直接把原始查询转发给上游并原样回传，
    /// 保证本地服务 / mDNS 这类场景拿到的仍是真实地址。
    fn dns_respond(&mut self, server: SocketAddrV4, payload: &[u8]) -> Option<Vec<u8>> {
        let query = super::dns_srv::parse_query(payload)?;
        let name = query.name.to_ascii_lowercase();

        let filtered = self
            .fake_pool
            .as_ref()
            .map(|pool| pool.is_filtered(&name))
            .unwrap_or(true);

        if filtered || query.qtype != 1 {
            if query.qtype != 1 {
                return Some(super::dns_srv::build_empty_response(payload, &query));
            }
            let policy = self.rules.evaluate(&MatchInput {
                domain: Some(&name),
                ..Default::default()
            });
            let resp = self.router.dns_exchange(
                *server.ip(),
                Egress::from(policy),
                payload,
                Duration::from_millis(1500),
            );
            return Some(
                resp.unwrap_or_else(|| super::dns_srv::build_empty_response(payload, &query)),
            );
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let ip = {
            let pool = self.fake_pool.as_mut()?;
            pool.allocate(&name, now, 300)?
        };
        self.fake_domains.insert(ip, name.clone());
        self.dns_servers.insert(name, *server.ip());
        Some(super::dns_srv::build_a_response(payload, &query, Ipv4Addr::from(ip)))
    }

    /// 把 fake-ip 反查出的域名解析成真实 IPv4。
    ///
    /// 优先用应用自己查询时用的 DNS 服务器；失败后换另一个出口重试，
    /// 再失败才退回公共 DNS，避免 VPN DNS 只在隧道内可达时把直连域名卡死。
    fn resolve_name(&mut self, name: &str, egress: Egress) -> Option<Ipv4Addr> {
        let key = name.to_ascii_lowercase();
        if let Some(ip) = self.real_cache.get(&key) {
            return Some(*ip);
        }
        let server = self
            .dns_servers
            .get(&key)
            .copied()
            .unwrap_or(Ipv4Addr::new(223, 5, 5, 5));
        let query_id = next_dns_id();
        let query = dns_client::build_query(&key, query_id)?;

        let has_a = |answer: &Option<Vec<u8>>| {
            answer
                .as_deref()
                .filter(|packet| dns_client::is_response_to(packet, query_id))
                .map(|packet| !dns_client::parse_a_records(packet).is_empty())
                .unwrap_or(false)
        };

        let mut answer = self.router.dns_exchange(
            server,
            egress,
            &query,
            Duration::from_millis(1200),
        );
        if !has_a(&answer) {
            let other = match egress {
                Egress::Vpn => Egress::Direct,
                Egress::Direct => Egress::Vpn,
            };
            answer = self.router.dns_exchange(
                server,
                other,
                &query,
                Duration::from_millis(800),
            );
        }
        // 应用的 DNS 可能只解析公网域名；内网域名再问一遍 VPN 下发的 DNS。
        if !has_a(&answer) {
            for dns in self.vpn_dns.clone() {
                if dns == server {
                    continue;
                }
                answer = self.router.dns_exchange(
                    dns,
                    Egress::Vpn,
                    &query,
                    Duration::from_millis(700),
                );
                if has_a(&answer) {
                    break;
                }
            }
        }
        let public = Ipv4Addr::new(223, 5, 5, 5);
        if !has_a(&answer) && server != public {
            answer = self.router.dns_exchange(
                public,
                Egress::Direct,
                &query,
                Duration::from_millis(800),
            );
        }

        let ip = answer
            .as_deref()
            .filter(|packet| dns_client::is_response_to(packet, query_id))
            .map(dns_client::parse_a_records)
            .unwrap_or_default()
            .into_iter()
            .next()?;
        self.real_cache.insert(key, ip);
        Some(ip)
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
        self.fake_domains.clear();
        self.dns_servers.clear();
        self.vpn_dns.clear();
        self.real_cache.clear();
    }
}

/// DNS 查询 ID：只需在当前进程内尽量不重复。
fn next_dns_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0x4d2);
    NEXT.fetch_add(1, Ordering::Relaxed)
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
        assert_eq!(
            decide(rules, Policy::Direct, "x.com", "1.2.3.4", 443),
            Policy::Direct
        );
    }

    #[test]
    fn ip_rule_matches_real_ip_behind_fake_domain() {
        let rules = vec![Rule::new(RuleKind::IpCidr, "10.0.0.0/8", Policy::Vpn)];
        let set = compile(&rules, &HashMap::new(), Policy::Direct).unwrap();
        // 假地址 198.18.x.x 不落在 10/8 里，只拿它判定会漏掉网段规则。
        let fake = MatchInput {
            domain: Some("git.example.com"),
            ip: Some("198.18.0.7"),
            port: Some(443),
            process: None,
        };
        assert_eq!(set.evaluate(&fake), Policy::Direct);
        // 解析出真实内网地址后必须命中 IP-CIDR。
        assert_eq!(
            decide_domain_at(&set, "git.example.com", "10.1.2.3".parse().unwrap(), 443, None),
            Egress::Vpn
        );
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
