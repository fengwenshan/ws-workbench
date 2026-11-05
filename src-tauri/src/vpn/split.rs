//! 路由注入式分流（规则模式，macOS）：不捕获、不抢默认路由、不改系统 DNS。
//!
//! 为什么不捕获：捕获式接管（抢 `0.0.0.0/1` + `128.0.0.0/1`）会让 macOS 的
//! 系统解析器（mDNSResponder）发不出 DNS 查询 —— 它把查询 socket 钉在物理网卡上，
//! 半默认路由一抢，`sendto` 先报 `[51] Network is unreachable`，给 DNS 加 /32
//! 旁路后又变成 `[49] Can't assign requested address`。系统解析器一废，所有走它的
//! 应用集体断网。真机两轮日志证明「捕获式接管」和「系统解析器」在这类机器上不能共存
//! （详见设计文档根因九），所以规则模式改成路由注入：
//!
//! 1. 命中「走 VPN」的域名写进 `/etc/resolver/<域名>`（macOS 官方的按域名分流解析机制），
//!    系统解析器便会把这些域名的查询发给 `127.0.0.1:53`；
//! 2. 本模块在 worker 里起一个极简 DNS 转发器监听 `127.0.0.1:53`，把查询**经隧道**
//!    送给 VPN 下发的 DNS，拿到真实内网 IP；
//! 3. 顺手给这些内网 IP 装 `/32` 主机路由指向隧道，应用随后连过去就直接进隧道；
//! 4. 命中 `IP-CIDR` 规则的网段也直接注入隧道；
//! 5. 默认路由、系统 DNS、其它流量一律不动 —— 结构上不可能再出现「一连上就断网」。
//!
//! 代价（必须让用户知道）：
//! - `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` 没有数据面可用，做不到，会让用户改规则；
//! - 域名只覆盖「系统解析器（getaddrinfo）」这条路：自带 DoH/DoT 的应用不受控
//!   （但它解析出来的 IP 若命中 IP-CIDR 规则，照样能进隧道）。

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::dns_client;
use super::models::VpnRule;
use super::route;
use super::rule_engine::{Policy, RuleKind};
use super::session::VpnSession;

/// 本机 DNS 转发器监听地址。
///
/// 用 53 而不是别的端口：`/etc/resolver` 里只写 `nameserver 127.0.0.1` 最稳，
/// worker 以 root 运行，绑 1024 以下端口没问题。
pub const FORWARDER_ADDR: &str = "127.0.0.1:53";
/// `/etc/resolver` 目录（macOS 的按域名分流解析配置目录）
pub const RESOLVER_DIR: &str = "/etc/resolver";
/// 我们写过的 resolver 文件名清单：进程被 SIGKILL 时靠它在下次连接前清干净
pub const MANIFEST_FILE: &str = "resolver-manifest.txt";
/// 单次上游查询的超时
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(2);
/// 转发线程的收包粒度：只为了让 stop 标志能及时生效
const POLL_TIMEOUT: Duration = Duration::from_millis(500);

// ============================== 规则 → 计划 ==============================

/// 规则翻译成「路由注入模式能做的事」。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SplitPlan {
    /// 需要写 `/etc/resolver` 的域名（小写、去通配符）
    pub domains: Vec<String>,
    /// 直接注入隧道的网段（`IP-CIDR` 规则）
    pub cidrs: Vec<String>,
    /// 这个模式下做不到的规则，以及需要用户知道的行为差异
    pub notes: Vec<String>,
}

