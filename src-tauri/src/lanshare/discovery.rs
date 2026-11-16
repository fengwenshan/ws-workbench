//! UDP 组播发现：周期性 announce、peer 表维护、异常掉线的过期清理。
//!
//! 两个方向都用同一个 socket：
//! - 每 3 秒向 `224.0.0.167:53317` 发一次 announce（TTL=1，只在本网段）；
//! - 收到别人的 announce 时更新 peer 表；新设备 / 信息变化时立即回一次，
//!   让双方都能在一两秒内看到对方（回包按 fingerprint 限速 1 秒）。
//!
//! 读超时 500ms + `AtomicBool` 停止标记，停止时最多等半秒就能退出。

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use super::protocol::Announce;
use super::{
    emit_snapshot, local_ipv4_addresses, lock_inner, unix_millis, InnerRef, ANNOUNCE_INTERVAL,
    DISCOVERY_PORT, MULTICAST_GROUP,
};

/// 发现线程的句柄：Drop 时置停止标记并 join。
pub(crate) struct DiscoveryHandle {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DiscoveryHandle {
    /// 绑定组播 socket 并启动发现线程；`http_port` 写进 announce 给对端回连。
    pub(crate) fn start(
        inner: InnerRef,
        http_port: u16,
        app: Option<AppHandle>,
    ) -> Result<Self, String> {
        let socket = bind_socket()?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || {
            run(inner, socket, http_port, thread_stop, app);
        });
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for DiscoveryHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 绑定组播 socket 并加入本机所有 IPv4 网卡的组播组。
///
/// 用 socket2 显式开 `SO_REUSEADDR`：同机同时跑开发版 + 安装版（或测试脚本）时，
/// 两个 socket 都能绑 53317 并各自收到组播；std 的 `UdpSocket::bind` 没有这个选项。
fn bind_socket() -> Result<UdpSocket, String> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|err| format!("创建局域网发现 socket 失败：{err}"))?;
    socket
        .set_reuse_address(true)
        .map_err(|err| format!("设置局域网发现端口复用失败：{err}"))?;
    // macOS / BSD 上 UDP 端口复用还要 SO_REUSEPORT（Node 的 dgram reuseAddr 也是两个都设）；
    // Windows 没有这个选项，失败忽略即可。
    #[cfg(unix)]
    let _ = socket.set_reuse_port(true);
    socket
        .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).into())
        .map_err(|err| format!("局域网发现端口 {DISCOVERY_PORT} 绑定失败：{err}"))?;
    let socket: UdpSocket = socket.into();

    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|err| format!("设置发现端口读超时失败：{err}"))?;
    // 同机调试（开发版 + 安装版）能收到自己发的 announce；业务侧按 fingerprint 忽略自己
    let _ = socket.set_multicast_loop_v4(true);
    let _ = socket.set_multicast_ttl_v4(1);

    let group = Ipv4Addr::from(MULTICAST_GROUP);
    let mut joined = false;
    for address in local_ipv4_addresses() {
        if let Ok(ip) = address.parse::<Ipv4Addr>() {
            if socket.join_multicast_v4(&group, &ip).is_ok() {
                joined = true;
            }
        }
    }
    if !joined {
        // 拿不到网卡列表时退回默认接口再试一次；失败也不直接报错，
        // 单机（无网卡）场景下仍允许启动，只是发现不到别人。
        let _ = socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED);
    }
    Ok(socket)
}

