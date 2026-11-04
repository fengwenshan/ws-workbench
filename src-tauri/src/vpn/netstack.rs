//! smoltcp 用户态网络栈：把捕获 TUN 上收到的 IP 包喂给 smoltcp，
//! 由 smoltcp 完成 TCP 三次握手/收发；应用侧连接终结后交 router 决定出口。
//! 本模块只做「终结」，不做出口选择。
//!
//! 透明接受的关键在三点：
//! - `Interface::set_any_ip(true)`：接受发往任意目的 IP 的包（捕获 TUN 是半默认路由，
//!   目的 IP 就是真实远端，不是本机地址）；
//! - 端口惰性学习：本机发往某个目的端口的第一个包到达时，才按需建监听/绑定；
//! - 首包重投：端口是驱动协议栈时才学到的，而此时 socket 尚未建立，故把首包压回
//!   重投队列，等下一轮 socket 建好再交付，避免首包被丢。

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, IpProtocol, Ipv4Address,
    Ipv4Packet, TcpPacket, UdpPacket,
};

use super::platform::{Transport, TunDevice};

pub use super::platform::FlowKey;

/// netstack 需要的配置（来自 VpnRequest + fake-ip 池）。
pub struct NetstackConfig {
    /// 捕获 TUN 的本地地址（点对点地址）
    pub local_ip: Ipv4Addr,
    pub mtu: usize,
    /// fake IP(u32) -> 域名
    pub fake_domains: HashMap<u32, String>,
}

impl Default for NetstackConfig {
    fn default() -> Self {
        NetstackConfig {
            local_ip: Ipv4Addr::UNSPECIFIED,
            mtu: 0,
            fake_domains: HashMap::new(),
        }
    }
}

impl NetstackConfig {
    pub fn domain_for_fake_ip(&self, ip: &str) -> Option<String> {
        let v: Ipv4Addr = ip.parse().ok()?;
        self.fake_domains.get(&u32::from(v)).cloned()
    }
}

pub fn fake_ip_to_u32(ip: &str) -> Option<u32> {
    ip.parse::<Ipv4Addr>().ok().map(u32::from)
}

impl FlowKey {
    pub fn from_parts(transport: Transport, local: SocketAddrV4, remote: SocketAddrV4) -> Self {
        FlowKey { transport, local, remote }
    }
}

/// 惰性学习到的「需要新建监听/绑定」的目的 (IP, 端口)，以及已建过 socket 的键。
///
/// `TunPhy::receive` 在每次读到包时填写，`Netstack::poll` 开头取走并消费——
/// 两个对象通过同一个 `Arc<Mutex<_>>` 共享，所以不必让 phy 持有 socket 集合。
#[derive(Default)]
pub struct PendingPorts {
    /// 待建监听的目的 (IP, 端口)（TCP）：来自 SYN && !ACK 的包
    pub tcp: HashSet<(Ipv4Addr, u16)>,
    /// 待建绑定的目的 (IP, 端口)（UDP）：来自任意 UDP 包
    pub udp: HashSet<(Ipv4Addr, u16)>,
    /// 已经历过「首次发现」的 (IP, 端口)；用于判断首包是否该压回重投队列。
    /// 与 `tcp`/`udp` 放在同一个共享结构里，避免两处状态不一致。
    pub known: HashSet<(Ipv4Addr, u16)>,
}

/// 从 IPv4 包里解析出「需要新建监听/绑定」的 (协议, 目的 IP, 目的端口)。
///
/// 只认 IPv4；解析失败（截断、非 IPv4）返回 `None`，由协议栈自己回 RST/丢弃。
/// TCP 只认主动握手的首个 SYN（SYN 且不带 ACK），避免被重传/半开包打扰。
/// 本函数不做任何状态写入，「是否新端口」由调用方（`TunPhy`）判断。
pub fn note_pending(
    buf: &[u8],
    _pending: &Arc<Mutex<PendingPorts>>,
) -> Option<(IpProtocol, Ipv4Addr, u16)> {
    let ipv4 = Ipv4Packet::new_checked(buf).ok()?;
    let dst_ip = Ipv4Addr::from(ipv4.dst_addr().0);
    match ipv4.next_header() {
        IpProtocol::Tcp => {
            let t = TcpPacket::new_checked(ipv4.payload()).ok()?;
            if t.syn() && !t.ack() {
                Some((IpProtocol::Tcp, dst_ip, t.dst_port()))
            } else {
                None
            }
        }
        IpProtocol::Udp => {
            let u = UdpPacket::new_checked(ipv4.payload()).ok()?;
            Some((IpProtocol::Udp, dst_ip, u.dst_port()))
        }
        _ => None,
    }
}