/// 把规则集翻译成路由注入计划。
///
/// 只有「走 VPN」的规则需要动作：「直连」规则在这个模式里天然满足（默认就是直连），
/// 不用翻译，也不用提示。
pub fn plan(rules: &[VpnRule], default_policy: &str) -> SplitPlan {
    let mut out = SplitPlan::default();
    for rule in rules {
        if !rule.enabled {
            continue;
        }
        if !matches!(Policy::parse(&rule.policy), Ok(Policy::Vpn)) {
            continue;
        }
        let kind = match RuleKind::parse(&rule.kind) {
            Ok(kind) => kind,
            Err(e) => {
                push_unique(&mut out.notes, format!("{} {}：{}", rule.kind, rule.value, e));
                continue;
            }
        };
        match kind {
            RuleKind::Domain | RuleKind::DomainSuffix => match normalize_domain(&rule.value) {
                Some(domain) => push_unique(&mut out.domains, domain),
                None => push_unique(
                    &mut out.notes,
                    format!("{} {} 不是合法域名，已跳过", kind.as_str(), rule.value),
                ),
            },
            RuleKind::IpCidr => match route::parse_cidr(&rule.value) {
                Ok((net, prefix)) => {
                    push_unique(
                        &mut out.cidrs,
                        format!("{}/{}", Ipv4Addr::from(net), prefix),
                    );
                }
                Err(e) => push_unique(
                    &mut out.notes,
                    format!("IP-CIDR {} 无法解析（{}），已跳过", rule.value, e),
                ),
            },
        }
    }
    if matches!(Policy::parse(default_policy), Ok(Policy::Vpn)) {
        out.notes.push(
            "兜底策略是 VPN：路由注入模式不抢默认路由，未命中的流量仍然直连；\
             需要「全部走 VPN」请用全局模式"
                .to_string(),
        );
    }
    out
}

impl SplitPlan {
    /// 并入一个域名（服务端下发的默认域名、规则里的域名后缀都走这里）。
    /// 返回是否真的新增了。
    pub fn add_domain(&mut self, raw: &str) -> bool {
        match normalize_domain(raw) {
            Some(domain) => add_unique(&mut self.domains, domain),
            None => false,
        }
    }

    /// 并入一个网段（服务端下发的 split-include 也走这里，校验/归一化一致）。
    /// 返回是否真的新增了。
    pub fn add_cidr(&mut self, raw: &str) -> bool {
        match route::parse_cidr(raw) {
            Ok((net, prefix)) => add_unique(&mut self.cidrs, format!("{}/{}", Ipv4Addr::from(net), prefix)),
            Err(_) => false,
        }
    }
}

fn add_unique(list: &mut Vec<String>, item: String) -> bool {
    if list.contains(&item) {
        return false;
    }
    list.push(item);
    true
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// 域名规范化：小写、去掉 `*.` 前缀和首尾点，校验标签。
///
/// 校验同时也是安全边界：这些字符串会变成 `/etc/resolver/<域名>` 的文件名，
/// 必须挡掉 `/`、`.` 开头之类的路径穿越。
pub fn normalize_domain(value: &str) -> Option<String> {
    let mut text = value.trim().to_ascii_lowercase();
    text = text.trim_start_matches("*.").to_string();
    text = text.trim_start_matches('.').trim_end_matches('.').to_string();
    if text.is_empty() || text.len() > 253 || !text.contains('.') {
        return None;
    }
    for label in text.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        let ok = label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return None;
        }
    }
    Some(text)
}

// ============================== DNS 报文小工具 ==============================

/// 把一条 DNS 查询改写成 SERVFAIL 应答（上游全都不通时用）。
///
/// 只保留头部与问题段：应用需要收到一个「明确失败」的应答，否则它会一直等到超时。
pub fn servfail(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let question_end = question_end(query)?;
    let mut resp = Vec::with_capacity(question_end);
    resp.extend_from_slice(&query[..question_end]);
    // QR=1，RD 保留，RCODE=2(SERVFAIL)
    resp[2] = 0x81;
    resp[3] = 0x82;
    // QDCOUNT 保留，AN/NS/AR 清零
    resp[6..12].fill(0);
    if question_end >= 12 {
        resp[4..6].copy_from_slice(&1u16.to_be_bytes());
    }
    Some(resp)
}

/// 问题段结束位置（含 QTYPE/QCLASS 四个字节）。
fn question_end(packet: &[u8]) -> Option<usize> {
    let mut pos = 12usize;
    loop {
        let len = *packet.get(pos)?;
        match len {
            0 => {
                pos += 5; // 0 结尾 + QTYPE(2) + QCLASS(2)
                return if pos <= packet.len() { Some(pos) } else { None };
            }
            0xC0..=0xFF => return None, // 问题段不该有压缩指针
            _ => {
                pos += 1 + len as usize;
                if pos >= packet.len() {
                    return None;
                }
            }
        }
    }
}

