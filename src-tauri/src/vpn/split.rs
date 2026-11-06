//! 路由注入式分流（规则模式）：不捕获、不抢默认路由、不改系统 DNS。
//!
//! **为什么不再抓包**：捕获式接管（抢 `0.0.0.0/1` + `128.0.0.0/1`）会让系统的
//! 解析器发不出 DNS 查询 —— macOS 的 mDNSResponder 把查询 socket 钉在物理网卡上，
//! 半默认路由一抢，`sendto` 先报 `[51] Network is unreachable`，给 DNS 加 /32 旁路
//! 后又变成 `[49] Can't assign requested address`；系统解析器一废，所有走它的应用
//! 集体断网（详见设计文档根因九）。所以规则模式改成路由注入。
//!
//! 两个平台共用同一套思路，只是「按域名指定解析器」的机制不同：
//!
//! | 步骤 | macOS | Windows |
//! | --- | --- | --- |
//! | 命中域名的解析交给本机转发器 | 写 `/etc/resolver/<域名>` | NRPT 规则指向 `127.0.0.1:53` |
//! | 本机转发器把查询交给 VPN DNS | `127.0.0.1:53` 转发器 | 同一个 `127.0.0.1:53` 转发器 |
//! | 解析出来的内网 IP 进隧道 | 转发器在应答前注入 `/32` 路由 | 同左 |
//! | `IP-CIDR` / 服务端 split-include | 直接注入隧道路由 | 同左 |
//!
//! 默认路由、系统 DNS、其它流量一律不动 —— 结构上不可能再出现「一连上就断网」。
//!
//! Windows 的 NRPT 只负责把命中域名的查询交给本机转发器；转发器在把应答交回
//! DNS Client 之前，先按 A 记录装好 `/32` 路由。这样 `DOMAIN-SUFFIX` 才能像
//! macOS 一样覆盖任意子域，而不是靠后台线程只解析规则里写死的字面域名。
//! 如果本机 53 端口被占用，或 NRPT 指向 loopback 后没有查询进来，会退回原来的
//! 「NRPT 直连 VPN DNS + 后台主动解析」路径，并在 worker.log 里写明原因。
//!
//! 代价（必须让用户知道）：
//! - `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` 没有数据面可用，做不到；
//! - 域名只覆盖「系统解析器（getaddrinfo）」这条路：自带 DoH/DoT 的应用不受控
//!   （但它解析出来的 IP 若命中 IP-CIDR 规则，照样能进隧道）。

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::dns_client;
use super::models::VpnRule;
use super::route;
use super::rule_engine::{Policy, RuleKind};
use super::session::VpnSession;

/// 本机 DNS 转发器监听地址。
///
/// macOS 的 `/etc/resolver` 只能写 nameserver 地址，Windows 的 NRPT 也只能写
/// 一个 IP；两边都用 loopback 的 53 端口最直接。worker 以管理员/root 运行，
/// Windows 上 53 不要求特权端口，但可能被别的 DNS 服务占用，占用时回退。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub const FORWARDER_ADDR: &str = "127.0.0.1:53";
/// NRPT 指向本机转发器时使用的地址。
#[cfg(target_os = "windows")]
const LOCAL_DNS_SERVER: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
/// `/etc/resolver` 目录（macOS 的按域名分流解析配置目录）
#[cfg(target_os = "macos")]
pub const RESOLVER_DIR: &str = "/etc/resolver";
/// 我们写过的 resolver 文件名清单：进程被 SIGKILL 时靠它在下次连接前清干净
#[cfg(target_os = "macos")]
pub const MANIFEST_FILE: &str = "resolver-manifest.txt";
/// 单次上游查询的超时
#[cfg(any(target_os = "macos", target_os = "windows"))]
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(2);
/// 转发线程的收包粒度：只为了让 stop 标志能及时生效
#[cfg(any(target_os = "macos", target_os = "windows"))]
const POLL_TIMEOUT: Duration = Duration::from_millis(500);
/// Windows：单次主动解析的超时。内网 DNS 正常是毫秒级，1.5s 足够，
/// 也避免一条不通的上游把整轮解析拖到十几秒。
#[cfg(target_os = "windows")]
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Windows：按域名解析出来的地址多久刷新一次（服务端可能做轮询/负载均衡）
#[cfg(target_os = "windows")]
const RESOLVE_INTERVAL: Duration = Duration::from_secs(15);
/// Windows：一轮解析最多等多久（域名多、并发线程多时兜底）
#[cfg(target_os = "windows")]
const RESOLVE_ROUND_DEADLINE: Duration = Duration::from_secs(5);

// ============================== 规则 → 计划 ==============================

