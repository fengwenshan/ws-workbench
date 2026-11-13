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
    let mut senders_at = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        // 网络切换（拔网线、连 / 断 VPN）后重新枚举一次发送网卡
        if senders_at.elapsed() >= Duration::from_secs(60) {
            senders = announce_senders();
            senders_at = Instant::now();
        }
        let due = last_announce
            .map(|at| at.elapsed() >= ANNOUNCE_INTERVAL)
            .unwrap_or(true);
        if due {
            send_announce(&inner, &senders, http_port, group_addr);
            last_announce = Some(Instant::now());
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
                if changed && may_reply {
                    send_announce(&inner, &senders, http_port, group_addr);
                    last_reply.insert(announce.fingerprint, now);
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
/// 绑到网卡自己的 IPv4 上，组播的出口就固定在这块网卡（源地址选路），
/// 不依赖系统默认路由——开着全局 VPN 时默认路由在 utun 上，LAN 上的设备会看不到我们。
fn announce_senders() -> Vec<UdpSocket> {
    let mut sockets = Vec::new();
    for address in local_ipv4_addresses() {
        let Ok(ip) = address.parse::<Ipv4Addr>() else {
            continue;
        };
        if let Ok(socket) = UdpSocket::bind((ip, 0)) {
            let _ = socket.set_multicast_loop_v4(true);
            let _ = socket.set_multicast_ttl_v4(1);
            sockets.push(socket);
        }
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

fn send_announce(
    inner: &InnerRef,
    senders: &[UdpSocket],
    http_port: u16,
    group: SocketAddr,
) {
    let announce = {
        let guard = lock_inner(inner);
        Announce::new(
            guard.effective_alias(),
            guard.settings.fingerprint.clone(),
            http_port,
        )
    };
    let payload = announce.to_json();
    for socket in senders {
        // 单块网卡发送失败（网卡刚断开等）不影响其它网卡
        let _ = socket.send_to(payload.as_bytes(), group);
    }
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