// ============================== 转发器 + 路由注入 ==============================

/// 路由注入分流的运行时状态：DNS 转发线程 + 我们装出去的路由 + resolver 文件。
pub struct SplitTunnel {
    domains: Vec<String>,
    tun_dev: String,
    vpn_dns: Vec<Ipv4Addr>,
    /// 固定要维护的路由（VPN DNS + `IP-CIDR` 规则）
    static_routes: Vec<String>,
    /// 从 DNS 应答里学到的内网 IP（动态注入，断开时按它回收）
    dynamic: Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    resolver_files: Vec<PathBuf>,
    manifest: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SplitTunnel {
    /// 装好路由注入分流。任何一步失败都会先把已做的动作回滚，再返回错误。
    pub fn apply(
        session: &VpnSession,
        tun_dev: &str,
        vpn_dns: &[Ipv4Addr],
        plan: SplitPlan,
    ) -> Result<Self, String> {
        if plan.domains.is_empty() && plan.cidrs.is_empty() {
            return Err("规则里没有可用的分流动作：域名请用「域名后缀」，网段请用 IP-CIDR".to_string());
        }
        // 上一轮被 SIGKILL 时留下的 resolver 文件先清掉，否则清不干净的域名
        // 会一直指向一个没人监听的 127.0.0.1:53。
        cleanup_stale(session);

        // 只有域名规则需要 `/etc/resolver` + 本机转发器；纯 IP-CIDR 分流不碰 53 端口，
        // 免得白白和别的 DNS 服务撞端口。
        let need_dns = !plan.domains.is_empty();
        if need_dns && vpn_dns.is_empty() {
            return Err("服务端没有下发 DNS 地址，内网域名无法解析".to_string());
        }
        let mut me = SplitTunnel {
            domains: plan.domains.clone(),
            tun_dev: tun_dev.to_string(),
            vpn_dns: vpn_dns.to_vec(),
            static_routes: Vec::new(),
            dynamic: Arc::new(Mutex::new(BTreeSet::new())),
            resolver_files: Vec::new(),
            manifest: session.dir.join(MANIFEST_FILE),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        };

        if let Err(e) = me.install_routes(&plan.cidrs) {
            me.teardown();
            return Err(e);
        }
        if need_dns {
            if let Err(e) = me.write_resolver_files() {
                me.teardown();
                return Err(e);
            }
            if let Err(e) = me.start_forwarder(session) {
                me.teardown();
                return Err(e);
            }
            flush_dns_cache();
        }
        Ok(me)
    }

    /// VPN DNS + IP-CIDR 规则：装成隧道的固定路由。
    fn install_routes(&mut self, cidrs: &[String]) -> Result<(), String> {
        let mut wanted: Vec<String> = Vec::new();
        for ip in &self.vpn_dns {
            wanted.push(format!("{}/32", ip));
        }
        wanted.extend(cidrs.iter().cloned());
        for cidr in &wanted {
            add_tunnel_route(cidr, &self.tun_dev)?;
            if !self.static_routes.contains(cidr) {
                self.static_routes.push(cidr.clone());
            }
        }
        Ok(())
    }

    /// 写 `/etc/resolver/<域名>`：让系统解析器把这些域名交给本机转发器。
    fn write_resolver_files(&mut self) -> Result<(), String> {
        dir_create(RESOLVER_DIR)?;
        let mut written: Vec<PathBuf> = Vec::new();
        for domain in &self.domains {
            let path = Path::new(RESOLVER_DIR).join(domain);
            if let Err(e) = std::fs::write(&path, resolver_file_content()) {
                for p in &written {
                    let _ = std::fs::remove_file(p);
                }
                return Err(format!(
                    "写 {} 失败：{}（worker 需要 root 权限）",
                    path.display(),
                    e
                ));
            }
            written.push(path);
        }
        self.resolver_files = written;
        // 清单落盘：崩溃后下次连接前能清理干净
        let mut text = String::new();
        for domain in &self.domains {
            text.push_str(domain);
            text.push('\n');
        }
        if let Err(e) = std::fs::write(&self.manifest, text) {
            return Err(format!("写清单 {} 失败：{}", self.manifest.display(), e));
        }
        Ok(())
    }

    /// 起 DNS 转发线程（监听 127.0.0.1:53）。
    fn start_forwarder(&mut self, session: &VpnSession) -> Result<(), String> {
        let socket = UdpSocket::bind(FORWARDER_ADDR).map_err(|e| {
            format!(
                "绑定 {} 失败：{}（端口被其它 DNS 服务占用？）",
                FORWARDER_ADDR, e
            )
        })?;
        socket.set_read_timeout(Some(POLL_TIMEOUT)).map_err(|e| e.to_string())?;
        let upstreams: Vec<SocketAddrV4> = self
            .vpn_dns
            .iter()
            .map(|ip| SocketAddrV4::new(*ip, 53))
            .collect();
        let tun_dev = self.tun_dev.clone();
        let dynamic = Arc::clone(&self.dynamic);
        let stop = Arc::clone(&self.stop);
        let log = session.clone();
        let handle = std::thread::Builder::new()
            .name("ws-dns-forwarder".to_string())
            .spawn(move || forward_loop(socket, upstreams, tun_dev, dynamic, stop, log))
            .map_err(|e| format!("启动 DNS 转发线程失败：{}", e))?;
        self.thread = Some(handle);
        Ok(())
    }

    /// 后台自检：按应用真实路径，经系统解析器解析规则里的域名。
    ///
    /// 这条链路是 `/etc/resolver` -> 本机转发器 -> 隧道 -> VPN DNS，
    /// 任一段断了都能在 worker.log 里一眼看出来。
    pub fn spawn_self_check(&self, session: VpnSession) {
        let domains = self.domains.clone();
        let dynamic = Arc::clone(&self.dynamic);
        if domains.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            session.append_log("[自检] 开始：系统解析器 -> /etc/resolver -> 本机转发器 -> 隧道 -> VPN DNS");
            for domain in domains.iter().take(3) {
                let name = domain.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    use std::net::ToSocketAddrs;
                    let result = (name.as_str(), 443u16).to_socket_addrs().map(|it| {
                        it.map(|a| a.ip().to_string()).collect::<Vec<_>>()
                    });
                    let _ = tx.send(result);
                });
                match rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(Ok(ips)) if !ips.is_empty() => {
                        let routed = ips.iter().any(|ip| {
                            ip.parse::<Ipv4Addr>()
                                .map(|v4| dynamic.lock().map(|set| set.contains(&v4)).unwrap_or(false))
                                .unwrap_or(false)
                        });
                        session.append_log(&format!(
                            "[自检] {} 解析到 {}（{}）",
                            domain,
                            ips.join(", "),
                            if routed {
                                "已按隧道路由接管，连接会走 VPN"
                            } else {
                                "还没见到隧道路由：若连接失败，看上面 [分流] 注入日志"
                            }
                        ));
                    }
                    Ok(Ok(_)) => session.append_log(&format!(
                        "[自检] {} 的查询已走到本机转发器，但这个域名本身没有 A 记录\
                         （内网主机请用完整域名，比如 oa.example.com）",
                        domain
                    )),
                    Ok(Err(e)) => {
                        // 「nodename nor servname」= 这个名字本身没有 A 记录（NXDOMAIN/无记录），
                        // 说明查询链路是通的；「Temporary failure / timed out」才是链路真不通。
                        let text = e.to_string();
                        let no_record = text.contains("nodename")
                            || text.contains("no such host")
                            || text.contains("Name or service not known");
                        if no_record {
                            session.append_log(&format!(
                                "[自检] {domain} 查询链路是通的，但这个名字本身没有 A 记录\
                                 （内网主机请用完整域名，比如 oa.{domain}）；原始错误：{text}"
                            ));
                        } else {
                            session.append_log(&format!(
                                "[自检] {domain} 解析失败：{text}（转发器或 VPN DNS 可能不通，看上面 [分流] 转发日志）"
                            ));
                        }
                    }
                    Err(_) => session.append_log(&format!(
                        "[自检] {} 解析超时：5 秒没有结果，转发器或 VPN DNS 可能不通",
                        domain
                    )),
                }
            }
        });
    }

    /// 路由守护：隧道路由被别的东西改掉时补回来，resolver 文件被删时补回来。
    pub fn guard(&self) {
        #[cfg(target_os = "macos")]
        {
            let target = route::RouteTarget::new(0, &self.tun_dev, "");
            for cidr in &self.static_routes {
                // 路由还在就不动它：删了重装会让匹配这条路由的流量有一瞬间掉回默认路由
                if route::route_exists(cidr, &target).unwrap_or(true) {
                    continue;
                }
                // 不在就补回来；单条失败不吵，下一次守护再试
                let _ = add_tunnel_route(cidr, &self.tun_dev);
            }
        }
        for path in &self.resolver_files {
            let missing = std::fs::metadata(path).is_err();
            if missing {
                let _ = std::fs::write(path, resolver_file_content());
            }
        }
    }

    /// 收工时回收一切：线程、动态路由、固定路由、resolver 文件、清单。
    pub fn teardown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        let dynamic: Vec<Ipv4Addr> = self
            .dynamic
            .lock()
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default();
        for ip in dynamic {
            let _ = del_tunnel_route(&format!("{}/32", ip), &self.tun_dev);
        }
        for cidr in &self.static_routes {
            let _ = del_tunnel_route(cidr, &self.tun_dev);
        }
        for path in &self.resolver_files {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&self.manifest);
        self.resolver_files.clear();
        self.static_routes.clear();
        if let Ok(mut set) = self.dynamic.lock() {
            set.clear();
        }
        flush_dns_cache();
    }

    pub fn domain_count(&self) -> usize {
        self.domains.len()
    }

    pub fn static_route_count(&self) -> usize {
        self.static_routes.len()
    }

    pub fn routed_ip_count(&self) -> usize {
        self.dynamic.lock().map(|set| set.len()).unwrap_or(0)
    }

    pub fn tun_dev(&self) -> &str {
        &self.tun_dev
    }

    /// 给界面看的「当前生效路由」列表（只列固定注入的隧道路由；
    /// 按解析结果动态注入的 /32 会很多，不往状态里塞）。
    pub fn routes_for_status(&self) -> Vec<super::models::AppliedRoute> {
        self.static_routes
            .iter()
            .map(|cidr| super::models::AppliedRoute {
                cidr: cidr.clone(),
                via: "vpn".to_string(),
                next_hop: String::new(),
                iface: self.tun_dev.clone(),
                owned: true,
            })
            .collect()
    }
}