fn run(
    inner: InnerRef,
    socket: UdpSocket,
    http_port: u16,
    stop: Arc<AtomicBool>,
    app: Option<AppHandle>,
) {
    let group_addr = SocketAddr::from((MULTICAST_GROUP, DISCOVERY_PORT));
    let mut buf = [0u8; 2048];
    let mut last_announce: Option<Instant> = None;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut dirty = false;
    // 回包限速：fingerprint → 上次回 announce 的时间
    let mut last_reply: HashMap<String, Instant> = HashMap::new();
    // 发送 announce 用的 socket 按网卡各建一个：默认路由可能被 VPN（utun）占着，
    // 只靠一个 socket 发可能根本出不了物理网卡。按本机 IPv4 各绑一个发送源，
    // 保证每个物理 / 隧道网卡上都能发出组播（接收仍走上面绑定 53317 的 socket）。
    let mut senders = announce_senders();
    let mut addresses = local_ipv4_addresses();
    let mut senders_at = Instant::now();
    // 单播回包 / 心跳用的 socket：不绑网卡，让系统按目的地址选出口
    // （组播在 VPN、防火墙场景下经常只有一个方向通，单播能补上另一个方向）
    let unicast = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok();

    while !stop.load(Ordering::Relaxed) {
        // 网络切换（拔网线、连 / 断 VPN）后重新枚举一次发送网卡
        if senders_at.elapsed() >= Duration::from_secs(60) {
            senders = announce_senders();
            addresses = local_ipv4_addresses();
            senders_at = Instant::now();
        }
        let due = last_announce
            .map(|at| at.elapsed() >= ANNOUNCE_INTERVAL)
            .unwrap_or(true);
        if due {
            send_announce(&inner, &senders, &addresses, http_port, group_addr);
            last_announce = Some(Instant::now());
            // 对已知设备再单播一遍：组播只有一个方向通时，对面也能靠这些包续上 last_seen
            if let Some(socket) = unicast.as_ref() {
                for addr in known_peer_addrs(&inner) {
                    send_announce_to(&inner, socket, addr, http_port, &addresses);
                }
            }
        }

        match socket.recv_from(&mut buf) {
            Ok((read, source)) => {
                let announce = match Announce::from_json(&buf[..read]) {
                    Ok(announce) if announce.is_valid() => announce,
                    _ => continue,
                };
                let self_fingerprint = lock_inner(&inner).settings.fingerprint.clone();
                if announce.fingerprint == self_fingerprint {
                    continue;
                }
                let now = Instant::now();
                let peer_addr = SocketAddr::new(source.ip(), announce.port);
                let changed = {
                    let mut guard = lock_inner(&inner);
                    guard.upsert_peer(&announce, peer_addr, now, unix_millis())
                };
                dirty |= changed;

                // 新设备或信息变化时立刻回一次，加速互相发现；同一设备 1 秒内只回一次
                let may_reply = last_reply
                    .get(&announce.fingerprint)
                    .map(|at| at.elapsed() >= Duration::from_secs(1))
                    .unwrap_or(true);
                if changed {
                    // 挨个探测它的地址，把连得上的那个写进 peer 表（开 VPN 时来源地址常常连不上）
                    let resolved =
                        resolve_peer_address(&inner, app.as_ref(), &announce.fingerprint);
                    if may_reply {
                        // 除了组播回包，再用单播直接回给探测成功的那个地址：
                        // 组播被单向拦住时，对面只有靠这一包才能看见我们
                        if let (Some(socket), Some(addr)) = (unicast.as_ref(), resolved) {
                            send_announce_to(&inner, socket, addr, http_port, &addresses);
                        }
                        send_announce(&inner, &senders, &addresses, http_port, group_addr);
                        last_reply.insert(announce.fingerprint.clone(), now);
                    }
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }

        // peer 过期清理放在每轮循环，成本很低（peer 数量是局域网设备数）
        let pruned = {
            let mut guard = lock_inner(&inner);
            guard.prune_peers(Instant::now())
        };
        dirty |= pruned;

        // 快照事件限速 ≤1 次/秒，避免 announce 风暴把事件通道刷爆
        if dirty && last_emit.elapsed() >= Duration::from_secs(1) {
            emit_snapshot(&inner, app.as_ref());
            dirty = false;
            last_emit = Instant::now();
        }
    }

    // 退出前再推一次，面板能立刻看到「设备已清空」
    emit_snapshot(&inner, app.as_ref());
}

/// 为每块本机网卡建一个 announce 发送 socket。
///
/// 绑到网卡自己的 IPv4 上，并且显式设 `IP_MULTICAST_IF`：只绑源地址不够——
/// Windows 发组播是按路由表选出口的，开着 VPN 时 `224.0.0.0/4` 会走虚拟网卡，
/// announce 根本出不了物理网卡（对端也就永远看不到我们）。
fn announce_senders() -> Vec<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let mut sockets = Vec::new();
    for address in local_ipv4_addresses() {
        let Ok(ip) = address.parse::<Ipv4Addr>() else {
            continue;
        };
        let Ok(socket) = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) else {
            continue;
        };
        if socket.bind(&SocketAddr::from((ip, 0)).into()).is_err() {
            continue;
        }
        // 出口网卡固定到这块网卡，绕开 VPN 抢走组播路由的问题
        let _ = socket.set_multicast_if_v4(&ip);
        let _ = socket.set_multicast_loop_v4(true);
        let _ = socket.set_multicast_ttl_v4(1);
        sockets.push(socket.into());
    }
    if sockets.is_empty() {
        // 枚举不到网卡时退回默认出口，至少保证单机 / 特殊网卡环境下还能发
        if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
            let _ = socket.set_multicast_loop_v4(true);
            let _ = socket.set_multicast_ttl_v4(1);
            sockets.push(socket);
        }
    }
    sockets
}