/// 规则翻译成「路由注入模式能做的事」。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SplitPlan {
    /// 需要按域名分流的域名（小写、去通配符）
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
                push_unique(
                    &mut out.notes,
                    format!("{} {}：{}", rule.kind, rule.value, e),
                );
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
            Ok((net, prefix)) => add_unique(
                &mut self.cidrs,
                format!("{}/{}", Ipv4Addr::from(net), prefix),
            ),
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
/// 校验同时也是安全边界：macOS 上这些字符串会变成 `/etc/resolver/<域名>` 的文件名，
/// Windows 上会写进 NRPT 规则的命名空间，必须挡掉 `/`、`.` 开头之类的路径穿越。
pub fn normalize_domain(value: &str) -> Option<String> {
    let mut text = value.trim().to_ascii_lowercase();
    text = text.trim_start_matches("*.").to_string();
    text = text
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_string();
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
#[cfg(any(target_os = "macos", target_os = "windows", test))]
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
#[cfg(any(target_os = "macos", target_os = "windows", test))]
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

/// 路由注入分流的运行时状态：DNS 分流线程 + 我们装出去的路由 + 解析器配置。
pub struct SplitTunnel {
    domains: Vec<String>,
    tun_dev: String,
    /// Windows：隧道接口索引；macOS 恒为 0（那边按接口名操作）
    tun_index: u32,
    vpn_dns: Vec<Ipv4Addr>,
    /// 固定要维护的路由（VPN DNS + `IP-CIDR` 规则 + 服务端 split-include）
    static_routes: Vec<String>,
    /// 从 DNS 应答里学到的内网 IP（动态注入，断开时按它回收）
    dynamic: Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    /// macOS：我们写过的 `/etc/resolver/<域名>`
    #[cfg(target_os = "macos")]
    resolver_files: Vec<std::path::PathBuf>,
    /// macOS：崩溃清理清单
    #[cfg(target_os = "macos")]
    manifest: std::path::PathBuf,
    /// Windows：是否已写好 NRPT 规则
    #[cfg(target_os = "windows")]
    nrpt_installed: bool,
    /// Windows：NRPT 不可用时退回了「只给隧道网卡配 DNS」
    #[cfg(target_os = "windows")]
    adapter_dns_fallback: bool,
    /// Windows：NRPT 是否指向本机 127.0.0.1:53 转发器
    #[cfg(target_os = "windows")]
    local_dns_proxy: bool,
    /// 本机转发器收到的查询数：Windows 用来确认 NRPT→loopback 是否真的通了
    dns_queries: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    /// macOS 的 DNS 转发线程 / Windows 的主动解析线程
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SplitTunnel {
    /// 装好路由注入分流。任何一步失败都会先把已做的动作回滚，再返回错误。
    pub fn apply(
        session: &VpnSession,
        tun_dev: &str,
        tun_index: u32,
        vpn_dns: &[Ipv4Addr],
        plan: SplitPlan,
    ) -> Result<Self, String> {
        if plan.domains.is_empty() && plan.cidrs.is_empty() {
            return Err(
                "规则里没有可用的分流动作：域名请用「域名后缀」，网段请用 IP-CIDR".to_string(),
            );
        }
        // 上一轮被 SIGKILL 时留下的解析器配置先清掉，否则清不干净的域名
        // 会一直指向一个已经断掉的 DNS（macOS）或一条过期规则（Windows）。
        cleanup_stale(session);

        // 只有域名规则需要按域名分流解析；纯 IP-CIDR 分流不碰 DNS，
        // 免得白白和别的 DNS 服务/规则撞车。
        let need_dns = !plan.domains.is_empty();
        if need_dns && vpn_dns.is_empty() {
            return Err("服务端没有下发 DNS 地址，内网域名无法解析".to_string());
        }
        let mut me = SplitTunnel {
            domains: plan.domains.clone(),
            tun_dev: tun_dev.to_string(),
            tun_index,
            vpn_dns: vpn_dns.to_vec(),
            static_routes: Vec::new(),
            dynamic: Arc::new(Mutex::new(BTreeSet::new())),
            #[cfg(target_os = "macos")]
            resolver_files: Vec::new(),
            #[cfg(target_os = "macos")]
            manifest: session.dir.join(MANIFEST_FILE),
            #[cfg(target_os = "windows")]
            nrpt_installed: false,
            #[cfg(target_os = "windows")]
            adapter_dns_fallback: false,
            #[cfg(target_os = "windows")]
            local_dns_proxy: false,
            dns_queries: Arc::new(AtomicUsize::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        };

        if let Err(e) = me.install_routes(&plan.cidrs) {
            me.teardown();
            return Err(e);
        }
        if need_dns {
            if let Err(e) = me.install_resolver(session) {
                me.teardown();
                return Err(e);
            }
            if let Err(e) = me.start_resolver(session) {
                me.teardown();
                return Err(e);
            }
            flush_dns_cache();
        }
        Ok(me)
    }

    fn tun_index(&self) -> u32 {
        self.tun_index
    }

    /// VPN DNS + IP-CIDR 规则：装成隧道的固定路由。
    fn install_routes(&mut self, cidrs: &[String]) -> Result<(), String> {
        let mut wanted: Vec<String> = Vec::new();
        for ip in &self.vpn_dns {
            wanted.push(format!("{}/32", ip));
        }
        wanted.extend(cidrs.iter().cloned());
        for cidr in &wanted {
            add_tunnel_route(cidr, &self.tun_dev, self.tun_index())?;
            if !self.static_routes.contains(cidr) {
                self.static_routes.push(cidr.clone());
            }
        }
        Ok(())
    }

    /// 把「这些域名交给 VPN DNS」这条本地布线装好。
    fn install_resolver(&mut self, session: &VpnSession) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let _ = session;
            self.write_resolver_files()
        }
        #[cfg(target_os = "windows")]
        {
            self.install_windows_resolver(session)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = session;
            Err("当前平台还没有按域名分流解析的实现".to_string())
        }
    }

    /// 写 `/etc/resolver/<域名>`：让系统解析器把这些域名交给本机转发器（macOS）。
    #[cfg(target_os = "macos")]
    fn write_resolver_files(&mut self) -> Result<(), String> {
        use std::path::{Path, PathBuf};
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

    /// 起 DNS 分流线程：macOS 是 127.0.0.1:53 的转发器；Windows 优先也是同一个
    /// 转发器（NRPT 指过来），只有转发器不可用时才退回后台主动解析线程。
    fn start_resolver(&mut self, session: &VpnSession) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            self.start_forwarder(session)
        }
        #[cfg(target_os = "windows")]
        {
            if self.local_dns_proxy {
                // 转发线程已经在 install_windows_resolver 里起来了
                Ok(())
            } else {
                self.start_resolve_thread(session)
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = session;
            Err("当前平台还没有按域名分流解析的实现".to_string())
        }
    }

    /// 绑定 127.0.0.1:53 并起转发线程（macOS）。
    #[cfg(target_os = "macos")]
    fn start_forwarder(&mut self, session: &VpnSession) -> Result<(), String> {
        let socket = UdpSocket::bind(FORWARDER_ADDR).map_err(|e| {
            format!(
                "绑定 {} 失败：{}（端口被其它 DNS 服务占用？）",
                FORWARDER_ADDR, e
            )
        })?;
        socket
            .set_read_timeout(Some(POLL_TIMEOUT))
            .map_err(|e| e.to_string())?;
        self.spawn_forwarder(socket, session)
    }

    /// 起转发线程（macOS / Windows 共用）。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn spawn_forwarder(&mut self, socket: UdpSocket, session: &VpnSession) -> Result<(), String> {
        let upstreams: Vec<SocketAddrV4> = self
            .vpn_dns
            .iter()
            .map(|ip| SocketAddrV4::new(*ip, 53))
            .collect();
        let tun_dev = self.tun_dev.clone();
        let tun_index = self.tun_index();
        let dynamic = Arc::clone(&self.dynamic);
        let queries = Arc::clone(&self.dns_queries);
        let stop = Arc::clone(&self.stop);
        let log = session.clone();
        let handle = std::thread::Builder::new()
            .name("ws-dns-forwarder".to_string())
            .spawn(move || {
                forward_loop(
                    socket, upstreams, tun_dev, tun_index, dynamic, queries, stop, log,
                )
            })
            .map_err(|e| format!("启动 DNS 转发线程失败：{}", e))?;
        self.thread = Some(handle);
        Ok(())
    }

    /// Windows：优先把 NRPT 指向本机 127.0.0.1:53 转发器，失败再退回旧路径。
    ///
    /// 这是 Windows 上和 macOS 对齐的关键：NRPT 只负责把命中域名的查询交给
    /// 本机转发器；转发器拿到 VPN DNS 的应答后，先按 A 记录装 `/32` 路由，
    /// 再把应答交回 DNS Client。这样 `DOMAIN-SUFFIX` 的任意子域都能在
    /// 「应用拿到地址之前」完成路由注入。
    #[cfg(target_os = "windows")]
    fn install_windows_resolver(&mut self, session: &VpnSession) -> Result<(), String> {
        match self.start_windows_local_proxy(session) {
            Ok(()) => {
                match super::win_dns::apply_namespace_rules(&self.domains, &[LOCAL_DNS_SERVER]) {
                    Ok(count) => {
                        self.nrpt_installed = true;
                        self.local_dns_proxy = true;
                        flush_dns_cache();
                        if self.local_proxy_receives_queries() {
                            session.append_log(&format!(
                                "[分流] NRPT 已把 {} 个域名交给本机 DNS 转发器 {}；\
                                 按应答注入内网 /32 路由（DOMAIN-SUFFIX 覆盖任意子域）",
                                count, FORWARDER_ADDR
                            ));
                            return Ok(());
                        }
                        // NRPT 写成功了，但系统没有把查询送进 loopback：不能假装成功。
                        session.append_log(&format!(
                            "[分流] NRPT 已指向 {}，但 1.5 秒内没有查询进入本机转发器；\
                             可能系统不接受 loopback DNS，回退到「NRPT 直连 VPN DNS + 后台解析」",
                            FORWARDER_ADDR
                        ));
                        let _ = super::win_dns::remove_all_rules();
                        self.nrpt_installed = false;
                        self.stop_local_forwarder();
                    }
                    Err(e) => {
                        session.append_log(&format!(
                            "[分流] 本机 DNS 转发器已启动，但 NRPT 指向 {} 失败：{}；回退到旧路径",
                            FORWARDER_ADDR, e
                        ));
                        self.stop_local_forwarder();
                    }
                }
            }
            Err(e) => {
                session.append_log(&format!(
                    "[分流] 本机 DNS 转发器不可用：{}；回退到「NRPT 直连 VPN DNS + 后台解析」",
                    e
                ));
            }
        }

        // 旧路径：NRPT 直接指向 VPN DNS 时，用户态看不到子域查询，只能靠后台
        // 主动解析规则里的字面域名；这是保底，不再是首选。
        match super::win_dns::apply_namespace_rules(&self.domains, &self.vpn_dns) {
            Ok(count) => {
                self.nrpt_installed = true;
                session.append_log(&format!(
                    "[分流] 已为 {} 个域名配置按域名解析（NRPT），命中即交给 VPN DNS {:?}",
                    count, self.vpn_dns
                ));
                Ok(())
            }
            Err(e) => {
                // 退而求其次：把 VPN DNS 配到隧道网卡上。这样内网域名至少能解析，
                // 代价是这台机器所有 DNS 都可能经隧道（NRPT 只覆盖列出的域名）。
                match super::win_dns::set_interface_dns(self.tun_index(), &self.vpn_dns) {
                    Ok(()) => {
                        self.adapter_dns_fallback = true;
                        session.append_log(&format!(
                            "[分流] 按域名解析（NRPT）不可用：{}；已退回「只给隧道网卡配 DNS」{}，\
                             内网域名仍可解析，但公共域名也可能走 VPN DNS",
                            e, self.tun_dev
                        ));
                        Ok(())
                    }
                    Err(e2) => Err(format!(
                        "按域名解析（NRPT）配置失败：{}；退回隧道网卡 DNS 也失败：{}",
                        e, e2
                    )),
                }
            }
        }
    }

    /// Windows：绑定 127.0.0.1:53 并起转发线程。失败时不会留下半启动状态。
    #[cfg(target_os = "windows")]
    fn start_windows_local_proxy(&mut self, session: &VpnSession) -> Result<(), String> {
        let socket = UdpSocket::bind(FORWARDER_ADDR).map_err(|e| {
            format!(
                "绑定 {} 失败：{}（端口被其它 DNS 服务占用？）",
                FORWARDER_ADDR, e
            )
        })?;
        socket
            .set_read_timeout(Some(POLL_TIMEOUT))
            .map_err(|e| e.to_string())?;
        self.spawn_forwarder(socket, session)?;
        self.local_dns_proxy = true;
        session.append_log(&format!(
            "[分流] 本机 DNS 转发器已监听 {}，上游 VPN DNS {:?}",
            FORWARDER_ADDR, self.vpn_dns
        ));
        Ok(())
    }

    /// 停掉本机转发线程，并把 stop 标志复位，方便回退路径再起主动解析线程。
    #[cfg(target_os = "windows")]
    fn stop_local_forwarder(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        self.stop = Arc::new(AtomicBool::new(false));
        self.local_dns_proxy = false;
    }

    /// 用一条随机子域查询确认「NRPT -> 127.0.0.1:53」真的能把查询送进来。
    /// 只看 NRPT 写成功不够：Windows 对 loopback DNS 的支持在不同版本/策略下
    /// 不一致，没查询进来时应当回退，而不是让用户以为子域分流已经生效。
    #[cfg(target_os = "windows")]
    fn local_proxy_receives_queries(&self) -> bool {
        let before = self.dns_queries.load(Ordering::Relaxed);
        let Some(domain) = self.domains.first() else {
            return false;
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let probe = format!("ws-probe-{}-{}.{}", std::process::id(), stamp, domain);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::net::ToSocketAddrs;
            let _ = (probe.as_str(), 443u16).to_socket_addrs();
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_millis(1500));
        self.dns_queries.load(Ordering::Relaxed) > before
    }

    /// 当前 NRPT 应该指向谁：本机转发器，或退回时的 VPN DNS。
    #[cfg(target_os = "windows")]
    fn nrpt_servers(&self) -> Vec<Ipv4Addr> {
        if self.local_dns_proxy {
            vec![LOCAL_DNS_SERVER]
        } else {
            self.vpn_dns.clone()
        }
    }

    /// 起主动解析线程（Windows）。
    ///
    /// NRPT 把查询交给系统 DNS Client，用户态看不到应答，所以拿不到「应用刚解析了
    /// 哪个地址」这个时机。这里改成主动问：按规则里的域名向 VPN DNS 查 A 记录，
    /// 把结果逐个注入隧道，并按 [`RESOLVE_INTERVAL`] 刷新，覆盖服务端轮询地址的情况。
    #[cfg(target_os = "windows")]
    fn start_resolve_thread(&mut self, session: &VpnSession) -> Result<(), String> {
        let domains = self.domains.clone();
        let upstreams: Vec<SocketAddrV4> = self
            .vpn_dns
            .iter()
            .map(|ip| SocketAddrV4::new(*ip, 53))
            .collect();
        let tun_dev = self.tun_dev.clone();
        let tun_index = self.tun_index();
        let dynamic = Arc::clone(&self.dynamic);
        let stop = Arc::clone(&self.stop);
        let log = session.clone();
        let handle = std::thread::Builder::new()
            .name("ws-dns-resolver".to_string())
            .spawn(move || resolve_loop(domains, upstreams, tun_dev, tun_index, dynamic, stop, log))
            .map_err(|e| format!("启动按域名解析线程失败：{}", e))?;
        self.thread = Some(handle);
        Ok(())
    }

    /// 后台自检：按应用真实路径，经系统解析器解析规则里的域名。
    ///
    /// 这条链路是「系统解析器 -> 按域名分流 -> 隧道 -> VPN DNS」，
    /// 任一段断了都能在 worker.log 里一眼看出来。
    pub fn spawn_self_check(&self, session: VpnSession) {
        let domains = self.domains.clone();
        let dynamic = Arc::clone(&self.dynamic);
        if domains.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            // Windows 的按域名注入是后台轮询的：先给它一轮时间，否则自检永远
            // 看到「还没见到隧道路由」，把正常的 1~2 秒延迟误报成故障。
            #[cfg(target_os = "windows")]
            std::thread::sleep(Duration::from_secs(3));
            session.append_log("[自检] 开始：系统解析器 -> 按域名分流 -> 隧道 -> VPN DNS");
            for domain in domains.iter().take(3) {
                let name = domain.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    use std::net::ToSocketAddrs;
                    let result = (name.as_str(), 443u16)
                        .to_socket_addrs()
                        .map(|it| it.map(|a| a.ip().to_string()).collect::<Vec<_>>());
                    let _ = tx.send(result);
                });
                match rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(Ok(ips)) if !ips.is_empty() => {
                        let routed = ips.iter().any(|ip| {
                            ip.parse::<Ipv4Addr>()
                                .map(|v4| {
                                    dynamic.lock().map(|set| set.contains(&v4)).unwrap_or(false)
                                })
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
                        "[自检] {} 的查询已走到按域名分流解析，但这个域名本身没有 A 记录\
                         （内网主机请用完整域名，比如 oa.example.com）",
                        domain
                    )),
                    Ok(Err(e)) => {
                        // 「nodename nor servname」= 这个名字本身没有 A 记录（NXDOMAIN/无记录），
                        // 说明查询链路是通的；「Temporary failure / timed out」才是链路真不通。
                        let text = e.to_string();
                        let no_record = text.contains("nodename")
                            || text.contains("no such host")
                            || text.contains("Name or service not known")
                            // 中文 Windows 的 getaddrinfo 文案
                            || text.contains("不知道这样的主机")
                            || text.contains("找不到主机");
                        if no_record {
                            // 这是一条**信息**，不是故障：裸域名/后缀本身没有 A 记录是常见的。
                            // 不要拼接原始错误文本（里面通常带 "os error 11001"），否则前端
                            // 只按关键字判断日志级别，会把整行染成 ERR，掩盖真正的分流失败。
                            session.append_log(&format!(
                                "[自检] {domain} 本身没有 A 记录（裸域名/域名后缀通常如此，\
                                 不代表隧道或分流异常；访问实际子域时会按应答注入 /32 路由）"
                            ));
                        } else {
                            session.append_log(&format!(
                                "[自检] {domain} 解析失败：{text}（按域名分流或 VPN DNS 可能不通，看上面 [分流] 日志）"
                            ));
                        }
                    }
                    Err(_) => session.append_log(&format!(
                        "[自检] {} 解析超时：5 秒没有结果，按域名分流或 VPN DNS 可能不通",
                        domain
                    )),
                }
            }
        });
    }

    /// 路由守护：隧道路由被别的东西改掉时补回来，解析器配置被删时补回来。
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
                let _ = add_tunnel_route(cidr, &self.tun_dev, self.tun_index());
            }
        }
        #[cfg(target_os = "windows")]
        {
            // 一条 PowerShell 把「缺哪条补哪条」全办了：逐条查要起 N 次进程，
            // 每 15 秒一轮会把 worker 主循环拖慢。
            let _ = route::ensure_onlink_routes(&self.static_routes, self.tun_index(), 1);
            // NRPT 规则被组策略/其它客户端盖掉时补回来。
            //
            // 注意必须重下**全部**域名：`apply_namespace_rules` 的脚本会先清掉所有
            // `ws-workbench:` 规则再逐条重建，只传「缺的那几个」会把还在的规则一起删掉，
            // 结果是每 15 秒把解析打出一个空洞。
            if self.nrpt_installed {
                if let Ok(installed) = super::win_dns::installed_domains() {
                    let missing = self
                        .domains
                        .iter()
                        .any(|domain| !installed.contains(domain));
                    if missing {
                        let _ = super::win_dns::apply_namespace_rules(
                            &self.domains,
                            &self.nrpt_servers(),
                        );
                    }
                }
            }
            if self.adapter_dns_fallback {
                let current = super::win_dns::interface_dns(self.tun_index()).unwrap_or_default();
                let wanted: Vec<String> = self.vpn_dns.iter().map(|ip| ip.to_string()).collect();
                if !wanted.iter().all(|ip| current.contains(ip)) {
                    let _ = super::win_dns::set_interface_dns(self.tun_index(), &self.vpn_dns);
                }
            }
        }
        #[cfg(target_os = "macos")]
        for path in &self.resolver_files {
            let missing = std::fs::metadata(path).is_err();
            if missing {
                let _ = std::fs::write(path, resolver_file_content());
            }
        }
    }

    /// 收工时回收一切：线程、动态路由、固定路由、解析器配置。
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
            let _ = del_tunnel_route(&format!("{}/32", ip), &self.tun_dev, self.tun_index());
        }
        for cidr in &self.static_routes {
            let _ = del_tunnel_route(cidr, &self.tun_dev, self.tun_index());
        }
        #[cfg(target_os = "macos")]
        {
            for path in &self.resolver_files {
                let _ = std::fs::remove_file(path);
            }
            let _ = std::fs::remove_file(&self.manifest);
            self.resolver_files.clear();
        }
        #[cfg(target_os = "windows")]
        {
            if self.nrpt_installed {
                let _ = super::win_dns::remove_all_rules();
                self.nrpt_installed = false;
            }
            if self.adapter_dns_fallback {
                let _ = super::win_dns::reset_interface_dns(self.tun_index());
                self.adapter_dns_fallback = false;
            }
        }
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