/// `/etc/resolver/<域名>` 的内容：把所有查询交给本机转发器。
pub fn resolver_file_content() -> &'static str {
    "nameserver 127.0.0.1\n"
}

fn dir_create(dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{}", dir, e))
}

/// 清掉上一次崩溃留下的 resolver 文件。
///
/// 只删清单里记过、且内容还是我们写的 `nameserver 127.0.0.1` 的文件 ——
/// 绝不能碰用户自己配的 `/etc/resolver`。
pub fn cleanup_stale(session: &VpnSession) {
    let manifest = session.dir.join(MANIFEST_FILE);
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return;
    };
    let mut removed = 0usize;
    for domain in text.lines() {
        let domain = domain.trim();
        if domain.is_empty() {
            continue;
        }
        let path = Path::new(RESOLVER_DIR).join(domain);
        if let Ok(content) = std::fs::read_to_string(&path) {
            if content.trim() == resolver_file_content().trim()
                && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }
    let _ = std::fs::remove_file(&manifest);
    if removed > 0 {
        flush_dns_cache();
    }
}

/// 转发线程主体：收应用（系统解析器）的查询 -> 经隧道问 VPN DNS -> 注入路由 -> 回包。
fn forward_loop(
    socket: UdpSocket,
    upstreams: Vec<SocketAddrV4>,
    tun_dev: String,
    dynamic: Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    stop: Arc<AtomicBool>,
    session: VpnSession,
) {
    // 收包超时自己设一遍：调用方忘了设的话，`recv_from` 会一直阻塞，
    // stop 标志检查不到，teardown 里的 join 就永远回不来。
    let _ = socket.set_read_timeout(Some(POLL_TIMEOUT));
    let upstream = match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(e) => {
            session.append_log(&format!("[分流] 转发器打不开上游 socket：{}", e));
            return;
        }
    };
    let _ = upstream.set_read_timeout(Some(UPSTREAM_TIMEOUT));
    let mut query = [0u8; 1500];
    let mut reply = [0u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        let (len, from) = match socket.recv_from(&mut query) {
            Ok(item) => item,
            Err(_) => continue, // 超时：回头看一眼 stop 标志
        };
        if len < 12 {
            continue;
        }
        let mut answer: Option<Vec<u8>> = None;
        for dns in &upstreams {
            if upstream.send_to(&query[..len], *dns).is_err() {
                continue;
            }
            let Ok((got, _)) = upstream.recv_from(&mut reply) else {
                continue;
            };
            // 只认同一次查询的应答（ID 相同且 QR=1），避免把迟到的旧应答当成结果
            let same_query = got >= 12
                && reply[0] == query[0]
                && reply[1] == query[1]
                && reply[2] & 0x80 != 0;
            if same_query {
                answer = Some(reply[..got].to_vec());
                break;
            }
        }
        match answer {
            Some(resp) => {
                install_dynamic_routes(&resp, &tun_dev, &dynamic, &session);
                let _ = socket.send_to(&resp, from);
            }
            None => {
                if let Some(resp) = servfail(&query[..len]) {
                    let _ = socket.send_to(&resp, from);
                }
            }
        }
    }
}