/// 构造「指定 IP + 端口」的监听端点：让 smoltcp 只接受发往该目的 IP 的包，
/// 且回包源地址即该 IP（修掉 UDP 回包源地址错的问题）。
fn listen_endpoint(ip: Ipv4Addr, port: u16) -> IpListenEndpoint {
    IpListenEndpoint {
        addr: Some(IpAddress::Ipv4(Ipv4Address::from_bytes(&ip.octets()))),
        port,
    }
}

/// 把 `TunDevice` 适配成 smoltcp 的 `Device`（medium = Ip，无以太头）。
pub struct TunPhy<D: TunDevice> {
    inner: D,
    mtu: usize,
    rx_buf: Vec<u8>,
    /// 与 `Netstack` 共享的待建端口集合与已建键
    pending: Arc<Mutex<PendingPorts>>,
    /// 因「发现的端口尚未建 socket」而被压回、待下一轮重投的包
    retry: VecDeque<Vec<u8>>,
}

impl<D: TunDevice> TunPhy<D> {
    pub fn new(inner: D, mtu: usize) -> Self {
        TunPhy {
            inner,
            mtu,
            rx_buf: vec![0u8; mtu],
            pending: Arc::new(Mutex::new(PendingPorts::default())),
            retry: VecDeque::new(),
        }
    }

    /// 注入由 `Netstack` 建立的共享集合，保证两侧看到同一份待建端口/已建键。
    pub fn set_pending(&mut self, p: Arc<Mutex<PendingPorts>>) {
        self.pending = p;
    }
}

pub struct TunRxToken {
    buffer: Vec<u8>,
}

pub struct TunTxToken<'a, D: TunDevice> {
    device: &'a mut D,
}

impl RxToken for TunRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.buffer)
    }
}

impl<'a, D: TunDevice> TxToken for TunTxToken<'a, D> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        let _ = self.device.write_packet(&buf);
        r
    }
}

impl<D: TunDevice> Device for TunPhy<D> {
    type RxToken<'a> = TunRxToken where Self: 'a;
    type TxToken<'a> = TunTxToken<'a, D> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // 先看重投队列：这些包对应的 socket 现在已建好
        let buf = if let Some(b) = self.retry.pop_front() {
            b
        } else {
            let n = self.inner.read_packet(&mut self.rx_buf).ok()?;
            if n == 0 {
                return None;
            }
            self.rx_buf[..n].to_vec()
        };
        // 学习端口：仅当该 (dst_ip, port) 是首次出现时才延迟交付
        if let Some((proto, dst_ip, dst_port)) = note_pending(&buf, &self.pending) {
            let key = (dst_ip, dst_port);
            let newly_known = {
                let mut g = self.pending.lock().unwrap();
                if g.known.insert(key) {
                    match proto {
                        IpProtocol::Tcp => {
                            g.tcp.insert(key);
                        }
                        IpProtocol::Udp => {
                            g.udp.insert(key);
                        }
                        _ => {}
                    }
                    true
                } else {
                    false
                }
            };
            if newly_known {
                // 该 (dst_ip, port) 的 socket 还没建，先把包压回重投队列，下一轮再交付
                self.retry.push_back(buf);
                return None;
            }
        }
        Some((TunRxToken { buffer: buf }, TunTxToken { device: &mut self.inner }))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(TunTxToken { device: &mut self.inner })
    }
}

/// 一条被接受、等待 Engine 决定出口的 TCP 流。
pub struct NewTcpFlow {
    pub key: FlowKey,
    pub handle: SocketHandle,
}

/// 一个从应用侧收到的 UDP 数据报。
pub struct UdpDatagram {
    pub key: FlowKey,
    pub payload: Vec<u8>,
}