/// 探测一台设备的所有候选地址，把能连上的写回 peer 表（面板上显示的就是它），并返回该地址。
///
/// 放在发现线程里同步做：只在设备新出现 / 地址表变化时触发，最多阻塞一两秒。
fn resolve_peer_address(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    fingerprint: &str,
) -> Option<SocketAddr> {
    let peer = lock_inner(inner).peer_by_fingerprint(fingerprint)?;
    let send_peer = super::client::SendPeer {
        fingerprint: peer.fingerprint.clone(),
        alias: peer.alias.clone(),
        ip: peer.ip.clone(),
        port: peer.port,
        addresses: peer.addresses.clone(),
    };
    let resolved = super::client::resolve_peer(&send_peer, inner, app).ok()?;
    format!("{}:{}", resolved.ip, resolved.port)
        .parse::<SocketAddr>()
        .ok()
}

/// 已知设备当前选定的地址（单播心跳用）
fn known_peer_addrs(inner: &InnerRef) -> Vec<SocketAddr> {
    let guard = lock_inner(inner);
    let mut addrs: Vec<SocketAddr> = guard
        .peers
        .values()
        .filter_map(|entry| {
            format!("{}:{}", entry.peer.ip, entry.peer.port)
                .parse()
                .ok()
        })
        .collect();
    addrs.sort();
    addrs.dedup();
    addrs
}

fn announce_payload(inner: &InnerRef, http_port: u16, addresses: &[String]) -> String {
    let guard = lock_inner(inner);
    Announce::new(
        guard.effective_alias(),
        guard.settings.fingerprint.clone(),
        http_port,
    )
    .with_addresses(addresses.to_vec())
    .to_json()
}

fn send_announce(
    inner: &InnerRef,
    senders: &[UdpSocket],
    addresses: &[String],
    http_port: u16,
    group: SocketAddr,
) {
    let payload = announce_payload(inner, http_port, addresses);
    for socket in senders {
        // 单块网卡发送失败（网卡刚断开等）不影响其它网卡
        let _ = socket.send_to(payload.as_bytes(), group);
    }
}

/// 单播一份 announce 给某个已知地址（补组播只有一个方向通的情况）
fn send_announce_to(
    inner: &InnerRef,
    socket: &UdpSocket,
    addr: SocketAddr,
    http_port: u16,
    addresses: &[String],
) {
    let payload = announce_payload(inner, http_port, addresses);
    let _ = socket.send_to(payload.as_bytes(), addr);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multicast_group_is_lan_only() {
        // 224.0.0.0/4 第一个字节的高 4 位是 1110；TTL=1 由 socket 选项保证
        assert_eq!(MULTICAST_GROUP[0] & 0xF0, 0xE0);
        assert_eq!(DISCOVERY_PORT, 53317);
        assert_eq!(super::super::PROTOCOL_VERSION, 1);
    }

    #[test]
    fn announce_produced_by_runtime_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::LanShareState::new(dir.path().join("recv"));
        let announce = {
            let guard = lock_inner(&state.inner);
            Announce::new(
                guard.effective_alias(),
                guard.settings.fingerprint.clone(),
                53317,
            )
        };
        assert!(announce.is_valid());
        assert_eq!(announce.port, 53317);
        assert!(!announce.alias.trim().is_empty());
        assert!(!announce.fingerprint.trim().is_empty());
    }
}