/// 把应答里的 A 记录逐个注入隧道（已注入过的不重复装）。
fn install_dynamic_routes(
    packet: &[u8],
    tun_dev: &str,
    dynamic: &Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    session: &VpnSession,
) {
    for ip in dns_client::parse_a_records(packet) {
        if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
            continue;
        }
        let mut set = match dynamic.lock() {
            Ok(set) => set,
            Err(_) => return,
        };
        if set.contains(&ip) {
            continue;
        }
        let cidr = format!("{}/32", ip);
        match add_tunnel_route(&cidr, tun_dev) {
            Ok(()) => {
                set.insert(ip);
                session.append_log(&format!("[分流] 内网 IP {} 已注入隧道 {}", ip, tun_dev));
            }
            Err(e) => session.append_log(&format!("[分流] 内网 IP {} 注入隧道失败：{}", ip, e)),
        }
    }
}

// ============================== 路由动作 ==============================

/// 给一条网段装「指向隧道」的路由。
///
/// 已存在同前缀的旧路由（上一轮 SIGKILL 留下的）先删再装：指向旧 utun 的路由
/// 会让应用连到一个不存在的隧道，表现是「一直转圈」。
pub(crate) fn add_tunnel_route(cidr: &str, tun_dev: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let target = route::RouteTarget::new(0, tun_dev, "");
        match route::add_route(cidr, &target, 1) {
            Ok(()) => Ok(()),
            Err(e) => {
                // File exists：可能指的就是隧道（重复注入），也可能是残留的旧隧道路由
                let _ = route::delete_route(cidr, &target);
                route::add_route(cidr, &target, 1)
                    .map(|_| ())
                    .map_err(|e2| format!("{}（重试前错误：{}）", e2, e))
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cidr, tun_dev);
        Err("路由注入式分流目前只实现了 macOS".to_string())
    }
}