/// smoltcp 接口 + socket 集合。负责终结应用侧连接并把成品流/数据报交给 Engine。
pub struct Netstack<D: TunDevice> {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    pub phy: TunPhy<D>,
    pub local_ip: Ipv4Addr,
    /// 与 phy 共享的待建端口集合与已建键
    pending: Arc<Mutex<PendingPorts>>,
    /// 每个 (目的 IP, 端口) 上仍在监听的 socket（同端口可补多个，支撑并发）
    tcp_listeners: HashMap<(Ipv4Addr, u16), Vec<SocketHandle>>,
    /// 已建立、待搬运的 TCP 流：handle -> 五元组
    tcp_active: HashMap<SocketHandle, FlowKey>,
    /// 每个已学习 (目的 IP, 端口) 上的 UDP socket
    udp_sockets: HashMap<(Ipv4Addr, u16), SocketHandle>,
    /// 本轮新接受的 TCP 流（drain 给 Engine）
    new_tcp: Vec<NewTcpFlow>,
    /// 本轮收到的 UDP 数据报（drain 给 Engine）
    udp_rx: Vec<UdpDatagram>,
}

impl<D: TunDevice> Netstack<D> {
    const TCP_BUF: usize = 64 * 1024;
    const UDP_BUF: usize = 64 * 1024;

