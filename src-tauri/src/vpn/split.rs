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
//! | `直连` 例外 | 转发器按查询名判 [`SplitPlan::tunnels`]，例外不注入 `/32` | 同左；回退路径的系统缓存扫描同一判定 |
//!
//! 严格规则模式：分流计划只由用户规则决定。服务端下发的默认域名
//! （`CISCO_DEF_DOMAIN`）与 split-include 网段**不在这里注入**，只由 worker
//! 写进 `status.server_scope` 供界面展示，避免出现「规则空着却通内网」的隐式行为。
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
//! - 域名只覆盖「系统解析器（getaddrinfo）」这条路：自带 DoH/DoT 的应用不受控；
//! - 网段（`IP-CIDR`）规则已下线：只能整段注入、无法按网段排除，与其半生效不如不做。

use std::collections::{BTreeMap, BTreeSet};
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
/// Windows：系统 DNS 缓存多久扫一次。
///
/// 回退路径下用户态看不到子域查询，但应用解析过的名字会留在 DNS Client 缓存里；
/// 每 3 秒扫一遍，`DOMAIN-SUFFIX` 的任意子域最多晚 3 秒拿到 `/32` 路由，
/// 而不是「只能解析规则里写死的字面域名」。
#[cfg(target_os = "windows")]
const CACHE_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Windows：每扫几次缓存做一轮字面域名的主动解析（15 秒 / 3 秒 = 5）
#[cfg(target_os = "windows")]
const CACHE_ROUNDS_PER_RESOLVE: u64 = 5;
/// Windows：确认「NRPT → 127.0.0.1:53」真的把查询送进来要等多久。
///
/// 1.5 秒在实机上偏紧：刚写完 NRPT 规则时 DNS Client 还要刷新策略表，
/// 太短会把「只是慢一点」误判成「loopback 不通」。
#[cfg(target_os = "windows")]
const PROBE_TIMEOUT: Duration = Duration::from_millis(2500);
/// Windows：一轮解析最多等多久（域名多、并发线程多时兜底）
#[cfg(target_os = "windows")]
const RESOLVE_ROUND_DEADLINE: Duration = Duration::from_secs(5);

/// 本会话见过的 DNS 应答：查询名 -> A 记录。
///
/// 为什么需要它：`/32` 只在转发器看到 DNS 应答时才注入，而应用（浏览器、
/// 系统解析器）会缓存答案。规则热更新把某个名字**改成走 VPN**（例如删掉
/// 「直连」例外）时，应用很可能不再发查询，于是永远等不到注入时机，表现
/// 就是「规则改了但没生效，断开重连才好」。这里把见过的答案记下来，重新
/// 布线时按新规则把该进隧道的地址补注入。
pub type DnsMemo = Arc<Mutex<BTreeMap<String, BTreeSet<Ipv4Addr>>>>;

/// 记账上限：够现场排障用就行，不为一条账目做 LRU（超了整体清空重来）。
const MEMO_MAX_NAMES: usize = 1024;

// ============================== 规则 → 计划 ==============================

/// 一条域名规则的数据面语义：匹配范围 + 走向。
///
/// 和用户规则一一对应，只是已经归一化：`DOMAIN` 只匹配自身，`DOMAIN-SUFFIX`
/// 匹配自身和任意子域；`policy` 决定命中后走 VPN 还是直连（直连 = 例外）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainRule {
    /// 小写、去通配符/尾点的域名
    pub pattern: String,
    /// true = `DOMAIN`（精确匹配），false = `DOMAIN-SUFFIX`（含子域）
    pub exact: bool,
    pub policy: Policy,
}

/// 规则翻译成「路由注入模式能做的事」。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SplitPlan {
    /// 域名规则全集（走 VPN 的 + 直连例外），保留用户规则里的先后顺序
    pub rules: Vec<DomainRule>,
    /// 需要按域名分流解析的域名（只含走 VPN 的，小写、去通配符）
    pub domains: Vec<String>,
    /// 这个模式下做不到的规则，以及需要用户知道的行为差异
    pub notes: Vec<String>,
}

impl SplitPlan {
    /// 一个域名最终走不走隧道。
    ///
    /// 取「最具体」的命中规则：pattern 越长越具体；长度相同时 `DOMAIN` 比
    /// `DOMAIN-SUFFIX` 具体；仍然相同时先出现的规则胜出。这条语义让「直连」
    /// 规则能当例外用——`DOMAIN hlwapi.example.com → 直连` 会盖住父级
    /// `DOMAIN-SUFFIX example.com → 走 VPN`，不需要任何域名特判。
    pub fn tunnels(&self, name: &str) -> bool {
        tunnels_domain(&self.rules, name)
    }
}

/// 同 [`SplitPlan::tunnels`]，给转发器和系统缓存扫描共用（它们只带规则集）。
pub fn tunnels_domain(rules: &[DomainRule], name: &str) -> bool {
    let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    let mut best: Option<(usize, Policy)> = None;
    for rule in rules {
        let Some(score) = match_score(rule, &name) else {
            continue;
        };
        match best {
            // `>=`：分数相同时先出现的规则胜出
            Some((best_score, _)) if best_score >= score => {}
            _ => best = Some((score, rule.policy)),
        }
    }
    matches!(best, Some((_, Policy::Vpn)))
}

/// 这个名字是否落在某条域名规则里（不管走向）。
///
/// 记账（[`DnsMemo`]）时用它筛掉与规则无关的缓存条目：Windows 回退路径扫的是
/// 整个 DNS 缓存，不筛的话 `www.baidu.com` 这类噪音也会占额度、还会被 1024 条
/// 上限清掉真正有用的记录。
#[cfg(any(target_os = "windows", test))]
fn covered_by_rules(rules: &[DomainRule], name: &str) -> bool {
    rules.iter().any(|rule| match_score(rule, name).is_some())
}

/// 规则对某个名字的命中分：没命中返回 `None`，命中返回「具体度」。
///
/// 具体度 = 域名长度 × 2 + 是否完整域名——长度按字符算，但域名都是 ASCII
/// （[`normalize_domain`] 保证了），所以等价于标签层级；同一域名下
/// `DOMAIN`（+1）比 `DOMAIN-SUFFIX` 具体。
fn match_score(rule: &DomainRule, name: &str) -> Option<usize> {
    let exact = name == rule.pattern;
    let hit = exact || (!rule.exact && name.ends_with(&format!(".{}", rule.pattern)));
    hit.then(|| rule.pattern.len() * 2 + usize::from(rule.exact))
}