pub(crate) fn del_tunnel_route(cidr: &str, tun_dev: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let target = route::RouteTarget::new(0, tun_dev, "");
        route::delete_route(cidr, &target)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cidr, tun_dev);
        Err("路由注入式分流目前只实现了 macOS".to_string())
    }
}

/// 刷新 DNS 缓存并让 mDNSResponder 重新读 `/etc/resolver`。
///
/// 不刷新的话：新写的 resolver 文件不生效（旧答案还在缓存里），
/// 断开后残留的解析结果也会继续指向内网地址。
pub(crate) fn flush_dns_cache() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/sbin/dscacheutil")
            .arg("-flushcache")
            .output();
        let _ = std::process::Command::new("/usr/bin/killall")
            .arg("-HUP")
            .arg("mDNSResponder")
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: &str, value: &str, policy: &str) -> VpnRule {
        VpnRule {
            id: "r".into(),
            kind: kind.into(),
            value: value.into(),
            policy: policy.into(),
            remark: String::new(),
            enabled: true,
        }
    }

    #[test]
    fn plan_extracts_suffix_rules_and_dedupes() {
        let rules = vec![
            rule("DOMAIN-SUFFIX", "example.com", "VPN"),
            rule("DOMAIN-SUFFIX", "*.example.com", "VPN"),
            rule("DOMAIN", "OA.example.com", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert_eq!(plan.domains, vec!["example.com", "oa.example.com"]);
        assert!(plan.cidrs.is_empty());
        assert!(plan.notes.is_empty(), "不该有提示：{:?}", plan.notes);
    }

    #[test]
    fn plan_skips_direct_rules_entirely() {
        let rules = vec![
            rule("DOMAIN-SUFFIX", "baidu.com", "DIRECT"),
            rule("DOMAIN-SUFFIX", "example.com", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert_eq!(plan.domains, vec!["example.com"]);
    }

    #[test]
    fn plan_explains_unsupported_kinds() {
        let rules = vec![
            rule("DOMAIN-KEYWORD", "corp", "VPN"),
            rule("PROCESS-NAME", "Lark", "VPN"),
            rule("DST-PORT", "8080", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert!(plan.domains.is_empty());
        assert_eq!(plan.notes.len(), 3, "每类不支持的规则都要给出一条提示");
        assert!(plan.notes[0].contains("DOMAIN-SUFFIX"));
    }

    #[test]
    fn plan_normalises_ip_cidr() {
        let rules = vec![rule("IP-CIDR", "10.10.3.7/16", "VPN")];
        let plan = plan(&rules, "DIRECT");
        assert_eq!(plan.cidrs, vec!["10.10.0.0/16"]);
    }

    #[test]
    fn plan_warns_when_default_policy_is_vpn() {
        let plan = plan(&[], "VPN");
        assert!(plan.notes.iter().any(|note| note.contains("全局模式")));
    }

    #[test]
    fn normalize_domain_rejects_path_traversal_and_junk() {
        assert_eq!(normalize_domain("*.OA.example.com."), Some("oa.example.com".into()));
        assert_eq!(normalize_domain("example.com"), Some("example.com".into()));
        assert_eq!(normalize_domain("../../etc/passwd"), None);
        assert_eq!(normalize_domain("/etc/resolver/x"), None);
        assert_eq!(normalize_domain("localhost"), None, "单标签域名不收");
        assert_eq!(normalize_domain(""), None);
        assert_eq!(normalize_domain("a..b"), None);
    }

    #[test]
    fn servfail_keeps_question_and_flags_failure() {
        let mut query = dns_client::build_query("oa.example.com", 0x1234).unwrap();
        let resp = servfail(&query).expect("应当能构造 SERVFAIL");
        assert_eq!(u16::from_be_bytes([resp[0], resp[1]]), 0x1234, "ID 要保留");
        assert_eq!(resp[2] & 0x80, 0x80, "QR=1");
        assert_eq!(resp[3] & 0x0f, 2, "RCODE=SERVFAIL");
        assert_eq!(u16::from_be_bytes([resp[4], resp[5]]), 1, "QDCOUNT 保留");
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 0, "ANCOUNT 清零");
        // 问题段原样保留（应用要能对上自己问的是哪个域名）
        assert_eq!(&resp[12..], &query[12..]);
        query.clear();
        assert_eq!(servfail(&query), None, "太短的报文不处理");
    }

    /// 转发器核心链路：收查询 -> 交给上游（模拟隧道那头的 VPN DNS）-> 原样把应答回给客户端。
    ///
    /// 真机上监听 53 需要 root，这里用随机端口起同一份 `forward_loop`，
    /// 上游也用随机端口，验证「转发 + 回包」这段逻辑本身没写错。
    #[test]
    fn forwarder_relays_upstream_answer() {
        let dir = std::env::temp_dir().join(format!("ws-split-test-{}", std::process::id()));
        let session = VpnSession::new(dir);
        let _ = session.ensure();

        let stub = UdpSocket::bind("127.0.0.1:0").expect("绑定测试转发器");
        let stub_addr = stub.local_addr().expect("转发器地址");
        let upstream = UdpSocket::bind("127.0.0.1:0").expect("绑定假上游");
        let upstream_addr = upstream.local_addr().expect("上游地址");

        let stop = Arc::new(AtomicBool::new(false));
        let dynamic = Arc::new(Mutex::new(BTreeSet::new()));
        let handle = {
            let stop = Arc::clone(&stop);
            let dynamic = Arc::clone(&dynamic);
            let session = session.clone();
            std::thread::spawn(move || {
                forward_loop(
                    stub,
                    vec![SocketAddrV4::new(match upstream_addr.ip() {
                        std::net::IpAddr::V4(v4) => v4,
                        _ => unreachable!(),
                    }, upstream_addr.port())],
                    "utun-test".to_string(),
                    dynamic,
                    stop,
                    session,
                )
            })
        };

        // 假上游：收到查询后回一个带 A 记录的应答（问题段用压缩指针指回去）
        let upstream_worker = {
            let upstream = upstream.try_clone().expect("克隆上游 socket");
            let upstream_addr = upstream_addr;
            std::thread::spawn(move || {
                let mut buf = [0u8; 512];
                let (len, from) = upstream.recv_from(&mut buf).expect("上游应收到查询");
                let mut resp = buf[..len].to_vec();
                resp[2] |= 0x80; // QR=1
                resp[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT=1
                resp.extend_from_slice(&[0xC0, 0x0C]); // 指向问题段的名字
                resp.extend_from_slice(&[0x00, 0x01]); // TYPE A
                resp.extend_from_slice(&[0x00, 0x01]); // CLASS IN
                resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]); // TTL 60
                resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH 4
                resp.extend_from_slice(&[10, 10, 3, 17]);
                let _ = upstream.send_to(&resp, from);
                let _ = upstream_addr;
            })
        };

        let client = UdpSocket::bind("127.0.0.1:0").expect("绑定客户端");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设置超时");
        let query = dns_client::build_query("oa.example.com", 0x4321).expect("构造查询");
        client
            .send_to(&query, stub_addr)
            .expect("发给本机转发器");
        let mut buf = [0u8; 1024];
        let (len, _) = client.recv_from(&mut buf).expect("应当收到应答");
        assert_eq!(u16::from_be_bytes([buf[0], buf[1]]), 0x4321, "ID 要原样带回");
        assert_eq!(buf[2] & 0x80, 0x80, "QR=1");
        let addresses = dns_client::parse_a_records(&buf[..len]);
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].to_string(), "10.10.3.17");

        upstream_worker.join().expect("假上游线程");
        stop.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }

    #[test]
    fn plan_absorbs_server_pushed_scope() {
        // 服务端下发的默认域名 + split-include 网段都要能被并入计划（这套校验和
        // 规则里的域名/网段完全一致），重复的不重复加。
        let mut plan = SplitPlan::default();
        assert!(plan.add_domain("example.com."));
        assert!(!plan.add_domain("example.com"), "重复域名不该重复加");
        assert!(!plan.add_domain("*.example.com"), "通配写法归一化后仍是同一个");
        assert!(plan.add_cidr("198.51.100.7/22"));
        assert_eq!(plan.cidrs, vec!["198.51.100.0/24"], "网段要归一化");
        assert!(!plan.add_cidr("198.51.100.9/22"), "归一化后同一个网段不重复加");
        assert!(!plan.add_cidr("not-a-cidr"));
        assert_eq!(plan.domains, vec!["example.com"]);
    }

    #[test]
    fn resolver_file_points_at_local_forwarder() {
        assert!(resolver_file_content().contains("nameserver 127.0.0.1"));
    }
}