    pub fn new(mut phy: TunPhy<D>, local_ip: Ipv4Addr, _mac: [u8; 6]) -> Self {
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = 0x5eed;
        let mut iface = Interface::new(config, &mut phy, Instant::now());
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(
                IpAddress::Ipv4(Ipv4Address::from_bytes(&local_ip.octets())),
                32,
            ));
        });
        // 目的 IP 是真实远端而非本机地址，必须开 any_ip 才会被接收
        iface.set_any_ip(true);
        // Netstack 与 phy 共用同一个 pending：先建好再注入 phy
        let pending = Arc::new(Mutex::new(PendingPorts::default()));
        phy.set_pending(pending.clone());
        let sockets = SocketSet::new(Vec::new());
        Netstack {
            iface,
            sockets,
            phy,
            local_ip,
            pending,
            tcp_listeners: HashMap::new(),
            tcp_active: HashMap::new(),
            udp_sockets: HashMap::new(),
            new_tcp: Vec::new(),
            udp_rx: Vec::new(),
        }
    }

    /// 在指定 (目的 IP, 端口) 上补一个监听 socket。
    fn ensure_tcp_listener(&mut self, ip: Ipv4Addr, port: u16) {
        let rx = tcp::SocketBuffer::new(vec![0u8; Self::TCP_BUF]);
        let tx = tcp::SocketBuffer::new(vec![0u8; Self::TCP_BUF]);
        let mut s = tcp::Socket::new(rx, tx);
        s.listen(listen_endpoint(ip, port)).ok();
        let h = self.sockets.add(s);
        self.tcp_listeners.entry((ip, port)).or_default().push(h);
    }

    /// 在指定 (目的 IP, 端口) 上绑定一个 UDP socket。
    fn ensure_udp_socket(&mut self, ip: Ipv4Addr, port: u16) {
        let rx = udp::PacketBuffer::new(
            vec![udp::PacketMetadata::EMPTY; 64],
            vec![0u8; Self::UDP_BUF],
        );
        let tx = udp::PacketBuffer::new(
            vec![udp::PacketMetadata::EMPTY; 64],
            vec![0u8; Self::UDP_BUF],
        );
        let mut s = udp::Socket::new(rx, tx);
        s.bind(listen_endpoint(ip, port)).ok();
        let h = self.sockets.add(s);
        self.udp_sockets.insert((ip, port), h);
    }

    /// 把上一轮/本轮学到的待建端口全部建成 socket（循环 drain，直到没有新增）。
    fn build_pending_sockets(&mut self) {
        loop {
            let (tcp, udp) = {
                let mut g = self.pending.lock().unwrap();
                (std::mem::take(&mut g.tcp), std::mem::take(&mut g.udp))
            };
            if tcp.is_empty() && udp.is_empty() {
                break;
            }
            for (ip, p) in tcp {
                self.ensure_tcp_listener(ip, p);
            }
            for (ip, p) in udp {
                self.ensure_udp_socket(ip, p);
            }
        }
    }

    /// 一轮协议栈推进：先建 socket -> 驱动 iface -> 再建/再驱动以交付重投首包 -> 收集。
    pub fn poll(&mut self, ts: Instant) {
        // 1) 先按上一轮学到的 (dst_ip, port) 建 socket（while 循环直到 pending 为空）
        self.build_pending_sockets();
        // 2) 驱动协议栈（可能学到新端口并把首包压回重投队列）
        {
            let Netstack { iface, sockets, phy, .. } = self;
            let _ = iface.poll(ts, phy, sockets);
        }
        // 3) 再建一次 + 再驱动一次（交付重投队列里的首包）
        self.build_pending_sockets();
        {
            let Netstack { iface, sockets, phy, .. } = self;
            let _ = iface.poll(ts, phy, sockets);
        }
        // 4) 收集新 accepted / UDP 数据报
        self.collect_tcp();
        self.collect_udp();
    }

    /// 收集已建立的 TCP 连接，并补监听实现同端口多并发。
    fn collect_tcp(&mut self) {
        let mut accepted: Vec<((Ipv4Addr, u16), SocketHandle, FlowKey)> = Vec::new();
        for (map_key, handles) in self.tcp_listeners.iter() {
            let (dst_ip, port) = *map_key;
            for h in handles {
                let s = self.sockets.get::<tcp::Socket>(*h);
                if s.state() != tcp::State::Established {
                    continue;
                }
                if let Some(app) = s.remote_endpoint() {
                    // socket 的远端 = 应用端；本地端口 = 应用连的目的端口 P
                    let app_ip = match app.addr {
                        IpAddress::Ipv4(a) => Ipv4Addr::from(a.0),
                    };
                    let flow = FlowKey::from_parts(
                        Transport::Tcp,
                        SocketAddrV4::new(app_ip, app.port), // local = 应用端
                        SocketAddrV4::new(dst_ip, port),     // remote = 真实远端
                    );
                    accepted.push((*map_key, *h, flow));
                }
            }
        }
        for (map_key, h, flow) in accepted {
            if let Some(list) = self.tcp_listeners.get_mut(&map_key) {
                list.retain(|x| *x != h);
            }
            self.tcp_active.insert(h, flow);
            self.new_tcp.push(NewTcpFlow { key: flow, handle: h });
            // 立即补一个新监听，否则同端口第二个连接会被拒
            self.ensure_tcp_listener(map_key.0, map_key.1);
        }
    }

    /// 收集 UDP 数据报。
    fn collect_udp(&mut self) {
        let targets: Vec<((Ipv4Addr, u16), SocketHandle)> =
            self.udp_sockets.iter().map(|(k, h)| (*k, *h)).collect();
        for (map_key, h) in targets {
            let (dst_ip, port) = map_key;
            loop {
                let s = self.sockets.get_mut::<udp::Socket>(h);
                if !s.can_recv() {
                    break;
                }
                let mut buf = vec![0u8; Self::UDP_BUF];
                match s.recv_slice(&mut buf) {
                    Ok((n, meta)) => {
                        // meta.endpoint = 应用端；绑定端口 port = 应用连的目的端口
                        let app = meta.endpoint;
                        let app_ip = match app.addr {
                            IpAddress::Ipv4(a) => Ipv4Addr::from(a.0),
                        };
                        let flow = FlowKey::from_parts(
                            Transport::Udp,
                            SocketAddrV4::new(app_ip, app.port), // local = 应用端
                            SocketAddrV4::new(dst_ip, port),     // remote = 真实远端
                        );
                        buf.truncate(n);
                        self.udp_rx.push(UdpDatagram { key: flow, payload: buf });
                    }
                    Err(_) => break,
                }
            }
        }
    }

    /// 取走本轮新接受的 TCP 流。
    pub fn take_new_tcp_flows(&mut self) -> Vec<NewTcpFlow> {
        std::mem::take(&mut self.new_tcp)
    }

    /// 取走本轮收到的 UDP 数据报。
    pub fn take_udp_datagrams(&mut self) -> Vec<UdpDatagram> {
        std::mem::take(&mut self.udp_rx)
    }

    /// 回收一条 TCP 流的 socket，避免 SocketSet 无界增长（C4）。
    fn remove_tcp_socket(&mut self, h: SocketHandle) {
        self.sockets.remove(h);
        self.tcp_active.remove(&h);
    }

    /// 在应用侧 socket 与出站流之间双向搬运 TCP 字节。
    ///
    /// 出口 `Router` 只在 macOS 竖切实现，故整个方法随之门控。
    #[cfg(target_os = "macos")]
    pub fn pump_tcp(&mut self, router: &mut super::router::Router) {
        let handles: Vec<SocketHandle> = self.tcp_active.keys().copied().collect();
        for h in handles {
            let key = match self.tcp_active.get(&h) {
                Some(k) => *k,
                None => continue,
            };
            // 应用 -> 出站
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let s = self.sockets.get_mut::<tcp::Socket>(h);
                if !s.can_recv() {
                    break;
                }
                match s.recv_slice(&mut buf) {
                    Ok(n) if n > 0 => {
                        let _ = router.egress_write(&key, &buf[..n]);
                    }
                    _ => break,
                }
            }
            // 出站 -> 应用
            let mut out = vec![0u8; 16 * 1024];
            if let Ok(n) = router.egress_read(&key, &mut out) {
                if n > 0 {
                    let s = self.sockets.get_mut::<tcp::Socket>(h);
                    if s.can_send() {
                        let _ = s.send_slice(&out[..n]);
                    }
                }
            }
            // 应用侧已关闭则回收出站资源与 socket 本体
            let closed = !self.sockets.get::<tcp::Socket>(h).is_open();
            if closed {
                self.sockets.get_mut::<tcp::Socket>(h).abort();
                router.close_flow(&key);
                self.remove_tcp_socket(h);
            }
        }
    }

    /// 因建连失败/规则拒绝而中止一条应用侧 TCP 流。
    pub fn abort_tcp(&mut self, key: &FlowKey) {
        let target: Option<SocketHandle> = self
            .tcp_active
            .iter()
            .find(|(_, k)| **k == *key)
            .map(|(h, _)| *h);
        if let Some(h) = target {
            self.sockets.get_mut::<tcp::Socket>(h).abort();
            self.remove_tcp_socket(h);
        }
    }

    /// 把一个 UDP 回包按五元组写回应用侧 socket。
    ///
    /// socket 绑定在 `key.remote`（真实远端 IP + 应用连的目的端口），回包要发回
    /// `key.local`（应用端），否则应用收不到。
    pub fn send_udp_to_app(&mut self, key: &FlowKey, payload: &[u8]) {
        let lookup = (*key.remote.ip(), key.remote.port());
        if let Some(h) = self.udp_sockets.get(&lookup).copied() {
            let s = self.sockets.get_mut::<udp::Socket>(h);
            if s.can_send() {
                let ep = IpEndpoint::new(
                    IpAddress::Ipv4(Ipv4Address::from_bytes(&key.local.ip().octets())),
                    key.local.port(),
                );
                let _ = s.send_slice(payload, ep);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_key_from_parts_is_stable() {
        let k = FlowKey::from_parts(
            Transport::Tcp,
            "10.0.0.2:40000".parse().unwrap(),
            "93.184.216.34:443".parse().unwrap(),
        );
        assert_eq!(k.remote.port(), 443);
        assert_eq!(k.transport, Transport::Tcp);
    }

    #[test]
    fn accepted_flow_carries_domain_when_fake_ip() {
        let mut ns = NetstackConfig::default();
        ns.fake_domains.insert(0x0A0A_0A0A_u32, "example.com".to_string());
        assert_eq!(ns.domain_for_fake_ip("10.10.10.10"), Some("example.com".to_string()));
        assert_eq!(ns.domain_for_fake_ip("1.1.1.1"), None);
    }

    /// 造一个最小的 IPv4 包：20 字节头 + `payload`。
    fn ipv4_packet(protocol: u8, payload: &[u8]) -> Vec<u8> {
        let total = 20 + payload.len();
        let mut p = vec![0u8; 20];
        p[0] = 0x45; // IPv4, IHL=5
        p[2..4].copy_from_slice(&(total as u16).to_be_bytes());
        p[8] = 64; // TTL
        p[9] = protocol;
        p[12..16].copy_from_slice(&Ipv4Addr::new(10, 9, 0, 2).octets());
        p[16..20].copy_from_slice(&Ipv4Addr::new(93, 184, 216, 34).octets());
        p.extend_from_slice(payload);
        p
    }

    fn tcp_syn(src: u16, dst: u16) -> Vec<u8> {
        let mut tcp = vec![0u8; 20];
        tcp[0..2].copy_from_slice(&src.to_be_bytes());
        tcp[2..4].copy_from_slice(&dst.to_be_bytes());
        tcp[12] = 0x50; // data offset = 5
        tcp[13] = 0x02; // SYN
        ipv4_packet(6, &tcp)
    }

    #[test]
    fn tcp_syn_reports_protocol_dst_ip_and_port() {
        let pkt = tcp_syn(12345, 443);
        let pending = Arc::new(Mutex::new(PendingPorts::default()));
        let got = note_pending(&pkt, &pending);
        assert_eq!(
            got,
            Some((IpProtocol::Tcp, Ipv4Addr::new(93, 184, 216, 34), 443)),
            "SYN 应返回 (Tcp, 目的 IP, 目的端口)"
        );
        // note_pending 只做解析，不写状态
        let g = pending.lock().unwrap();
        assert!(g.tcp.is_empty() && g.udp.is_empty() && g.known.is_empty());
    }

    #[test]
    fn tcp_syn_ack_is_ignored() {
        let mut tcp = vec![0u8; 20];
        tcp[2..4].copy_from_slice(&443u16.to_be_bytes());
        tcp[12] = 0x50;
        tcp[13] = 0x12; // SYN + ACK
        let pkt = ipv4_packet(6, &tcp);

        let pending = Arc::new(Mutex::new(PendingPorts::default()));
        assert_eq!(note_pending(&pkt, &pending), None);
    }

    #[test]
    fn udp_reports_protocol_dst_ip_and_port() {
        let mut udp = vec![0u8; 8];
        udp[0..2].copy_from_slice(&5000u16.to_be_bytes()); // src
        udp[2..4].copy_from_slice(&53u16.to_be_bytes()); // dst
        udp[4..6].copy_from_slice(&8u16.to_be_bytes()); // len
        let pkt = ipv4_packet(17, &udp);

        let pending = Arc::new(Mutex::new(PendingPorts::default()));
        let got = note_pending(&pkt, &pending);
        assert_eq!(
            got,
            Some((IpProtocol::Udp, Ipv4Addr::new(93, 184, 216, 34), 53)),
            "UDP 应返回 (Udp, 目的 IP, 目的端口)"
        );
    }

    #[test]
    fn truncated_packet_is_ignored() {
        let pending = Arc::new(Mutex::new(PendingPorts::default()));
        assert_eq!(note_pending(&[0x45, 0x00], &pending), None);
    }

    /// 假设备：按队列吐包，写包丢弃。
    struct MockDev {
        rx: VecDeque<Vec<u8>>,
    }

    impl MockDev {
        fn new(pkts: Vec<Vec<u8>>) -> Self {
            MockDev { rx: pkts.into() }
        }
    }

    impl TunDevice for MockDev {
        fn name(&self) -> &str {
            "mock0"
        }
        fn index(&self) -> u32 {
            0
        }
        fn read_packet(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.rx.pop_front() {
                Some(p) => {
                    let n = p.len().min(buf.len());
                    buf[..n].copy_from_slice(&p[..n]);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
        fn write_packet(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn new_port_first_packet_is_held_then_retried() {
        let pkt = tcp_syn(40000, 443);
        let mut phy = TunPhy::new(MockDev::new(vec![pkt.clone()]), 1500);
        let pending = Arc::new(Mutex::new(PendingPorts::default()));
        phy.set_pending(pending.clone());

        // 首次 receive：发现新端口，(dst_ip, port) 尚需建 socket，本轮不交付
        assert!(phy.receive(Instant::now()).is_none(), "新端口首包本轮不交付");
        {
            let g = pending.lock().unwrap();
            let key = (Ipv4Addr::new(93, 184, 216, 34), 443);
            assert!(g.known.contains(&key), "端口应被标记为已发现");
            assert!(g.tcp.contains(&key), "待建监听队列应含该端口");
        }
        // 再次 receive：从重投队列取回，端口已 known，正常交付
        let second = phy.receive(Instant::now());
        assert!(second.is_some(), "下一轮应交付重投包");
        let (rx, _tx) = second.unwrap();
        let got = rx.consume(|b| b.to_vec());
        assert_eq!(got, pkt, "交付的应是原首包");
    }
}