/// `/etc/resolver/<域名>` 的内容：把所有查询交给本机转发器（macOS）。
#[cfg(any(target_os = "macos", test))]
pub fn resolver_file_content() -> &'static str {
    "nameserver 127.0.0.1\n"
}

#[cfg(target_os = "macos")]
fn dir_create(dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{}", dir, e))
}

/// 清掉上一次崩溃留下的解析器配置。
///
/// macOS 只删清单里记过、且内容还是我们写的 `nameserver 127.0.0.1` 的文件 ——
/// 绝不能碰用户自己配的 `/etc/resolver`；Windows 只删 `DisplayName` 带我们前缀的
/// NRPT 规则，同样不碰用户/组策略下发的规则。
pub fn cleanup_stale(session: &VpnSession) {
    #[cfg(target_os = "macos")]
    {
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
            let path = std::path::Path::new(RESOLVER_DIR).join(domain);
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
    #[cfg(target_os = "windows")]
    {
        match super::win_dns::remove_all_rules() {
            Ok(0) => {}
            Ok(count) => {
                session.append_log(&format!(
                    "[分流] 已清理上次残留的 {} 条按域名分流规则（NRPT）",
                    count
                ));
                flush_dns_cache();
            }
            Err(e) => session.append_log(&format!(
                "[分流] 清理残留的按域名分流规则失败：{}（不影响本次连接）",
                e
            )),
        }
    }
}

/// 转发线程主体（macOS / Windows）：收本机 DNS Client 的查询 -> 经隧道问 VPN DNS
/// -> 注入路由 -> 回包。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn forward_loop(
    socket: UdpSocket,
    upstreams: Vec<SocketAddrV4>,
    tun_dev: String,
    tun_index: u32,
    dynamic: Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    queries: Arc<AtomicUsize>,
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
        queries.fetch_add(1, Ordering::Relaxed);
        let mut answer: Option<Vec<u8>> = None;
        for dns in &upstreams {
            if upstream.send_to(&query[..len], *dns).is_err() {
                continue;
            }
            let Ok((got, _)) = upstream.recv_from(&mut reply) else {
                continue;
            };
            // 只认同一次查询的应答（ID 相同且 QR=1），避免把迟到的旧应答当成结果
            let same_query =
                got >= 12 && reply[0] == query[0] && reply[1] == query[1] && reply[2] & 0x80 != 0;
            if same_query {
                answer = Some(reply[..got].to_vec());
                break;
            }
        }
        match answer {
            Some(resp) => {
                install_dynamic_routes(&resp, &tun_dev, tun_index, &dynamic, &session);
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

/// 把应答里的 A 记录注入隧道（已注入过的不重复装；macOS / Windows）。
///
/// Windows 必须在 DNS 应答回给应用之前装好路由；多 A 记录时逐条起 PowerShell
/// 会把回包拖到 DNS Client 超时，所以先把新地址攒起来，一条 PowerShell 批量装。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn install_dynamic_routes(
    packet: &[u8],
    tun_dev: &str,
    tun_index: u32,
    dynamic: &Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    session: &VpnSession,
) {
    let candidates: Vec<Ipv4Addr> = {
        let set = match dynamic.lock() {
            Ok(set) => set,
            Err(_) => return,
        };
        let mut out = Vec::new();
        for ip in dns_client::parse_a_records(packet) {
            if usable_tunnel_ip(ip) && !set.contains(&ip) && !out.contains(&ip) {
                out.push(ip);
            }
        }
        out
    };
    if candidates.is_empty() {
        return;
    }

    #[cfg(target_os = "windows")]
    {
        let cidrs: Vec<String> = candidates.iter().map(|ip| format!("{}/32", ip)).collect();
        match route::ensure_onlink_routes(&cidrs, tun_index, 1) {
            Ok(()) => {
                if let Ok(mut set) = dynamic.lock() {
                    for ip in &candidates {
                        set.insert(*ip);
                    }
                }
                for ip in &candidates {
                    session.append_log(&format!(
                        "[分流] 内网 IP {} 已注入隧道 {}",
                        ip, tun_dev
                    ));
                }
            }
            Err(e) => session.append_log(&format!(
                "[分流] 内网 IP {} 注入隧道失败：{}",
                candidates
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                e
            )),
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = tun_index;
        for ip in candidates {
            let cidr = format!("{}/32", ip);
            match add_tunnel_route(&cidr, tun_dev, 0) {
                Ok(()) => {
                    if let Ok(mut set) = dynamic.lock() {
                        set.insert(ip);
                    }
                    session.append_log(&format!("[分流] 内网 IP {} 已注入隧道 {}", ip, tun_dev));
                }
                Err(e) => {
                    session.append_log(&format!("[分流] 内网 IP {} 注入隧道失败：{}", ip, e))
                }
            }
        }
    }
}

/// 一个解析出来的地址值不值得为它加 `/32` 隧道路由。
fn usable_tunnel_ip(ip: Ipv4Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_link_local()
        && !ip.is_broadcast()
}

// ============================== Windows：主动解析 ==============================

/// 主动解析线程主体（Windows）：按域名问 VPN DNS -> 把 A 记录注入隧道 -> 定时刷新。
#[cfg(target_os = "windows")]
fn resolve_loop(
    domains: Vec<String>,
    upstreams: Vec<SocketAddrV4>,
    tun_dev: String,
    tun_index: u32,
    dynamic: Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    stop: Arc<AtomicBool>,
    session: VpnSession,
) {
    let mut first = true;
    // 注入失败时只在错误变化时记一行：每 15 秒刷同一条会把 worker.log 冲垮
    let mut last_error = String::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let found = resolve_all(&domains, &upstreams);
        match sync_dynamic_routes(&found, tun_index, &tun_dev, &dynamic, &session) {
            Ok(added) => {
                if !last_error.is_empty() {
                    session.append_log("[分流] 内网地址注入隧道已恢复");
                    last_error.clear();
                }
                if first {
                    session.append_log(&format!(
                        "[分流] 按域名解析：{} 个域名，首批注入 {} 个内网地址；之后每 {} 秒刷新一次",
                        domains.len(),
                        added,
                        RESOLVE_INTERVAL.as_secs()
                    ));
                    if added == 0 {
                        session.append_log(
                            "[分流] 首批没有拿到可用的内网地址：VPN DNS 可能经隧道不可达，\
                             或这些域名本身没有 A 记录（内网主机请写完整域名，比如 oa.example.com）",
                        );
                    }
                }
            }
            Err(e) => {
                if e != last_error {
                    session.append_log(&format!("[分流] 内网地址注入隧道失败：{}", e));
                    last_error = e;
                }
            }
        }
        first = false;
        // 分片睡眠：teardown 时最多 200ms 就能退出，不用等满一个刷新周期
        let deadline = std::time::Instant::now() + RESOLVE_INTERVAL;
        while std::time::Instant::now() < deadline {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// 并发解析所有域名，等一轮的结果（Windows）。
#[cfg(target_os = "windows")]
fn resolve_all(domains: &[String], upstreams: &[SocketAddrV4]) -> Vec<Ipv4Addr> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let mut pending = 0usize;
    for domain in domains {
        let tx = tx.clone();
        let name = domain.clone();
        let upstreams = upstreams.to_vec();
        // 一个域名一个线程：单个域名超时不会把后面的域名一起拖住
        std::thread::spawn(move || {
            let ips = query_a_records(&name, &upstreams);
            let _ = tx.send(ips);
        });
        pending += 1;
    }
    drop(tx);
    let mut out: Vec<Ipv4Addr> = Vec::new();
    let deadline = std::time::Instant::now() + RESOLVE_ROUND_DEADLINE;
    while pending > 0 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(ips) => {
                out.extend(ips);
                pending -= 1;
            }
            Err(_) => break,
        }
    }
    out
}

/// 直接向 VPN DNS 发一个 A 查询（Windows）。
///
/// 不走系统解析器：这里要的就是「隧道那头的答案」，系统解析器此时由 NRPT 接管，
/// 再走 `getaddrinfo` 会把查询绕回 DNS Client，既拿不到刚解析的地址，也可能递归。
#[cfg(target_os = "windows")]
fn query_a_records(name: &str, upstreams: &[SocketAddrV4]) -> Vec<Ipv4Addr> {
    let id = query_id(name);
    let Some(query) = dns_client::build_query(name, id) else {
        return Vec::new();
    };
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = socket.set_read_timeout(Some(RESOLVE_TIMEOUT));
    let mut buf = [0u8; 4096];
    for upstream in upstreams {
        if socket.send_to(&query, upstream).is_err() {
            continue;
        }
        let Ok((len, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if dns_client::is_response_to(&buf[..len], id) {
            return dns_client::parse_a_records(&buf[..len]);
        }
    }
    Vec::new()
}

/// 查询 ID：域名哈希 + 纳秒，避免同一域名在同一毫秒内重复时撞 ID。
#[cfg(target_os = "windows")]
fn query_id(name: &str) -> u16 {
    let mut hash = 0x811c9dc5u32;
    for byte in name.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (hash ^ nanos) as u16
}

/// 把一批解析出来的地址装成隧道路由，返回本次新增几个（Windows）。
///
/// 每轮都把「本轮所有地址」交给 `ensure_onlink_routes`（幂等）：这样即使路由被
/// 别的软件删掉，下一轮也会补回来，而不是因为集合里已有这个 IP 就永远不管它。
#[cfg(target_os = "windows")]
fn sync_dynamic_routes(
    found: &[Ipv4Addr],
    tun_index: u32,
    tun_dev: &str,
    dynamic: &Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    session: &VpnSession,
) -> Result<usize, String> {
    let mut wanted: Vec<Ipv4Addr> = Vec::new();
    for ip in found {
        if usable_tunnel_ip(*ip) && !wanted.contains(ip) {
            wanted.push(*ip);
        }
    }
    if wanted.is_empty() {
        return Ok(0);
    }
    let before: BTreeSet<Ipv4Addr> = dynamic.lock().map(|set| set.clone()).unwrap_or_default();
    let cidrs: Vec<String> = wanted.iter().map(|ip| format!("{}/32", ip)).collect();
    route::ensure_onlink_routes(&cidrs, tun_index, 1)?;
    let mut added = 0usize;
    if let Ok(mut set) = dynamic.lock() {
        for ip in wanted {
            if set.insert(ip) && !before.contains(&ip) {
                added += 1;
                session.append_log(&format!("[分流] 内网 IP {} 已注入隧道 {}", ip, tun_dev));
            }
        }
    }
    Ok(added)
}

// ============================== 路由动作 ==============================

/// 给一条网段装「指向隧道」的路由。
///
/// 已存在同前缀的旧路由（上一轮 SIGKILL 留下的）先删再装：指向旧隧道的路由
/// 会让应用连到一个不存在的隧道，表现是「一直转圈」。
pub(crate) fn add_tunnel_route(cidr: &str, tun_dev: &str, tun_index: u32) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = tun_index;
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
    #[cfg(target_os = "windows")]
    {
        // 隧道侧一律 on-link（0.0.0.0）：Wintun 网卡上没有真实的下一跳可用
        let _ = tun_dev;
        route::ensure_onlink_routes(&[cidr.to_string()], tun_index, 1)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (cidr, tun_dev, tun_index);
        Err("路由注入式分流目前只实现了 macOS / Windows".to_string())
    }
}

pub(crate) fn del_tunnel_route(cidr: &str, tun_dev: &str, tun_index: u32) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = tun_index;
        let target = route::RouteTarget::new(0, tun_dev, "");
        route::delete_route(cidr, &target)
    }
    #[cfg(target_os = "windows")]
    {
        let target = route::RouteTarget::new(tun_index, tun_dev, "");
        route::delete_route(cidr, &target)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (cidr, tun_dev, tun_index);
        Err("路由注入式分流目前只实现了 macOS / Windows".to_string())
    }
}

/// 刷新 DNS 缓存，并让系统重新读解析器配置。
///
/// 不刷新的话：新配置不生效（旧答案还在缓存里），断开后残留的解析结果
/// 也会继续指向内网地址。
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
    #[cfg(target_os = "windows")]
    {
        super::win_dns::flush_cache();
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
            rule("DOMAIN-SUFFIX", "RisenCN.com", "VPN"),
            rule("DOMAIN-SUFFIX", "*.risencn.com", "VPN"),
            rule("DOMAIN", "OA.risencn.com", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert_eq!(plan.domains, vec!["risencn.com", "oa.risencn.com"]);
        assert!(plan.cidrs.is_empty());
        assert!(plan.notes.is_empty(), "不该有提示：{:?}", plan.notes);
    }

    #[test]
    fn plan_skips_direct_rules_entirely() {
        let rules = vec![
            rule("DOMAIN-SUFFIX", "baidu.com", "DIRECT"),
            rule("DOMAIN-SUFFIX", "risencn.com", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert_eq!(plan.domains, vec!["risencn.com"]);
    }

    #[test]
    fn plan_explains_unsupported_kinds() {
        let rules = vec![
            rule("DOMAIN-KEYWORD", "risen", "VPN"),
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
        let rules = vec![rule("IP-CIDR", "10.8.3.7/16", "VPN")];
        let plan = plan(&rules, "DIRECT");
        assert_eq!(plan.cidrs, vec!["10.8.0.0/16"]);
    }

    #[test]
    fn plan_warns_when_default_policy_is_vpn() {
        let plan = plan(&[], "VPN");
        assert!(plan.notes.iter().any(|note| note.contains("全局模式")));
    }

    #[test]
    fn normalize_domain_rejects_path_traversal_and_junk() {
        assert_eq!(
            normalize_domain("*.OA.RisenCN.com."),
            Some("oa.risencn.com".into())
        );
        assert_eq!(normalize_domain("risencn.com"), Some("risencn.com".into()));
        assert_eq!(normalize_domain("../../etc/passwd"), None);
        assert_eq!(normalize_domain("/etc/resolver/x"), None);
        assert_eq!(normalize_domain("localhost"), None, "单标签域名不收");
        assert_eq!(normalize_domain(""), None);
        assert_eq!(normalize_domain("a..b"), None);
    }

    #[test]
    fn servfail_keeps_question_and_flags_failure() {
        let mut query = dns_client::build_query("oa.risencn.com", 0x1234).unwrap();
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
    #[cfg(any(target_os = "macos", target_os = "windows"))]
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
                    vec![SocketAddrV4::new(
                        match upstream_addr.ip() {
                            std::net::IpAddr::V4(v4) => v4,
                            _ => unreachable!(),
                        },
                        upstream_addr.port(),
                    )],
                    "utun-test".to_string(),
                    0,
                    dynamic,
                    Arc::new(AtomicUsize::new(0)),
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
                resp.extend_from_slice(&[10, 8, 3, 17]);
                let _ = upstream.send_to(&resp, from);
                let _ = upstream_addr;
            })
        };

        let client = UdpSocket::bind("127.0.0.1:0").expect("绑定客户端");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设置超时");
        let query = dns_client::build_query("oa.risencn.com", 0x4321).expect("构造查询");
        client.send_to(&query, stub_addr).expect("发给本机转发器");
        let mut buf = [0u8; 1024];
        let (len, _) = client.recv_from(&mut buf).expect("应当收到应答");
        assert_eq!(
            u16::from_be_bytes([buf[0], buf[1]]),
            0x4321,
            "ID 要原样带回"
        );
        assert_eq!(buf[2] & 0x80, 0x80, "QR=1");
        let addresses = dns_client::parse_a_records(&buf[..len]);
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].to_string(), "10.8.3.17");

        upstream_worker.join().expect("假上游线程");
        stop.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }

    /// Windows 的按域名解析：主动向 VPN DNS 查 A 记录，答什么就要拿到什么。
    #[cfg(target_os = "windows")]
    #[test]
    fn query_a_records_reads_the_vpn_dns_answer() {
        let upstream = UdpSocket::bind("127.0.0.1:0").expect("绑定假上游");
        let upstream_addr = upstream.local_addr().expect("上游地址");
        let worker = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (len, from) = upstream.recv_from(&mut buf).expect("上游应收到查询");
            let mut resp = buf[..len].to_vec();
            resp[2] |= 0x80;
            resp[6..8].copy_from_slice(&1u16.to_be_bytes());
            resp.extend_from_slice(&[0xC0, 0x0C]);
            resp.extend_from_slice(&[0x00, 0x01]);
            resp.extend_from_slice(&[0x00, 0x01]);
            resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]);
            resp.extend_from_slice(&[0x00, 0x04]);
            resp.extend_from_slice(&[10, 8, 3, 17]);
            let _ = upstream.send_to(&resp, from);
        });
        let got = query_a_records(
            "oa.risencn.com",
            &[SocketAddrV4::new(
                match upstream_addr.ip() {
                    std::net::IpAddr::V4(v4) => v4,
                    _ => unreachable!(),
                },
                upstream_addr.port(),
            )],
        );
        worker.join().expect("假上游线程");
        assert_eq!(got, vec![Ipv4Addr::new(10, 8, 3, 17)]);
    }

    #[test]
    fn dynamic_ips_skip_addresses_that_must_not_be_routed() {
        assert!(usable_tunnel_ip(Ipv4Addr::new(10, 8, 3, 17)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(224, 0, 0, 1)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(169, 254, 1, 1)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(255, 255, 255, 255)));
    }

    #[test]
    fn plan_absorbs_server_pushed_scope() {
        // 服务端下发的默认域名 + split-include 网段都要能被并入计划（这套校验和
        // 规则里的域名/网段完全一致），重复的不重复加。
        let mut plan = SplitPlan::default();
        assert!(plan.add_domain("RisenCN.com."));
        assert!(!plan.add_domain("risencn.com"), "重复域名不该重复加");
        assert!(
            !plan.add_domain("*.risencn.com"),
            "通配写法归一化后仍是同一个"
        );
        assert!(plan.add_cidr("192.168.136.7/22"));
        assert_eq!(plan.cidrs, vec!["192.168.136.0/22"], "网段要归一化");
        assert!(
            !plan.add_cidr("192.168.136.9/22"),
            "归一化后同一个网段不重复加"
        );
        assert!(!plan.add_cidr("not-a-cidr"));
        assert_eq!(plan.domains, vec!["risencn.com"]);
    }

    #[test]
    fn resolver_file_points_at_local_forwarder() {
        assert!(resolver_file_content().contains("nameserver 127.0.0.1"));
    }
}