/// 把规则集翻译成路由注入计划。
///
/// 「走 VPN」的规则决定哪些流量进隧道；「直连」的域名规则是**例外**，
/// 用更具体的匹配把某个子域从父级后缀里摘出来。整条链路（DNS 分流、`/32`
/// 注入、Windows 缓存扫描）都按 [`SplitPlan::tunnels`] 的结论走，所以例外
/// 不会出现「解析走了 VPN、路由却没装上」这种半截状态。
///
/// `IP-CIDR` 规则已下线：`RuleKind::parse` 会拒绝它，并给出一条说明性提示
/// （网段路由只能整段注入、无法按网段排除，半生效不如不做）。
pub fn plan(rules: &[VpnRule], default_policy: &str) -> SplitPlan {
    let mut out = SplitPlan::default();
    for rule in rules {
        if !rule.enabled {
            continue;
        }
        let policy = match Policy::parse(&rule.policy) {
            Ok(policy) => policy,
            Err(e) => {
                push_unique(
                    &mut out.notes,
                    format!("{} {}：{}", rule.kind, rule.value, e),
                );
                continue;
            }
        };
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
            RuleKind::Domain | RuleKind::DomainSuffix => {
                let Some(domain) = normalize_domain(&rule.value) else {
                    push_unique(
                        &mut out.notes,
                        format!("{} {} 不是合法域名，已跳过", kind.as_str(), rule.value),
                    );
                    continue;
                };
                let exact = matches!(kind, RuleKind::Domain);
                if matches!(policy, Policy::Vpn) {
                    push_unique(&mut out.domains, domain.clone());
                }
                // 同一条规则写两遍只留一条；不同走向的同名规则都保留，交给
                // 「具体度相同时先出现的胜出」去定胜负。
                let duplicate = out
                    .rules
                    .iter()
                    .any(|item| item.pattern == domain && item.exact == exact && item.policy == policy);
                if !duplicate {
                    out.rules.push(DomainRule {
                        pattern: domain,
                        exact,
                        policy,
                    });
                }
            }
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
    /// 域名规则全集（含直连例外）：注入 `/32` 前用它判断这个地址该不该进隧道
    rules: Vec<DomainRule>,
    /// VPN 服务器自己的公网 IP：绝不能把它们的路由指向隧道，否则隧道会自己抓自己
    server_ips: Vec<Ipv4Addr>,
    /// 本会话见过的 DNS 应答，规则热更新时用来补注入
    memo: DnsMemo,
    tun_dev: String,
    /// Windows：隧道接口索引；macOS 恒为 0（那边按接口名操作）
    tun_index: u32,
    vpn_dns: Vec<Ipv4Addr>,
    /// 固定要维护的路由（VPN DNS 的 `/32`）
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
        server_ips: &[Ipv4Addr],
        memo: DnsMemo,
        plan: SplitPlan,
    ) -> Result<Self, String> {
        if plan.domains.is_empty() {
            return Err("规则里没有可用的分流动作：请加一条「域名后缀 → 走 VPN」".to_string());
        }
        // 上一轮被 SIGKILL 时留下的解析器配置先清掉，否则清不干净的域名
        // 会一直指向一个已经断掉的 DNS（macOS）或一条过期规则（Windows）。
        cleanup_stale(session);

        // 直连例外会跟着父级后缀进本机转发器，但不会被注入 `/32`；日志里点名，
        // 免得现场看到「这个域名的查询走了 VPN DNS」就以为分流没按规则走。
        let exceptions: Vec<String> = plan
            .rules
            .iter()
            .filter(|rule| matches!(rule.policy, Policy::Direct))
            .map(|rule| rule.pattern.clone())
            .collect();
        if !exceptions.is_empty() {
            session.append_log(&format!(
                "[分流] 直连例外：{}（查询仍由本机转发器代答，但不为它们注入隧道路由）",
                exceptions.join(", ")
            ));
        }

        if vpn_dns.is_empty() {
            return Err("服务端没有下发 DNS 地址，内网域名无法解析".to_string());
        }
        let mut me = SplitTunnel {
            domains: plan.domains.clone(),
            rules: plan.rules.clone(),
            server_ips: server_ips.to_vec(),
            memo: Arc::clone(&memo),
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

        if let Err(e) = me.install_routes() {
            me.teardown();
            return Err(e);
        }
        // 规则热更新：应用可能缓存着旧答案、不再发查询，这里按记过的答案
        // 把「现在该走隧道」的地址补注入 `/32`（现场：删掉直连例外后仍直连）。
        // 放在启动转发器之前，免得和「刚好这时来一条查询」抢着装同一条路由。
        let restored = remembered_injections(&me.memo, &me.rules, &me.server_ips);
        if !restored.is_empty() {
            session.append_log(&format!(
                "[分流] 按记过的 DNS 答案补注入 {} 个地址（应用可能还缓存着旧答案）",
                restored.len()
            ));
            inject_ips(&restored, &me.tun_dev, me.tun_index, &me.dynamic, session);
        }
        if let Err(e) = me.install_resolver(session) {
            me.teardown();
            return Err(e);
        }
        if let Err(e) = me.start_resolver(session) {
            me.teardown();
            return Err(e);
        }
        flush_dns_cache();
        Ok(me)
    }

    fn tun_index(&self) -> u32 {
        self.tun_index
    }

    /// VPN DNS：装成指向隧道的固定 `/32` 路由（否则 DNS 查询出不去）。
    fn install_routes(&mut self) -> Result<(), String> {
        let mut wanted: Vec<String> = Vec::new();
        for ip in &self.vpn_dns {
            wanted.push(format!("{}/32", ip));
        }
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
        let rules = self.rules.clone();
        let server_ips = self.server_ips.clone();
        let memo = Arc::clone(&self.memo);
        let session = session.clone();
        let handle = std::thread::Builder::new()
            .name("ws-dns-forwarder".to_string())
            .spawn(move || {
                forward_loop(
                    socket,
                    ForwarderCtx {
                        upstreams,
                        tun_dev,
                        tun_index,
                        dynamic,
                        queries,
                        stop,
                        rules,
                        server_ips,
                        memo,
                        session,
                    },
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
                    Ok(count) if count > 0 => {
                        self.nrpt_installed = true;
                        self.local_dns_proxy = true;
                        flush_dns_cache();
                        if self.local_proxy_receives_queries() {
                            session.append_log(&format!(
                                "[分流] NRPT 已写好 {} 条规则（域名 + 前导点子域后缀），\
                                 查询交给本机转发器 {}；按应答注入内网 /32 路由\
                                 （DOMAIN-SUFFIX 覆盖任意子域）",
                                count, FORWARDER_ADDR
                            ));
                            return Ok(());
                        }
                        // NRPT 写成功了，但系统没有把（子域）查询送进 loopback：
                        // 不能假装成功，先说明白再回退。
                        session.append_log(&format!(
                            "[分流] NRPT 已指向 {}，但 {}ms 内没有查询进入本机转发器；\
                             回退到「NRPT 直连 VPN DNS + 后台解析」",
                            FORWARDER_ADDR,
                            PROBE_TIMEOUT.as_millis()
                        ));
                        // 把系统实际存下来的命名空间打进日志：带前导点的才覆盖子域，
                        // 现场能据此判断是「规则没点」还是「loopback 不通」。
                        if let Ok(namespaces) = super::win_dns::installed_namespaces() {
                            session.append_log(&format!(
                                "[分流] 当前 NRPT 命名空间：{:?}（只有带前导点的才匹配子域）",
                                namespaces
                            ));
                        }
                        let _ = super::win_dns::remove_all_rules();
                        self.nrpt_installed = false;
                        self.stop_local_forwarder();
                    }
                    Ok(_) => {
                        session.append_log(
                            "[分流] NRPT 规则一条都没写进去（域名与子域后缀都被系统拒绝）；\
                             回退到「NRPT 直连 VPN DNS + 后台解析」",
                        );
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
        // 主动解析 + 扫系统 DNS 缓存补齐子域路由；这是保底，不再是首选。
        match super::win_dns::apply_namespace_rules(&self.domains, &self.vpn_dns) {
            Ok(count) if count > 0 => {
                self.nrpt_installed = true;
                session.append_log(&format!(
                    "[分流] 已写好 {} 条 NRPT 规则（域名 + 前导点子域后缀），命中即交给 VPN DNS {:?}；\
                     子域地址从系统 DNS 缓存补齐",
                    count, self.vpn_dns
                ));
                Ok(())
            }
            Ok(_) => {
                // 计数器为 0 有可能是脚本输出没解析出来；再用一次列表确认，避免误判。
                if super::win_dns::installed_domains()
                    .map(|domains| domains.is_empty())
                    .unwrap_or(true)
                {
                    Err("NRPT 规则一条都没写进去：域名交给 VPN DNS 的布线没有生效".to_string())
                } else {
                    self.nrpt_installed = true;
                    session.append_log(&format!(
                        "[分流] 已为 {} 个域名配置按域名解析（NRPT），命中即交给 VPN DNS {:?}",
                        self.domains.len(),
                        self.vpn_dns
                    ));
                    Ok(())
                }
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

    /// 用一条随机子域查询确认「NRPT -> 127.0.0.1:53」真的能把**子域**查询送进来。
    ///
    /// 只看 NRPT 写成功不够，两个原因：
    /// 1. 带前导点的后缀命名空间可能被系统拒绝（只剩精确规则，子域根本不会进来）；
    /// 2. Windows 对 loopback DNS 的支持在不同版本/策略下不一致。
    /// 探针用的是 `随机子域.域名`，只有后缀规则生效才会命中；没查询进来就回退，
    /// 而不是让用户以为子域分流已经生效。
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
        let _ = rx.recv_timeout(PROBE_TIMEOUT);
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

    /// 起后台解析线程（Windows，回退路径）。
    ///
    /// NRPT 把查询交给系统 DNS Client，用户态看不到应答，所以拿不到「应用刚解析了
    /// 哪个地址」这个时机。这里用两条腿补：
    /// - 每 [`CACHE_POLL_INTERVAL`] 扫一遍系统 DNS 缓存，把应用刚解析过的**任意子域**
    ///   （只要属于规则里的域名）补成 `/32` 路由 —— `DOMAIN-SUFFIX` 因此不再要求
    ///   用户逐条写完整域名；
    /// - 每 [`RESOLVE_INTERVAL`] 直接向 VPN DNS 主动解析规则里的字面域名，覆盖
    ///   「还没人访问过」和「服务端轮询地址」两种情况。
    #[cfg(target_os = "windows")]
    fn start_resolve_thread(&mut self, session: &VpnSession) -> Result<(), String> {
        let domains = self.tunnel_domains();
        let rules = self.rules.clone();
        let server_ips = self.server_ips.clone();
        let memo = Arc::clone(&self.memo);
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
            .spawn(move || {
                resolve_loop(
                    domains,
                    rules,
                    server_ips,
                    memo,
                    upstreams,
                    tun_dev,
                    tun_index,
                    dynamic,
                    stop,
                    log,
                )
            })
            .map_err(|e| format!("启动按域名解析线程失败：{}", e))?;
        self.thread = Some(handle);
        Ok(())
    }

    /// 后台自检：按应用真实路径，经系统解析器解析规则里的域名。
    ///
    /// 这条链路是「系统解析器 -> 按域名分流 -> 隧道 -> VPN DNS」，
    /// 任一段断了都能在 worker.log 里一眼看出来。
    pub fn spawn_self_check(&self, session: VpnSession) {
        let domains = self.tunnel_domains();
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

    /// 需要主动解析 / 自检的域名：走 VPN 的规则里，本身没被「直连例外」盖掉的。
    ///
    /// 只影响「我们主动去查谁」，不影响 resolver/NRPT 的安装范围——父级后缀
    /// 即使裸域名被排除，子域仍可能需要分流解析。
    fn tunnel_domains(&self) -> Vec<String> {
        self.domains
            .iter()
            .filter(|domain| tunnels_domain(&self.rules, domain))
            .cloned()
            .collect()
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

/// 转发线程的运行参数。
///
/// 单独拧出来是因为 `forward_loop` 原来有 8 个位置参数，调用点很难看出哪个是
/// 域名清单、哪个是动态路由集合；收成一个结构体后，线程闭包里只搬一份。
#[cfg(any(target_os = "macos", target_os = "windows"))]
struct ForwarderCtx {
    upstreams: Vec<SocketAddrV4>,
    tun_dev: String,
    tun_index: u32,
    dynamic: Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    queries: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    /// 域名规则全集：按查询名字判断该不该注入 `/32`（直连例外在这里生效）
    rules: Vec<DomainRule>,
    /// VPN 服务器自己的公网 IP：跳过，不给它们注入隧道路由
    server_ips: Vec<Ipv4Addr>,
    /// 见过的 DNS 应答（与 SplitTunnel 共享）
    memo: DnsMemo,
    session: VpnSession,
}

/// 转发线程主体（macOS / Windows）：收本机 DNS Client 的查询 -> 经隧道问 VPN DNS
/// -> 注入路由 -> 回包。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn forward_loop(socket: UdpSocket, ctx: ForwarderCtx) {
    // 收包超时自己设一遍：调用方忘了设的话，`recv_from` 会一直阻塞，
    // stop 标志检查不到，teardown 里的 join 就永远回不来。
    let _ = socket.set_read_timeout(Some(POLL_TIMEOUT));
    let upstream = match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(e) => {
            ctx.session
                .append_log(&format!("[分流] 转发器打不开上游 socket：{}", e));
            return;
        }
    };
    let _ = upstream.set_read_timeout(Some(UPSTREAM_TIMEOUT));
    let mut query = [0u8; 1500];
    let mut reply = [0u8; 4096];
    while !ctx.stop.load(Ordering::Relaxed) {
        let (len, from) = match socket.recv_from(&mut query) {
            Ok(item) => item,
            Err(_) => continue, // 超时：回头看一眼 stop 标志
        };
        if len < 12 {
            continue;
        }
        ctx.queries.fetch_add(1, Ordering::Relaxed);
        let mut answer: Option<Vec<u8>> = None;
        for dns in &ctx.upstreams {
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
                install_dynamic_routes(&resp, &ctx);
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
fn install_dynamic_routes(packet: &[u8], ctx: &ForwarderCtx) {
    let ForwarderCtx {
        tun_dev,
        tun_index,
        dynamic,
        rules,
        server_ips,
        memo,
        session,
        ..
    } = ctx;
    let answers = dns_client::parse_a_records(packet);
    // 见过的答案一律记账（包括直连例外的）：应用缓存着旧答案时不会再发查询，
    // 规则热更新要靠这份账把「现在该进隧道的名字」补注入 `/32`。
    // 问题名解析不出来时只按老行为处理（宁可多注入，也不要漏掉分流）。
    if let Some(name) = dns_client::parse_question_name(packet) {
        record_answers(memo, &name, &answers);
        // 直连例外的查询也会进到转发器（它在父级后缀的 resolver/NRPT 范围里）：
        // 应答照常回给应用，但不能按它注入 `/32`，否则「直连」就变成走 VPN 了。
        if !tunnels_domain(rules, &name) {
            return;
        }
    }
    let candidates: Vec<Ipv4Addr> = {
        let set = match dynamic.lock() {
            Ok(set) => set,
            Err(_) => return,
        };
        let mut out = Vec::new();
        for ip in answers {
            if injectable(ip, server_ips) && !set.contains(&ip) && !out.contains(&ip) {
                out.push(ip);
            }
        }
        out
    };
    if candidates.is_empty() {
        return;
    }
    inject_ips(&candidates, tun_dev, *tun_index, dynamic, session);
}

/// 记下「查询名 -> 见过的 A 记录」，供规则热更新时补注入。空答案（NXDOMAIN、
/// 只有 AAAA 等）不记，免得白占记账额度。
fn record_answers(memo: &DnsMemo, name: &str, ips: &[Ipv4Addr]) {
    if ips.is_empty() {
        return;
    }
    let Ok(mut map) = memo.lock() else {
        return;
    };
    if !map.contains_key(name) && map.len() >= MEMO_MAX_NAMES {
        map.clear();
    }
    map.entry(name.to_string()).or_default().extend(ips.iter().copied());
}

/// 规则热更新后要立刻补注入的地址：记过的名字里，按新规则该走隧道的。
fn remembered_injections(
    memo: &DnsMemo,
    rules: &[DomainRule],
    server_ips: &[Ipv4Addr],
) -> Vec<Ipv4Addr> {
    let Ok(map) = memo.lock() else {
        return Vec::new();
    };
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for (name, ips) in map.iter() {
        if !tunnels_domain(rules, name) {
            continue;
        }
        for ip in ips {
            if injectable(*ip, server_ips) && !out.contains(ip) {
                out.push(*ip);
            }
        }
    }
    out
}

/// 把一组地址装成 `/32` 隧道路由（macOS / Windows 的具体做法不同）。
///
/// Windows 必须一条 PowerShell 批量装：逐条起进程会把 DNS 回包拖到超时。
/// 故意不挂平台的 `cfg`：`SplitTunnel::apply` 要能在没有路由实现的平台上编译
/// （那里连 `install_routes` 都会失败，这里保持空实现即可）。
fn inject_ips(
    ips: &[Ipv4Addr],
    tun_dev: &str,
    tun_index: u32,
    dynamic: &Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    session: &VpnSession,
) {
    if ips.is_empty() {
        return;
    }
    let _ = (tun_dev, tun_index);

    #[cfg(target_os = "windows")]
    {
        let cidrs: Vec<String> = ips.iter().map(|ip| format!("{}/32", ip)).collect();
        match route::ensure_onlink_routes(&cidrs, tun_index, 1) {
            Ok(()) => {
                if let Ok(mut set) = dynamic.lock() {
                    for ip in ips {
                        set.insert(*ip);
                    }
                }
                for ip in ips {
                    session.append_log(&format!("[分流] 内网 IP {} 已注入隧道 {}", ip, tun_dev));
                }
            }
            Err(e) => session.append_log(&format!(
                "[分流] 内网 IP {} 注入隧道失败：{}",
                ips.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                e
            )),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (ips, tun_dev, tun_index, dynamic, session);
    }

    #[cfg(target_os = "macos")]
    {
        for ip in ips {
            let cidr = format!("{}/32", ip);
            match add_tunnel_route(&cidr, tun_dev, 0) {
                Ok(()) => {
                    if let Ok(mut set) = dynamic.lock() {
                        set.insert(*ip);
                    }
                    session.append_log(&format!("[分流] 内网 IP {} 已注入隧道 {}", ip, tun_dev));
                }
                Err(e) => session.append_log(&format!("[分流] 内网 IP {} 注入隧道失败：{}", ip, e)),
            }
        }
    }
}

/// 解析出来的地址能不能装 `/32` 隧道路由。
///
/// 除了不可路由的地址，还要排掉 VPN 服务器自己的公网 IP：给服务器 IP 装上隧道
/// 路由，等于把隧道的 TLS/DTLS 流量再塞回隧道，直接打环（现场日志里出现过
/// 「内网 IP 203.0.113.10 已注入隧道」，那是 VPN 服务器地址）。
fn injectable(ip: Ipv4Addr, server_ips: &[Ipv4Addr]) -> bool {
    usable_tunnel_ip(ip) && !server_ips.contains(&ip)
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

/// 后台解析线程主体（Windows）：扫系统 DNS 缓存 + 定期主动解析 -> 注入隧道路由。
#[cfg(target_os = "windows")]
fn resolve_loop(
    domains: Vec<String>,
    rules: Vec<DomainRule>,
    server_ips: Vec<Ipv4Addr>,
    memo: DnsMemo,
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
    let mut cache_error: Option<String> = None;
    let mut round: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut found: Vec<Ipv4Addr> = Vec::new();
        // 回退路径下用户态看不到子域查询，但 DNS Client 的缓存里有应用刚解析过的
        // 名字。每轮扫一遍，`DOMAIN-SUFFIX` 的任意子域也能补进隧道。
        match scan_system_dns_cache(&rules, &server_ips, &dynamic, &memo, &session) {
            Ok(ips) => found.extend(ips),
            Err(e) => {
                if cache_error.as_deref() != Some(e.as_str()) {
                    session.append_log(&format!(
                        "[分流] 读取系统 DNS 缓存失败：{}（只影响子域自动补路由）",
                        e
                    ));
                    cache_error = Some(e);
                }
            }
        }
        if round % CACHE_ROUNDS_PER_RESOLVE == 0 {
            found.extend(resolve_all(&domains, &upstreams));
        }
        match sync_dynamic_routes(&found, tun_index, &tun_dev, &dynamic, &session, &server_ips) {
            Ok(added) => {
                if !last_error.is_empty() {
                    session.append_log("[分流] 内网地址注入隧道已恢复");
                    last_error.clear();
                }
                if first {
                    session.append_log(&format!(
                        "[分流] 按域名解析：{} 个域名，首批注入 {} 个内网地址；\
                         之后每 {} 秒扫一次系统 DNS 缓存（子域），每 {} 秒主动解析一次字面域名",
                        domains.len(),
                        added,
                        CACHE_POLL_INTERVAL.as_secs(),
                        RESOLVE_INTERVAL.as_secs()
                    ));
                    if added == 0 {
                        session.append_log(
                            "[分流] 首批没有拿到可用的内网地址：VPN DNS 可能经隧道不可达，\
                             或这些域名本身没有 A 记录；登录完成后浏览器解析过的子域\
                             会由系统 DNS 缓存补进隧道（最多晚几秒）",
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
        round += 1;
        // 分片睡眠：teardown 时最多 200ms 就能退出，不用等满一个扫描周期
        let deadline = std::time::Instant::now() + CACHE_POLL_INTERVAL;
        while std::time::Instant::now() < deadline {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// 扫系统 DNS Client 缓存，挑出该进隧道的 A 记录（Windows）。
///
/// 回退路径（NRPT 直连 VPN DNS）下应用解析过的名字会留在缓存里，这是唯一能在
/// 「应用拿到地址之后」补装 `/32` 的时机；最多晚一个 [`CACHE_POLL_INTERVAL`]。
/// 只返回还没注入过的地址，并顺手记一行「哪个子域带来的」，现场一眼能对上。
/// 直连例外在这里同样生效：规则判定为直连的名字不会补路由。
///
/// 缓存里的每一条（含直连例外的）都先进 [`DnsMemo`]：回退路径没有转发器，
/// 热更新又会在 teardown 里清空 DNS Client 缓存（`Clear-DnsClientCache`），
/// 只靠「等应用重新查询」的话删掉例外又要等到重连；记下账才能在重新布线时
/// 立刻补注入。
#[cfg(target_os = "windows")]
fn scan_system_dns_cache(
    rules: &[DomainRule],
    server_ips: &[Ipv4Addr],
    dynamic: &Arc<Mutex<BTreeSet<Ipv4Addr>>>,
    memo: &DnsMemo,
    session: &VpnSession,
) -> Result<Vec<Ipv4Addr>, String> {
    let value = crate::winps::run_json(&build_dns_cache_script())?;
    let known: BTreeSet<Ipv4Addr> = dynamic.lock().map(|set| set.clone()).unwrap_or_default();
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for (name, ip) in parse_dns_cache(value) {
        if covered_by_rules(rules, &name) {
            record_answers(memo, &name, &[ip]);
        }
        if !tunnels_domain(rules, &name) || !injectable(ip, server_ips) {
            continue;
        }
        if known.contains(&ip) || out.contains(&ip) {
            continue;
        }
        session.append_log(&format!(
            "[分流] 系统 DNS 缓存发现子域 {} -> {}，补注入隧道",
            name, ip
        ));
        out.push(ip);
    }
    Ok(out)
}

/// 扫缓存的 PowerShell：只要 A 记录（Type=1），字段固定是 `Entry` / `Data`。
#[cfg(target_os = "windows")]
fn build_dns_cache_script() -> String {
    "ConvertTo-Json -InputObject @(Get-DnsClientCache -ErrorAction SilentlyContinue | \
     Where-Object { $_.Type -eq 1 } | Select-Object Entry,Data) -Compress"
        .to_string()
}

/// 把 `Get-DnsClientCache` 的 JSON 解析成「名字 -> 可路由地址」。
///
/// 这里**不做规则过滤**：回退路径要先把缓存里看到的名字记进 [`DnsMemo`]
/// （含直连例外的），再由调用方按 [`tunnels_domain`] 决定注不注入。过滤逻辑
/// 只有一份，别在这里再抄一遍——两边不一致就会出现「例外在回退路径上失效」。
#[cfg(any(target_os = "windows", test))]
fn parse_dns_cache(value: serde_json::Value) -> Vec<(String, Ipv4Addr)> {
    let mut out: Vec<(String, Ipv4Addr)> = Vec::new();
    for item in crate::winps::json_array(value) {
        let Some(name) = item.get("Entry").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(data) = item.get("Data").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(ip) = data.trim().parse::<Ipv4Addr>() else {
            continue;
        };
        if !usable_tunnel_ip(ip) {
            continue;
        }
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        if out.iter().any(|(n, i)| *n == name && *i == ip) {
            continue;
        }
        out.push((name, ip));
    }
    out
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
    server_ips: &[Ipv4Addr],
) -> Result<usize, String> {
    let mut wanted: Vec<Ipv4Addr> = Vec::new();
    for ip in found {
        if injectable(*ip, server_ips) && !wanted.contains(ip) {
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

    /// 把缓存条目按规则过滤成「该进隧道」的部分（Windows 回退路径的判定）。
    fn tunnel_cache_entries(
        value: serde_json::Value,
        rules: &[DomainRule],
    ) -> Vec<(String, Ipv4Addr)> {
        parse_dns_cache(value)
            .into_iter()
            .filter(|(name, _)| tunnels_domain(rules, name))
            .collect()
    }

    /// 构造「一组走 VPN 的域名后缀规则」，给 `parse_dns_cache` 那类只看规则集的
    /// 函数用。
    fn suffix_rules(domains: &[&str]) -> Vec<DomainRule> {
        domains
            .iter()
            .map(|domain| DomainRule {
                pattern: (*domain).to_string(),
                exact: false,
                policy: Policy::Vpn,
            })
            .collect()
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
        assert!(plan.notes.is_empty(), "不该有提示：{:?}", plan.notes);
    }

    #[test]
    fn plan_keeps_direct_domains_as_exceptions() {
        let rules = vec![
            rule("DOMAIN-SUFFIX", "baidu.com", "DIRECT"),
            rule("DOMAIN-SUFFIX", "example.com", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        // 直连规则不进「要按域名分流解析」的名单，但会作为例外保留
        assert_eq!(plan.domains, vec!["example.com"]);
        assert_eq!(plan.rules.len(), 2);
        assert!(!plan.tunnels("www.baidu.com"));
    }

    /// 现场（macOS）：`DOMAIN-SUFFIX example.com → 走 VPN` 时，
    /// `DOMAIN hlwapi.example.com → 直连` 必须把这个子域从分流里摘出来。
    #[test]
    fn direct_rule_excludes_a_subdomain_from_a_suffix_rule() {
        let rules = vec![
            rule("DOMAIN", "hlwapi.example.com", "DIRECT"),
            rule("DOMAIN-SUFFIX", "example.com", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert!(
            !plan.tunnels("hlwapi.example.com"),
            "直连例外必须盖住父级后缀"
        );
        assert!(plan.tunnels("oa.example.com"));
        assert!(plan.tunnels("example.com"));
        // 例外本身不需要按域名分流解析
        assert_eq!(plan.domains, vec!["example.com"]);
    }

    #[test]
    fn exact_direct_rule_only_excludes_itself() {
        let plan = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.com", "VPN"),
                rule("DOMAIN", "hlwapi.example.com", "DIRECT"),
            ],
            "DIRECT",
        );
        assert!(!plan.tunnels("hlwapi.example.com"));
        assert!(
            plan.tunnels("x.hlwapi.example.com"),
            "完整域名例外只作用于它自己"
        );
    }

    #[test]
    fn suffix_direct_rule_excludes_the_whole_subtree() {
        let plan = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.com", "VPN"),
                rule("DOMAIN-SUFFIX", "hlwapi.example.com", "DIRECT"),
            ],
            "DIRECT",
        );
        assert!(!plan.tunnels("hlwapi.example.com"));
        assert!(!plan.tunnels("a.hlwapi.example.com"));
        assert!(plan.tunnels("oa.example.com"));
    }

    /// 例外的判定只看「谁更具体」，和规则在列表里排第几无关。
    #[test]
    fn exception_wins_by_specificity_not_by_order() {
        for direct_first in [true, false] {
            let direct = rule("DOMAIN", "hlwapi.example.com", "DIRECT");
            let vpn = rule("DOMAIN-SUFFIX", "example.com", "VPN");
            let rules = if direct_first {
                vec![direct, vpn]
            } else {
                vec![vpn, direct]
            };
            let plan = plan(&rules, "DIRECT");
            assert!(!plan.tunnels("hlwapi.example.com"), "顺序不该影响结论");
            assert!(plan.tunnels("oa.example.com"));
        }
    }

    /// 现场只拿 example.com 举例；换个域名排除行为必须一模一样。
    #[test]
    fn exclusion_is_generic_for_any_suffix() {
        let plan = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.org", "VPN"),
                rule("DOMAIN", "test.example.org", "DIRECT"),
            ],
            "DIRECT",
        );
        assert!(!plan.tunnels("test.example.org"));
        assert!(plan.tunnels("prod.example.org"));
        assert!(plan.tunnels("bate.example.org"));
    }

    /// Windows 回退路径扫系统 DNS 缓存时，直连例外的地址不能被补成隧道路由。
    #[test]
    fn dns_cache_skips_direct_exceptions() {
        let rules = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.com", "VPN"),
                rule("DOMAIN", "hlwapi.example.com", "DIRECT"),
            ],
            "DIRECT",
        )
        .rules;
        let value = serde_json::json!([
            {"Entry": "hlwapi.example.com", "Data": "203.0.113.11"},
            {"Entry": "oa.example.com", "Data": "192.0.2.41"},
        ]);
        let got = tunnel_cache_entries(value, &rules);
        assert_eq!(
            got,
            vec![(
                "oa.example.com".to_string(),
                "192.0.2.41".parse().unwrap()
            )]
        );
    }

    /// `IP-CIDR` 已下线：老配置里留着它时，计划里要给出「已下线」的说明，
    /// 而不是静默丢弃。
    #[test]
    fn retired_ip_cidr_is_reported() {
        let rules = vec![
            rule("DOMAIN-SUFFIX", "example.com", "VPN"),
            rule("IP-CIDR", "10.10.3.0/24", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert_eq!(plan.domains, vec!["example.com"]);
        assert!(
            plan.notes.iter().any(|note| note.contains("已下线")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn plan_explains_unsupported_kinds() {
        let rules = vec![
            rule("DOMAIN-KEYWORD", "corp", "VPN"),
            rule("PROCESS-NAME", "Lark", "VPN"),
            rule("DST-PORT", "8080", "VPN"),
            rule("IP-CIDR", "10.10.3.0/24", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert!(plan.domains.is_empty());
        assert_eq!(plan.notes.len(), 4, "每类不支持的规则都要给出一条提示");
        assert!(plan.notes[0].contains("DOMAIN-SUFFIX"));
    }

    #[test]
    fn plan_warns_when_default_policy_is_vpn() {
        let plan = plan(&[], "VPN");
        assert!(plan.notes.iter().any(|note| note.contains("全局模式")));
    }

    #[test]
    fn dns_cache_fills_subdomains_of_suffix_rules() {
        let value = serde_json::json!([
            // 子域：DOMAIN-SUFFIX 必须覆盖
            {"Entry": "oa.example.com", "Data": "192.0.2.41"},
            // 裸域名本身也在缓存里
            {"Entry": "example.com", "Data": "10.0.0.1"},
            // 同名字但后缀边界不同：不能被 example.com 命中
            {"Entry": "notexample.com", "Data": "10.0.0.2"},
            // 公网域名：不碰
            {"Entry": "www.baidu.com", "Data": "1.2.3.4"},
            // Data 不是 IPv4：忽略
            {"Entry": "bad.example.com", "Data": "vpn.example.com"},
            // 不可路由地址：忽略
            {"Entry": "loop.example.com", "Data": "127.0.0.1"},
            // 大小写 / 末尾点归一化
            {"Entry": "CAS.example.com.", "Data": "192.0.2.42"},
        ]);
        let got = tunnel_cache_entries(value, &suffix_rules(&["example.com"]));
        assert_eq!(
            got,
            vec![
                (
                    "oa.example.com".to_string(),
                    "192.0.2.41".parse().unwrap()
                ),
                ("example.com".to_string(), "10.0.0.1".parse().unwrap()),
                (
                    "cas.example.com".to_string(),
                    "192.0.2.42".parse().unwrap()
                ),
            ]
        );
    }

    /// 现场只拿 example.com 举例；换个域名必须一模一样 —— 代码里不许有域名特例。
    #[test]
    fn suffix_rule_is_generic_for_any_domain() {
        let rules = vec![rule("DOMAIN-SUFFIX", "example.org", "VPN")];
        assert_eq!(plan(&rules, "DIRECT").domains, vec!["example.org"]);

        let value = serde_json::json!([
            {"Entry": "test.example.org", "Data": "10.10.0.11"},
            {"Entry": "prod.example.org", "Data": "10.10.0.12"},
            {"Entry": "bate.example.org", "Data": "10.10.0.13"},
            {"Entry": "example.org", "Data": "10.10.0.10"},
            // 后缀边界：不能被 example.org 命中
            {"Entry": "notexample.org", "Data": "10.10.0.99"},
        ]);
        let got = tunnel_cache_entries(value, &suffix_rules(&["example.org"]));
        let names: Vec<&str> = got.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "test.example.org",
                "prod.example.org",
                "bate.example.org",
                "example.org"
            ]
        );
    }

    /// Windows 回退路径没有转发器，缓存里的名字是唯一的线索；而热更新的
    /// teardown 会 `Clear-DnsClientCache`，所以扫描时必须先记账，删掉直连例外
    /// 才能立刻补注入（否则又要等应用重新查询/重连）。
    #[test]
    fn fallback_cache_scan_feeds_the_memo() {
        let memo = DnsMemo::default();
        let cache = serde_json::json!([
            {"Entry": "hlwapi.example.com", "Data": "203.0.113.11"},
            {"Entry": "oa.example.com", "Data": "192.0.2.41"},
        ]);
        for (name, ip) in parse_dns_cache(cache) {
            record_answers(&memo, &name, &[ip]);
        }

        let with_exception = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.com", "VPN"),
                rule("DOMAIN", "hlwapi.example.com", "DIRECT"),
            ],
            "DIRECT",
        );
        assert_eq!(
            remembered_injections(&memo, &with_exception.rules, &[]),
            vec![Ipv4Addr::new(192, 0, 2, 41)]
        );

        let without_exception = plan(&[rule("DOMAIN-SUFFIX", "example.com", "VPN")], "DIRECT");
        assert_eq!(
            remembered_injections(&memo, &without_exception.rules, &[]),
            vec![
                Ipv4Addr::new(203, 0, 113, 11),
                Ipv4Addr::new(192, 0, 2, 41)
            ]
        );
    }

    /// 记账只看「落不落在某条规则里」：跟规则无关的缓存条目（公网域名）
    /// 不该占 [`MEMO_MAX_NAMES`] 的额度。
    #[test]
    fn memo_recording_only_covers_rule_names() {
        let rules = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.com", "VPN"),
                rule("DOMAIN", "hlwapi.example.com", "DIRECT"),
            ],
            "DIRECT",
        )
        .rules;
        assert!(covered_by_rules(&rules, "oa.example.com"));
        assert!(covered_by_rules(&rules, "hlwapi.example.com"));
        assert!(!covered_by_rules(&rules, "www.baidu.com"));
        assert!(!covered_by_rules(&rules, "notexample.com"));
    }

    #[test]
    fn dns_cache_ignores_unrelated_domains_and_duplicates() {
        let value = serde_json::json!([
            {"Entry": "a.example.com", "Data": "10.0.0.1"},
            {"Entry": "a.example.com", "Data": "10.0.0.1"},
            {"Entry": "b.other.com", "Data": "10.0.0.2"},
        ]);
        let got = tunnel_cache_entries(value, &suffix_rules(&["example.com"]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "a.example.com");
    }

    #[test]
    fn dns_cache_tolerates_null_and_empty_output() {
        assert!(parse_dns_cache(serde_json::Value::Null).is_empty());
        assert!(parse_dns_cache(serde_json::json!({"Entry": "a.example.com"})).is_empty());
    }

    #[test]
    fn normalize_domain_rejects_path_traversal_and_junk() {
        assert_eq!(
            normalize_domain("*.OA.example.com."),
            Some("oa.example.com".into())
        );
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
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn forwarder_relays_upstream_answer() {
        let dir = crate::test_support::TempDir::new("split-forwarder");
        let session = VpnSession::new(dir.path().to_path_buf());
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
                    ForwarderCtx {
                        upstreams: vec![SocketAddrV4::new(
                            match upstream_addr.ip() {
                                std::net::IpAddr::V4(v4) => v4,
                                _ => unreachable!(),
                            },
                            upstream_addr.port(),
                        )],
                        tun_dev: "utun-test".to_string(),
                        tun_index: 0,
                        dynamic,
                        queries: Arc::new(AtomicUsize::new(0)),
                        stop,
                        rules: suffix_rules(&["example.com"]),
                        server_ips: Vec::new(),
                        memo: DnsMemo::default(),
                        session,
                    },
                )
            })
        };

        // 假上游：收到查询后回一个带 A 记录的应答（问题段用压缩指针指回去）
        let upstream_worker = {
            let upstream = upstream.try_clone().expect("克隆上游 socket");
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
        assert_eq!(addresses[0].to_string(), "10.10.3.17");

        upstream_worker.join().expect("假上游线程");
        stop.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }

    /// 直连例外的应答也要记账：热更新把例外删掉后，靠这份账补注入 `/32`。
    ///
    /// 这条链路是现场 bug 的关键——应用缓存着旧答案、不再发查询，只能靠转发器
    /// 曾经看到的应答；例外名字的应答同样要有记录。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn forwarder_remembers_answers_even_for_direct_exceptions() {
        let dir = crate::test_support::TempDir::new("split-forwarder-memo");
        let session = VpnSession::new(dir.path().to_path_buf());
        let _ = session.ensure();

        let stub = UdpSocket::bind("127.0.0.1:0").expect("绑定测试转发器");
        let stub_addr = stub.local_addr().expect("转发器地址");
        let upstream = UdpSocket::bind("127.0.0.1:0").expect("绑定假上游");
        let upstream_addr = upstream.local_addr().expect("上游地址");

        let stop = Arc::new(AtomicBool::new(false));
        let dynamic = Arc::new(Mutex::new(BTreeSet::new()));
        let dynamic_probe = Arc::clone(&dynamic);
        let memo = DnsMemo::default();
        let memo_probe = Arc::clone(&memo);
        let rules = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.com", "VPN"),
                rule("DOMAIN", "hlwapi.example.com", "DIRECT"),
            ],
            "DIRECT",
        )
        .rules;
        let handle = {
            let stop = Arc::clone(&stop);
            let dynamic = Arc::clone(&dynamic);
            let memo = Arc::clone(&memo);
            let session = session.clone();
            std::thread::spawn(move || {
                forward_loop(
                    stub,
                    ForwarderCtx {
                        upstreams: vec![SocketAddrV4::new(
                            match upstream_addr.ip() {
                                std::net::IpAddr::V4(v4) => v4,
                                _ => unreachable!(),
                            },
                            upstream_addr.port(),
                        )],
                        tun_dev: "utun-test".to_string(),
                        tun_index: 0,
                        dynamic,
                        queries: Arc::new(AtomicUsize::new(0)),
                        stop,
                        rules,
                        server_ips: Vec::new(),
                        memo,
                        session,
                    },
                )
            })
        };

        let upstream_worker = {
            let upstream = upstream.try_clone().expect("克隆上游 socket");
            std::thread::spawn(move || {
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
                resp.extend_from_slice(&[10, 10, 3, 17]);
                let _ = upstream.send_to(&resp, from);
            })
        };

        let client = UdpSocket::bind("127.0.0.1:0").expect("绑定客户端");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设置超时");
        let query = dns_client::build_query("hlwapi.example.com", 0x5321).expect("构造查询");
        client.send_to(&query, stub_addr).expect("发给本机转发器");
        let mut buf = [0u8; 1024];
        let (len, _) = client.recv_from(&mut buf).expect("应当收到应答");
        assert_eq!(dns_client::parse_a_records(&buf[..len]).len(), 1);

        let injected = dynamic_probe.lock().map(|set| set.len()).unwrap_or(0);
        assert_eq!(injected, 0, "直连例外不该注入 /32");
        let remembered = memo_probe
            .lock()
            .map(|map| map.get("hlwapi.example.com").cloned().unwrap_or_default())
            .unwrap_or_default();
        assert!(
            remembered.contains(&Ipv4Addr::new(10, 10, 3, 17)),
            "例外名字的答案也要记账：{remembered:?}"
        );

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
            resp.extend_from_slice(&[10, 10, 3, 17]);
            let _ = upstream.send_to(&resp, from);
        });
        let got = query_a_records(
            "oa.example.com",
            &[SocketAddrV4::new(
                match upstream_addr.ip() {
                    std::net::IpAddr::V4(v4) => v4,
                    _ => unreachable!(),
                },
                upstream_addr.port(),
            )],
        );
        worker.join().expect("假上游线程");
        assert_eq!(got, vec![Ipv4Addr::new(10, 10, 3, 17)]);
    }

    /// 「只有 `DOMAIN-SUFFIX example.com → 走 VPN`」时，`hlwapi.example.com`
    /// 落回父级后缀规则：算**走 VPN**（例外一删就是这个状态）。
    #[test]
    fn a_subdomain_falls_back_to_the_suffix_rule() {
        let plan = plan(&[rule("DOMAIN-SUFFIX", "example.com", "VPN")], "DIRECT");
        assert!(plan.tunnels("hlwapi.example.com"));
        assert!(plan.tunnels("oa.example.com"));
        assert!(!plan.tunnels("hlwapi.example.com.cn"));
        assert!(!plan.tunnels("hlwapi.notexample.com"));
    }

    /// 规则热更新把某个名字从「直连例外」改回「走 VPN」后，要能按记过的
    /// DNS 答案立刻补注入——应用缓存着旧答案、不再发查询（现场 bug：
    /// 删掉例外后仍然直连，断开重连才正常）。
    #[test]
    fn hot_rule_change_reinjects_remembered_answers() {
        let memo = DnsMemo::default();
        record_answers(
            &memo,
            "hlwapi.example.com",
            &[Ipv4Addr::new(203, 0, 113, 11)],
        );
        record_answers(
            &memo,
            "oa.example.com",
            &[Ipv4Addr::new(192, 0, 2, 41)],
        );
        record_answers(&memo, "empty.example.com", &[]); // 空答案不该记账

        let with_exception = plan(
            &[
                rule("DOMAIN-SUFFIX", "example.com", "VPN"),
                rule("DOMAIN", "hlwapi.example.com", "DIRECT"),
            ],
            "DIRECT",
        );
        let stayed = remembered_injections(&memo, &with_exception.rules, &[]);
        assert!(
            !stayed.contains(&Ipv4Addr::new(203, 0, 113, 11)),
            "例外还在时 hlwapi 不能被补注入：{stayed:?}"
        );
        assert!(stayed.contains(&Ipv4Addr::new(192, 0, 2, 41)));

        let without_exception = plan(&[rule("DOMAIN-SUFFIX", "example.com", "VPN")], "DIRECT");
        let got = remembered_injections(&memo, &without_exception.rules, &[]);
        assert_eq!(
            got,
            vec![
                Ipv4Addr::new(203, 0, 113, 11),
                Ipv4Addr::new(192, 0, 2, 41),
            ]
        );
    }

    /// 补注入同样要守「VPN 服务器自己的地址不进隧道」。
    #[test]
    fn remembered_answers_skip_the_vpn_server() {
        let memo = DnsMemo::default();
        let server = Ipv4Addr::new(203, 0, 113, 10);
        record_answers(&memo, "vpn.example.com", &[server]);
        let plan = plan(&[rule("DOMAIN-SUFFIX", "example.com", "VPN")], "DIRECT");
        assert!(remembered_injections(&memo, &plan.rules, &[server]).is_empty());
    }

    #[test]
    fn dns_memo_is_capped() {
        let memo = DnsMemo::default();
        for i in 0..(MEMO_MAX_NAMES + 5) {
            record_answers(
                &memo,
                &format!("n{i}.example.test"),
                &[Ipv4Addr::new(10, 0, 0, 1)],
            );
        }
        let len = memo.lock().map(|map| map.len()).unwrap_or(0);
        assert!(len <= MEMO_MAX_NAMES, "记账要有上限：{len}");
    }

    #[test]
    fn dynamic_ips_skip_addresses_that_must_not_be_routed() {        assert!(usable_tunnel_ip(Ipv4Addr::new(10, 10, 3, 17)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(224, 0, 0, 1)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(169, 254, 1, 1)));
        assert!(!usable_tunnel_ip(Ipv4Addr::new(255, 255, 255, 255)));
    }

    /// VPN 服务器自己的公网 IP 不能被注入隧道，否则隧道的 TLS/DTLS 会打环。
    #[test]
    fn vpn_server_ip_is_never_injected_into_the_tunnel() {
        let server = Ipv4Addr::new(203, 0, 113, 10);
        assert!(!injectable(server, &[server]));
        assert!(injectable(Ipv4Addr::new(10, 10, 3, 17), &[server]));
        assert!(injectable(server, &[]));
        assert!(!injectable(Ipv4Addr::new(127, 0, 0, 1), &[server]));
    }

    #[test]
    fn resolver_file_points_at_local_forwarder() {
        assert!(resolver_file_content().contains("nameserver 127.0.0.1"));
    }
}
