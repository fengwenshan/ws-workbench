//! 提权 worker
//!
//! 由主进程通过计划任务（Windows）或 osascript（macOS）以管理员权限拉起，
//! 独立完成「建隧道 -> 下发分流路由 -> 守护 -> 回滚」的全过程。
//!
//! 之所以要独立进程：openconnect 建虚拟网卡、改路由表都需要管理员权限，
//! 而主程序保持普通权限体验更好；提权进程又拿不到父进程的管道，
//! 所以一切状态通过会话目录里的文件回传。

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crate::vpn::connector;
use crate::vpn::models::{NetworkSnapshot, ProxyMode, VpnPhase, VpnRequest, VpnRule, VpnStatus};
use crate::vpn::route;
use crate::vpn::script;
use crate::vpn::session::VpnSession;

/// 路由守护的巡检间隔
const GUARD_INTERVAL: Duration = Duration::from_secs(15);
/// 主循环节拍：只看控制文件与子进程状态，200ms 足够
const POLL_INTERVAL_MS: u64 = 200;
/// 等待隧道就绪的最长时间
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

/// worker 退出码
pub const EXIT_OK: i32 = 0;
pub const EXIT_NO_REQUEST: i32 = 2;
pub const EXIT_FAILED: i32 = 3;

struct Worker {
    session: VpnSession,
    request: VpnRequest,
    status: VpnStatus,
    snapshot: NetworkSnapshot,
    /// 连接时解析出的 VPN 服务器 IP，用来清理脚本残留的防环主机路由
    vpn_server_ips: Vec<String>,
    /// 安全阀把接管放弃掉时的原因：隧道仍然连着，只是不劫持系统流量
    degraded_reason: Option<String>,
    /// 路由注入式分流（规则模式）：不捕获、不抢默认路由
    split: Option<crate::vpn::split::SplitTunnel>,
    /// 全局模式：半默认路由 + DNS 都指向隧道
    full: Option<crate::vpn::fulltunnel::FullTunnel>,
    /// 服务端下发的 split-include 网段（「公司内网」最权威的覆盖范围）
    server_split_routes: Vec<String>,
    /// 服务端下发的默认域名（AnyConnect 的 default domain），有它就能自动覆盖整族子域
    server_def_domain: Option<String>,
    /// 最近几行输出，失败时作为诊断信息
    tail: Vec<String>,
    /// script.log 已经读过的行数（脚本的 stdout 接不到管道，只能靠这个文件回传）
    script_log_lines: usize,
    /// 当前生效的代理模式（可由主进程通过 mode 文件改）
    applied_mode: ProxyMode,
    /// 当前生效的规则与兜底走向（可由主进程通过 rules.json 改）
    current_rules: Vec<VpnRule>,
    current_default_policy: String,
    /// 上次读到的规则原文，内容没变就不重新编译
    last_rules_raw: Option<String>,
    last_mode_check: Instant,
    last_rules_check: Instant,
    last_alive_check: Instant,
    /// 心跳过期：主进程已经不在了，循环里看到就把隧道收掉
    parent_lost: bool,
}

impl Worker {
    fn new(session: VpnSession, request: VpnRequest) -> Self {
        let mut status = VpnStatus::default();
        status.profile_id = request.profile_id.clone();
        status.profile_name = request.profile_name.clone();
        status.conn_identity = request.conn_identity.clone();
        status.pid = std::process::id();
        status.started_at = now();
        status.updated_at = now();
        let applied_mode = ProxyMode::parse(&request.proxy_mode).unwrap_or(ProxyMode::Rule);
        // 让界面知道这条会话实际按哪个模式在跑（界面上的选择只代表「下次连接」）
        status.applied_mode = applied_mode.as_str().to_string();
        let current_rules: Vec<VpnRule> =
            request.rules.iter().filter(|r| r.enabled).cloned().collect();
        let current_default_policy = request.default_policy.clone();
        Worker {
            session,
            request,
            status,
            snapshot: NetworkSnapshot::default(),
            vpn_server_ips: Vec::new(),
            degraded_reason: None,
            split: None,
            full: None,
            server_split_routes: Vec::new(),
            server_def_domain: None,
            tail: Vec::new(),
            script_log_lines: 0,
            applied_mode,
            current_rules,
            current_default_policy,
            last_rules_raw: None,
            last_mode_check: Instant::now(),
            last_rules_check: Instant::now(),
            last_alive_check: Instant::now(),
            parent_lost: false,
        }
    }

    fn log(&self, line: &str) {
        self.session.append_log(line);
    }

    /// 更新阶段并落盘，主进程轮询 status.json 后推给界面
    fn set_phase(&mut self, phase: VpnPhase, message: &str) {
        self.status.phase = phase;
        self.status.message = message.to_string();
        self.status.updated_at = now();
        if let Err(e) = self.session.write_status(&self.status) {
            self.log(&format!("[worker] 写状态失败: {}", e));
        }
        self.log(&format!("[状态] {} {}", phase.label(), message));
    }

    fn fail(&mut self, message: &str) -> i32 {
        self.status.error = message.to_string();
        self.set_phase(VpnPhase::Failed, message);
        self.cleanup_routes();
        let _ = self.session.write_status(&self.status);
        EXIT_FAILED
    }

    /// 记录一行输出；同时从脚本回报里提取隧道参数
    fn handle_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        self.log(trimmed);
        self.tail.push(trimmed.to_string());
        if self.tail.len() > 40 {
            self.tail.remove(0);
        }

        if let Some((key, value)) = script::parse_marker_line(trimmed) {
            match key.as_str() {
                "tunidx" => self.status.tun_index = value.parse().unwrap_or(0),
                "tundev" => self.status.tun_dev = value,
                "ip4" => self.status.vpn_ip = value,
                "mask" => {
                    self.status.vpn_gateway = tunnel_next_hop(&self.status.vpn_ip, &value)
                }
                "mtu" => self.status.mtu = value.parse().unwrap_or(0),
                "dns" => {
                    self.status.dns = value
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect();
                }
                "done" => {}
                // 服务端下发的默认域名：按域名分流解析就能覆盖整族子域，
                // 不用用户把每个内网域名都列出来
                "defdomain" => {
                    let domain = value.trim().trim_end_matches('.').to_ascii_lowercase();
                    if !domain.is_empty() {
                        self.server_def_domain = Some(domain);
                    }
                }
                // 服务端下发的 split-include 网段：正规 VPN 客户端的做法就是把这些
                // 网段的路由指向隧道。这里只记下来，装配分流时统一注入。
                "split" => {
                    let cidr = value.trim();
                    if !cidr.is_empty() && !self.server_split_routes.iter().any(|item| item == cidr) {
                        self.server_split_routes.push(cidr.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    fn tunnel_ready(&self) -> bool {
        !self.status.vpn_ip.is_empty() && (self.status.tun_index > 0 || !self.status.tun_dev.is_empty())
    }

    /// 读取脚本自己写的 `script.log`，这是隧道参数的真正来源。
    ///
    /// openconnect 执行脚本时用的是 `CreateProcessW(..., bInheritHandles=FALSE, ...)`，
    /// 脚本的标准输出**没有**接到 openconnect 的管道上，所以我们从子进程 stdout 里
    /// 永远读不到脚本的回报。脚本因此把同一份回报追加写进会话目录的 script.log，
    /// 由这里增量读取——不这么做 `tunnel_ready()` 永远不会为真，界面就会一直停在
    /// 「连接中」，直到 CONNECT_TIMEOUT（120s）才报超时。
    fn ingest_script_log(&mut self) {
        let text = match std::fs::read_to_string(self.session.script_log_path()) {
            Ok(text) => text,
            Err(_) => return,
        };
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() <= self.script_log_lines {
            return;
        }
        // 只取新增的行：脚本每写一行就关一次文件，不会留下半行
        let fresh: Vec<String> = lines[self.script_log_lines..]
            .iter()
            .map(|line| line.to_string())
            .collect();
        self.script_log_lines = lines.len();
        for line in fresh {
            self.handle_line(&line);
        }
    }

    /// 找出 openconnect 报出的「脚本执行失败」原话。
    ///
    /// 注意 `script.c` 的坑：脚本**正常启动但退出码非 0** 时，它也走
    /// `ret < 0` 那条分支，并用一个过期的 `GetLastError()` 拼出
    /// `Failed to spawn script '…': 参数错误`，看起来像进程压根没起来。
    /// 脚本由本程序生成、内容固定，cscript 要么能跑要么不能跑，
    /// 失败一次后面也不会突然成功，所以见到就该立刻收工，不必白等超时。
    fn script_failure(&self) -> Option<String> {
        self.tail.iter().find_map(|line| {
            let lower = line.to_lowercase();
            if lower.contains("failed to spawn script")
                || lower.contains("returned error")
                || lower.contains("script did not complete")
            {
                Some(line.clone())
            } else {
                None
            }
        })
    }

    /// 最近输出的摘要，用作失败原因
    fn tail_summary(&self) -> String {
        let interesting: Vec<&String> = self
            .tail
            .iter()
            .filter(|line| {
                let lower = line.to_lowercase();
                lower.contains("failed")
                    || lower.contains("error")
                    || lower.contains("cannot")
                    || lower.contains("refused")
                    || lower.contains("unable")
            })
            .collect();
        if interesting.is_empty() {
            self.tail
                .iter()
                .rev()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        } else {
            interesting
                .iter()
                .rev()
                .take(3)
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        }
    }

    /// 把 openconnect 的经典失败翻译成「照着做就能过」的提示。
    ///
    /// openconnect 建网卡失败时只会吐一句干巴巴的 `Set up tun device failed`，
    /// 日志里看着像程序的锅，其实都是环境问题。内置的 9.21 自带 `wintun.dll`
    /// （建网卡时优先用 Wintun，失败才回退 TAP-Windows），正常情况**不需要**
    /// 另外装驱动，所以排在最前面的怀疑对象是权限和杀软，而不是「缺驱动」。
    fn tun_setup_hint(&self) -> Option<String> {
        let text = self.tail.join("\n").to_lowercase();
        // openconnect 认不出可用网卡时的原话，末尾就是「Is the driver installed?」
        let tun_failed = text.contains("set up tun device failed")
            || text.contains("failed to open tap")
            || text.contains("no tap-windows adapters")
            || text.contains("tap-windows adapter")
            || text.contains("is the driver installed");
        // 这条是 Wintun 专用的：创建网卡时被系统拒绝了，纯权限问题
        let denied = text.contains("access denied creating wintun adapter")
            || text.contains("administrator privileges");
        if !tun_failed && !denied {
            return None;
        }

        let mut hint = String::from(
            "没能创建虚拟网卡。内置的 openconnect 9.21 自带 wintun.dll，\
             正常情况不需要另装网卡驱动，所以先别急着找驱动：",
        );
        if denied {
            hint.push_str("日志里写的是权限不足，说明拉起隧道的进程没拿到管理员权限；");
        } else {
            hint.push_str(
                "最常见的原因是连接进程没有管理员权限，\
                 或者 wintun.dll 被杀软拦下/清理掉了（它就在程序目录的 apps/openconnect 里）；",
            );
        }
        hint.push_str(
            "请确认以管理员身份运行本程序，并把安装目录加进杀毒软件白名单后重试。\
             如果仍然失败，可以再装一份 TAP-Windows 驱动，openconnect 会自动回退到它。",
        );
        Some(hint)
    }

    // ======================= 心跳与守护 =======================

    /// 心跳守卫：主进程被强杀时没人来收尾，心跳停了就自己走正常停止流程
    fn check_parent_alive(&mut self) {
        if self.last_alive_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_alive_check = Instant::now();
        match self.session.alive_age() {
            // 心跳文件还没出现：给启动留 10s 宽限期
            None => {
                if elapsed_since(&self.status.started_at) > Duration::from_secs(10) {
                    self.parent_lost = true;
                }
            }
            Some(age) => {
                if age > Duration::from_secs(8) {
                    self.parent_lost = true;
                }
            }
        }
        if self.parent_lost {
            self.log("[worker] 主进程心跳已停止，自动收尾并退出");
        }
    }

    /// 轮询 mode 文件：变化时只改判定开关，已建立的连接不受影响
    fn poll_mode_change(&mut self) {
        if self.last_mode_check.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_mode_check = Instant::now();
        let Some(raw) = self.session.read_mode() else {
            return;
        };
        let Ok(target) = ProxyMode::parse(&raw) else {
            return;
        };
        if target == self.applied_mode {
            return;
        }
        self.log(&format!(
            "[模式] 运行期切换：{} -> {}",
            self.applied_mode.as_str(),
            target.as_str()
        ));
        self.switch_mode(target);
    }

    /// 轮询 rules.json：内容变了才重新编译；规则模式下只重置受影响的连接
    fn poll_rules_change(&mut self) {
        if self.last_rules_check.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_rules_check = Instant::now();
        let Some(raw) = self.session.read_rules_raw() else {
            return;
        };
        if self.last_rules_raw.as_deref() == Some(raw.as_str()) {
            return;
        }
        let Some((rules, default_policy)) = self.session.read_rules() else {
            return;
        };
        let enabled: Vec<VpnRule> = rules.into_iter().filter(|r| r.enabled).collect();
        self.last_rules_raw = Some(raw);
        self.current_rules = enabled;
        self.current_default_policy = default_policy;
        if self.split.is_some() {
            self.log(&format!(
                "[规则] 已更新 {} 条规则，正在重新下发路由注入分流",
                self.current_rules.len()
            ));
            self.restart_split();
        } else {
            self.log(&format!(
                "[规则] 已更新 {} 条规则，当前是{}模式，切回规则模式后生效",
                self.current_rules.len(),
                self.applied_mode.as_str()
            ));
        }
    }
    /// 路由守护：VPN 客户端或系统可能在后台把我们的路由改掉，这里定期补回来
    fn guard_routes(&mut self) {
        // 全局模式：守护半默认路由 + 隧道 DNS
        if let Some(full) = self.full.as_ref() {
            full.guard();
            return;
        }
        // 规则模式：守护隧道路由 + 按域名解析配置
        if let Some(split) = self.split.as_ref() {
            split.guard();
        }
    }

    // ======================= 路由注入式分流（规则模式） =======================

    /// 规则模式的落地实现：不捕获、不抢默认路由、不改系统 DNS，只做两件事——
    ///
    /// 1. 把命中「走 VPN」的域名交给 VPN DNS 解析：macOS 写 `/etc/resolver/<域名>`
    ///    + 本机转发器，Windows 写 NRPT 规则（见 `split.rs` / `win_dns.rs`）；
    /// 2. 把这些域名解析出来的内网 IP 装 `/32` 隧道路由（macOS 在转发时注入，
    ///    Windows 由后台解析线程注入）。
    ///
    /// 默认路由和系统 DNS 一律不动，所以结构上不可能再出现「一连上就全网断」。
    /// 任何一步失败都只是「本次分流不生效」，隧道照旧，网络不受影响。
    fn start_split_rules(&mut self) -> Result<(), String> {
        let mut plan = crate::vpn::split::plan(&self.current_rules, &self.current_default_policy);
        for note in &plan.notes {
            self.log(&format!("[分流] {}", note));
        }
        // 服务端下发的「公司内网」范围：默认域名 + split-include 网段。
        // 这是 VPN 管理员配的权威范围，比用户手写规则更全 —— 默认域名一条就能
        // 覆盖整族子域，网段则把「落在内网的 IP」全部兜住（应用自己解析域名也跑不掉）。
        if let Some(domain) = self.server_def_domain.clone() {
            if plan.add_domain(&domain) {
                self.log(&format!(
                    "[分流] 服务端下发默认域名 {}：该域及其所有子域自动走 VPN",
                    domain
                ));
            }
        }
        let mut pushed = 0usize;
        let mut all_tunnel = false;
        for cidr in self.server_split_routes.clone() {
            match crate::vpn::route::parse_cidr(&cidr) {
                Ok((_, 0)) => all_tunnel = true,
                Ok(_) => {
                    if plan.add_cidr(&cidr) {
                        pushed += 1;
                    }
                }
                Err(e) => self.log(&format!(
                    "[分流] 服务端下发的网段 {} 无法解析，跳过：{}",
                    cidr, e
                )),
            }
        }
        if all_tunnel {
            self.log(
                "[分流] 服务端下发的是全隧道（0.0.0.0/0）：路由注入模式不接管全隧道，\
                 要「全部走 VPN」请用全局模式",
            );
        }
        if pushed > 0 {
            self.log(&format!(
                "[分流] 服务端下发 {} 条分流网段，已并入隧道路由（落这些网段的流量都走 VPN）",
                pushed
            ));
        }
        let tun_dev = self.status.tun_dev.clone();
        if tun_dev.is_empty() {
            return Err("隧道接口名未知，无法注入隧道路由".to_string());
        }
        #[cfg(target_os = "windows")]
        if self.status.tun_index == 0 {
            return Err("隧道接口索引未知，无法注入隧道路由".to_string());
        }
        // 断开时要回收脚本留下的「VPN 服务器防环主机路由」：断开走 SIGKILL，
        // 脚本的 disconnect 分支根本不会执行。分流模式不装捕获路由，但这一步照做。
        if self.vpn_server_ips.is_empty() {
            self.vpn_server_ips = resolve_server_ips(&self.request.server);
        }
        let vpn_dns: Vec<std::net::Ipv4Addr> = self
            .status
            .dns
            .iter()
            .filter_map(|item| item.trim().parse().ok())
            .collect();
        self.log(&format!(
            "[分流] 路由注入模式：域名 {:?}，网段 {:?}，VPN DNS {:?}，隧道 {}",
            plan.domains, plan.cidrs, vpn_dns, tun_dev
        ));
        match crate::vpn::split::SplitTunnel::apply(
            &self.session,
            &tun_dev,
            self.status.tun_index,
            &vpn_dns,
            plan,
        ) {
            Ok(split) => {
                self.log(&format!(
                    "[分流] 路由注入分流已生效：{} 个域名走隧道 {}，{} 条固定隧道路由；\
                     默认路由与系统 DNS 均未改动（系统解析器不受影响）",
                    split.domain_count(),
                    tun_dev,
                    split.static_route_count()
                ));
                split.spawn_self_check(self.session.clone());
                self.status.routes = split.routes_for_status();
                self.split = Some(split);
                Ok(())
            }
            Err(e) => {
                self.degraded_reason = Some(format!("路由注入分流未能启动：{}", e));
                self.log(&format!(
                    "[分流] 路由注入分流未能启动：{}（本次只建隧道、不接管任何流量，网络不受影响）",
                    e
                ));
                Ok(())
            }
        }
    }

    /// 全局模式：整机流量走隧道（详见 fulltunnel.rs 注释）。
    ///
    /// 半默认路由指向隧道 + 系统 DNS 改成 VPN DNS，装完立刻验证解析器还能解析，
    /// 不通就整段回滚。
    fn start_full_tunnel(&mut self) -> Result<(), String> {
        let tun_dev = self.status.tun_dev.clone();
        if tun_dev.is_empty() {
            return Err("隧道接口名未知，无法接管默认路由".to_string());
        }
        #[cfg(target_os = "windows")]
        if self.status.tun_index == 0 {
            return Err("隧道接口索引未知，无法接管默认路由".to_string());
        }
        // SIGKILL 时脚本的 disconnect 分支不执行，服务器防环路由要自己收
        if self.vpn_server_ips.is_empty() {
            self.vpn_server_ips = resolve_server_ips(&self.request.server);
        }
        let vpn_dns: Vec<std::net::Ipv4Addr> = self
            .status
            .dns
            .iter()
            .filter_map(|item| item.trim().parse().ok())
            .collect();
        self.log(&format!(
            "[全局] 接管整机流量：0.0.0.0/1 + 128.0.0.0/1 → {}，DNS → {:?}，本地网段保持直连",
            tun_dev, vpn_dns
        ));
        match crate::vpn::fulltunnel::FullTunnel::apply(
            &self.session,
            &tun_dev,
            self.status.tun_index,
            &self.status.vpn_ip,
            &vpn_dns,
        ) {
            Ok(full) => {
                self.log(&format!(
                    "[全局] 全局模式已生效：{} 条半默认路由指向 {}，DNS 已指到 {:?}（断开时还原）",
                    full.route_count(),
                    full.tun_dev(),
                    full.dns_servers()
                ));
                self.status.routes = full.routes_for_status();
                self.full = Some(full);
                Ok(())
            }
            Err(e) => {
                self.degraded_reason = Some(format!("全局模式未能接管整机流量：{}", e));
                self.log(&format!(
                    "[全局] 未能接管整机流量：{}（本次只建隧道，网络不受影响）",
                    e
                ));
                Ok(())
            }
        }
    }

    /// 运行期切换代理模式：撤掉旧布线，按目标模式重新布线。
    ///
    /// 规则/全局/直连本质都是「布几条路由 + 改不改 DNS」，所以切换就是替换，
    /// 不需要断开重连（界面上点了就生效）。
    fn switch_mode(&mut self, target: ProxyMode) {
        if let Some(mut split) = self.split.take() {
            split.teardown();
            self.log(&format!(
                "[模式] 已撤掉规则分流的布线（{} 个域名、动态注入的内网 IP 一并回收）",
                split.domain_count()
            ));
        }
        if let Some(mut full) = self.full.take() {
            full.teardown();
            self.log("[模式] 已撤掉全局模式的布线（半默认路由与 DNS 已还原）");
        }
        self.degraded_reason = None;
        self.status.routes.clear();
        self.applied_mode = target;
        self.status.applied_mode = target.as_str().to_string();
        let result = match target {
            ProxyMode::Direct => {
                self.log("[模式] 直连模式：隧道保留，不接管任何流量");
                Ok(())
            }
            ProxyMode::Rule => self.start_split_rules(),
            ProxyMode::Global => self.start_full_tunnel(),
        };
        if let Err(e) = result {
            self.log(&format!("[模式] 切换为 {} 失败：{}", target.as_str(), e));
        }
        let message = match (&self.split, &self.full, &self.degraded_reason) {
            (_, Some(_), _) => "隧道已建立，全局模式生效（全部流量走 VPN）".to_string(),
            (Some(split), None, _) => format!(
                "隧道已建立，路由注入分流已生效（{} 个域名 → {}）",
                split.domain_count(),
                split.tun_dev()
            ),
            (None, None, Some(reason)) => format!("已连接（未接管系统流量：{}）", reason),
            (None, None, None) if target == ProxyMode::Direct => {
                "隧道已建立（直连模式，未接管任何流量）".to_string()
            }
            _ => "隧道已建立".to_string(),
        };
        self.set_phase(VpnPhase::Connected, &message);
    }

    /// 规则改了之后重新下发分流（域名、网段都可能变）。
    fn restart_split(&mut self) {
        if let Some(mut old) = self.split.take() {
            old.teardown();
        }
        self.degraded_reason = None;
        self.status.routes.clear();
        if let Err(e) = self.start_split_rules() {
            self.log(&format!("[分流] 重新下发分流失败：{}", e));
        }
        let message = match (&self.split, &self.degraded_reason) {
            (Some(split), _) => format!(
                "隧道已建立，路由注入分流已生效（{} 个域名 → {}）",
                split.domain_count(),
                split.tun_dev()
            ),
            (None, Some(reason)) => format!("已连接（未接管系统流量：{}）", reason),
            (None, None) => "隧道已建立".to_string(),
        };
        self.set_phase(VpnPhase::Connected, &message);
    }

    // ======================= 按代理模式布线 =======================

    /// 隧道就绪后按本次生效的代理模式布线。
    ///
    /// 三种模式都是「本地布线」，不再开捕获网卡：
    /// - 直连：隧道留着，什么都不改；
    /// - 规则：按域名/网段的 DNS + 路由注入（`split.rs`），默认路由与系统 DNS 不动；
    /// - 全局：两条半默认路由 + 系统 DNS 指向 VPN（`fulltunnel.rs`），带验证与回滚。
    ///
    /// 非 macOS / Windows 平台不在这里判：`SplitTunnel::apply` / `FullTunnel::apply`
    /// 的第一步就会因为「没有该平台的路由实现」报错，走统一的降级分支（只建隧道）。
    /// 这样三个平台都能编译，也不会在没验证过的平台上动路由表。
    fn start_engine(&mut self) -> Result<(), String> {
        match self.applied_mode {
            ProxyMode::Direct => {
                self.log("[模式] 直连模式：隧道已建立，不接管任何流量");
                Ok(())
            }
            ProxyMode::Rule => self.start_split_rules(),
            ProxyMode::Global => self.start_full_tunnel(),
        }
    }

    /// 回收本程序布下的分流通路（隧道路由、解析器配置、半默认路由与 DNS）
    fn cleanup_routes(&mut self) {
        if let Some(mut full) = self.full.take() {
            full.teardown();
            self.log("[清理] 全局模式已回收（半默认路由与 DNS 已还原）");
        }
        if let Some(mut split) = self.split.take() {
            split.teardown();
            self.log("[清理] 路由注入分流已回收（解析器配置、隧道路由、后台解析线程）");
        }
        self.status.routes.clear();
        // 脚本给 VPN 服务器加的防环主机路由不在这份记录里，SIGKILL 停止时脚本的
        // disconnect 分支不会执行，它会一直残留并可能让下次连接选源失败，这里一并清掉。
        for ip in std::mem::take(&mut self.vpn_server_ips) {
            if let Err(e) = route::delete_host_route(&ip) {
                self.log(&format!("[清理] 服务器主机路由 {} 回收失败: {}", ip, e));
            }
        }
    }

    // ======================= 网络快照 =======================

    fn take_snapshot(&mut self) {
        let routes = route::list_default_routes().unwrap_or_default();
        let metrics = route::list_interface_metrics().unwrap_or_default();
        let interfaces = route::list_interfaces().unwrap_or_default();

        let mut snapshot = NetworkSnapshot::default();
        if let Some(physical) = route::pick_physical_default(&routes, &metrics) {
            snapshot.default_gateway = physical.next_hop.clone();
            snapshot.default_if_index = physical.index;
            snapshot.default_if_alias = physical.alias.clone();
            self.log(&format!(
                "[快照] 本地出口: {} via {} (if#{})",
                physical.alias, physical.next_hop, physical.index
            ));
        } else {
            self.log("[快照] 未找到可用的物理默认路由（VPN 网卡已被排除）");
        }

        // 物理网卡上的直连网段，公司网络为主模式下需要作为直连例外
        for info in interfaces {
            // 必须连驱动描述一起判断：中文系统上网卡别名可能只是「以太网 2」这种，
            // 只看名字会把隧道网卡当成物理网卡，快照里的「本地直连网段」就会变成公司内网。
            if route::interface_is_vpn(&info) {
                continue;
            }
            // Windows 拿得到接口索引，直接比索引即可；macOS 的索引恒为 0，
            // 只能退化成比接口名——否则 lo0 的 127.0.0.0/8 也会被当成
            // 「本地直连网段」写进直连例外，路由守护就会一直去动回环路由。
            let on_default_interface = if snapshot.default_if_index != 0 {
                info.index == snapshot.default_if_index
            } else if !snapshot.default_if_alias.is_empty() {
                info.alias == snapshot.default_if_alias
            } else {
                true
            };
            if !on_default_interface {
                continue;
            }
            if let Some(subnet) = route::subnet_of(&info.ip, info.prefix_len) {
                if !snapshot.local_subnets.contains(&subnet) {
                    self.log(&format!("[快照] 本地直连网段: {}", subnet));
                    snapshot.local_subnets.push(subnet);
                }
            }
        }

        self.snapshot = snapshot.clone();
        self.status.snapshot = Some(snapshot);
        let _ = self.session.write_status(&self.status);
    }

    // ======================= openconnect 主通道 =======================

    /// 清掉上一轮会话可能残留的「VPN 服务器防环主机路由」。
    ///
    /// 脚本会给服务器本身加一条主机路由避免环路，而 worker 停隧道走的是 SIGKILL，
    /// 脚本的 disconnect 分支不会执行，这条路由就会残留。残留路由绑定的源地址在
    /// 物理网卡地址变化后会失效，内核发起连接时选不到本地地址，直接以
    /// `Can't assign requested address` 失败——连最初的 TCP 握手都做不了。
    /// 所以必须在拉起 openconnect *之前* 先按解析出的服务器 IP 自愈一次。
    fn clear_stale_server_routes(&mut self) {
        let ips = resolve_server_ips(&self.request.server);
        self.vpn_server_ips = ips.clone();
        if ips.is_empty() {
            self.log("[清理] 未能解析 VPN 服务器地址，跳过旧主机路由清理");
            return;
        }
        for ip in &ips {
            match route::delete_host_route(ip) {
                Ok(()) => {}
                Err(e) => self.log(&format!("[清理] 旧服务器主机路由 {} 清理失败: {}", ip, e)),
            }
        }
        self.log(&format!(
            "[清理] 已刷新 VPN 服务器主机路由 {}",
            ips.join(", ")
        ));
    }

    fn connect_openconnect(&mut self) -> Result<(Child, Vec<String>), String> {
        self.clear_stale_server_routes();
        let (file_name, content) = script::generate(&script::ScriptContext {
            session_dir: &self.session.dir,
            original_gateway: &self.snapshot.default_gateway,
        });
        let script_path = script_path_for_openconnect(&self.session.script_dir(), file_name)?;
        write_script_file(&script_path, &content)?;
        self.log(&format!("[脚本] 已生成 {}", script_path.display()));

        let mut request = self.request.clone();
        request.script_path = script_path.to_string_lossy().to_string();
        // 返回真正下发的那份参数：self.request.script_path 还是空的，
        // 拿它拼日志会打出一句 `--script= vpn.example.com:1218` 误导排查
        let args = connector::openconnect_args(&request);
        let child = connector::spawn_openconnect(&request)?;
        Ok((child, args))
    }

    /// 跑完整个 openconnect 会话
    fn run_openconnect(&mut self) -> i32 {
        self.take_snapshot();

        let (mut child, spawn_args) = match self.connect_openconnect() {
            Ok(pair) => pair,
            Err(e) => return self.fail(&e),
        };
        self.log(&format!(
            "[隧道] openconnect 已启动，pid {}，程序 {}，参数 {}",
            child.id(),
            self.request.connector_path,
            spawn_args.join(" ")
        ));

        let rx = pump_child_output(&mut child);
        self.set_phase(VpnPhase::Connecting, "正在建立隧道并等待服务端下发地址");

        let mut routes_applied = false;
        let mut guard_at = Instant::now() + GUARD_INTERVAL;
        loop {
            self.drain_output(&rx);
            // 运行期开关：模式与规则都可能在任何时刻被主进程改写
            self.poll_mode_change();
            self.poll_rules_change();
            self.check_parent_alive();

            if self.parent_lost || self.session.is_stop_requested() {
                if !self.parent_lost {
                    self.log("[worker] 收到停止请求");
                }
                break;
            }

            // 脚本没跑起来 -> 网卡参数永远拿不到 -> 没必要白等到 120s 超时。
            // 这条分支是界面上「点了连接后一直转圈、过一分钟才失败」的根治办法。
            if !routes_applied {
                if let Some(raw) = self.script_failure() {
                    let _ = child.kill();
                    let _ = child.wait();
                    // 各平台的脚本引擎完全不同：Windows 走 cscript/JScript，类 Unix 走
                    // /bin/sh。排障提示必须跟着平台走，否则 macOS 上看到「Windows
                    // Script Host」只会把人带偏。Windows 文案保持原样不动。
                    #[cfg(target_os = "windows")]
                    let hint = "常见原因是 Windows Script Host（cscript）被杀软拦截或系统策略禁用，\
                                可先确认 `cscript //?` 能正常运行；";
                    #[cfg(not(target_os = "windows"))]
                    let hint = "常见原因是脚本路径被 openconnect 交给 /bin/sh 时按空格拆了词\
                                （`--script` 的值在它眼里是一条 shell 命令行），本程序已把脚本\
                                改放到不含空格的目录来规避；若仍失败，再排查脚本可执行位或 \
                                /bin/sh 是否被安全策略拦截；";
                    let reason = format!(
                        "路由脚本执行失败，隧道无法完成配置。openconnect 的原话：{}\n\
                         {}脚本本身由本程序生成，\
                         如需排查请把会话目录里的 worker.log 与 script.log 一并提供。",
                        raw, hint
                    );
                    return self.fail(&reason);
                }
            }

            if !routes_applied && self.tunnel_ready() {
                if let Err(e) = self.start_engine() {
                    // 接管失败必须把隧道一起收掉，否则留下一个没人管辖的
                    // openconnect 进程，网络可能处于黑洞状态。
                    let _ = child.kill();
                    let _ = child.wait();
                    return self.fail(&e);
                }
                match self.degraded_reason.clone() {
                    // 降级：隧道在建，但没有接管系统流量（例如当前平台还没有分流实现）
                    Some(reason) => {
                        self.set_phase(
                            VpnPhase::Connected,
                            &format!("已连接（未接管系统流量：{}）", reason),
                        );
                        self.log(&format!(
                            "[安全] 本次连接只建隧道、不接管系统流量：{}",
                            reason
                        ));
                    }
                    None => {
                        let message = match (&self.split, &self.full, self.applied_mode) {
                            (_, Some(_), _) => {
                                "隧道已建立，全局模式生效（全部流量走 VPN）".to_string()
                            }
                            (Some(split), _, _) => format!(
                                "隧道已建立，路由注入分流已生效（{} 个域名 → {}）",
                                split.domain_count(),
                                split.tun_dev()
                            ),
                            (None, None, ProxyMode::Direct) => {
                                "隧道已建立（直连模式，未接管任何流量）".to_string()
                            }
                            (None, None, _) => "隧道已建立（未接管系统流量）".to_string(),
                        };
                        self.set_phase(VpnPhase::Connected, &message);
                    }
                }
                self.last_mode_check = Instant::now();
                self.last_rules_check = Instant::now();
                routes_applied = true;
                guard_at = Instant::now() + GUARD_INTERVAL;
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    self.drain_output(&rx);
                    let code = status.code().unwrap_or(-1);
                    if routes_applied {
                        self.log(&format!("[隧道] openconnect 退出，code {}", code));
                    }
                    return if routes_applied {
                        self.status.error = "隧道已断开".to_string();
                        self.set_phase(VpnPhase::Disconnected, "隧道已断开");
                        self.cleanup_routes();
                        let _ = self.session.write_status(&self.status);
                        EXIT_OK
                    } else {
                        let mut reason = format!(
                            "openconnect 退出（code {}），未能建立隧道：{}",
                            code,
                            self.tail_summary()
                        );
                        // 驱动没装这类问题，光看 openconnect 的原文没人知道该干嘛
                        if let Some(hint) = self.tun_setup_hint() {
                            reason.push('。');
                            reason.push_str(&hint);
                        }
                        return self.fail(&reason);
                    };
                }
                Ok(None) => {}
                Err(e) => return self.fail(&format!("等待 openconnect 失败: {}", e)),
            }

            if !routes_applied && elapsed_since(&self.status.started_at) > CONNECT_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                let reason = format!(
                    "连接超时（{}s）：{}",
                    CONNECT_TIMEOUT.as_secs(),
                    self.tail_summary()
                );
                return self.fail(&reason);
            }

            if routes_applied && Instant::now() >= guard_at {
                self.guard_routes();
                guard_at = Instant::now() + GUARD_INTERVAL;
            }

            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        }

        // 正常停止流程
        self.set_phase(VpnPhase::Disconnecting, "正在断开并还原路由");
        let _ = child.kill();
        let _ = child.wait();
        self.drain_output(&rx);
        self.cleanup_routes();
        self.session.remove_control_files();
        self.session.scrub_request();
        self.set_phase(VpnPhase::Disconnected, "已断开");
        EXIT_OK
    }

    fn drain_output(&mut self, rx: &Receiver<String>) {
        loop {
            match rx.try_recv() {
                Ok(line) => self.handle_line(&line),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        // 脚本的 stdout 接不到管道，参数只能从 script.log 里捞
        self.ingest_script_log();
    }
}

/// 把子进程的 stdout / stderr 变成一串行，通过 channel 交给主循环
fn pump_child_output(child: &mut Child) -> Receiver<String> {
    let (tx, rx) = mpsc::channel::<String>();
    let streams: Vec<Box<dyn Read + Send>> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();

    for stream in streams {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut buffer = Vec::new();
            loop {
                buffer.clear();
                match reader.read_until(b'\n', &mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {
                        // openconnect 的输出不保证是合法 UTF-8，用有损转换
                        let line = String::from_utf8_lossy(&buffer).trim_end().to_string();
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    drop(tx);
    rx
}

/// 隧道侧的下一跳地址。
///
/// 与官方脚本保持一致的思路：隧道场景下网关本身没有意义，只要保证地址在该网卡
/// 的子网内、能被系统判定为 on-link 即可。前缀长度 >= 31 时退化为隧道自身地址。
pub fn tunnel_next_hop(vpn_ip: &str, netmask: &str) -> String {
    if vpn_ip.is_empty() {
        return String::new();
    }
    let prefix = if netmask.contains('.') {
        route::mask_to_prefix(netmask).unwrap_or(24)
    } else {
        netmask.parse::<u8>().unwrap_or(24)
    };
    if prefix >= 31 {
        return vpn_ip.to_string();
    }
    route::first_usable_in_subnet(vpn_ip, netmask).unwrap_or_else(|| vpn_ip.to_string())
}

fn now() -> String {
    crate::util::time::now_local_rfc3339()
}

/// 从 RFC3339 时间戳算到现在过了多久
fn elapsed_since(timestamp: &str) -> Duration {
    match chrono::DateTime::parse_from_rfc3339(timestamp) {
        Ok(start) => {
            let start = start.with_timezone(&chrono::Local);
            let delta = chrono::Local::now().signed_duration_since(start);
            delta.to_std().unwrap_or(Duration::ZERO)
        }
        Err(_) => Duration::ZERO,
    }
}

/// worker 入口
pub fn run(session_dir: &Path) -> i32 {
    let session = VpnSession::new(session_dir.to_path_buf());
    if let Err(e) = session.ensure() {
        eprintln!("[worker] {}", e);
        return EXIT_NO_REQUEST;
    }

    session.append_log(&format!("[worker] 启动，pid {}", std::process::id()));

    let request = match session.read_request() {
        Ok(request) => request,
        Err(e) => {
            session.append_log(&format!("[worker] 读取连接请求失败: {}", e));
            return EXIT_NO_REQUEST;
        }
    };

    // 上一轮被 SIGKILL 时留下的解析器配置先清掉：macOS 是 `/etc/resolver` 文件
    // （会把内网域名指向一个没人监听的 127.0.0.1:53），Windows 是 NRPT 规则
    // （会把内网域名指向已经断掉的 VPN DNS）。清不干净就是「内网域名全解析不了」。
    crate::vpn::split::cleanup_stale(&session);
    // 全局模式崩溃现场：macOS 的 DNS 被改到 VPN DNS、标记文件还在；Windows 的
    // 半默认路由还挂在 VPN 网卡上。先还原再连，否则会把残留状态当成「用户自己配的」。
    crate::vpn::fulltunnel::restore_stale(&session);
    let mut worker = Worker::new(session, request);

    // 权限问题排查的第一现场：日志里必须先能回答「这个 worker 到底提权了没有、
    // 跑的是哪个 exe」。之前只有间接证据（Get-NetAdapter 报「拒绝访问」），
    // 用户看到失败根本分不清是没提权、提权到了旧版本、还是别的原因。
    worker.log(&format!(
        "[worker] 提权状态：{}，程序 {}，会话目录 {}",
        if crate::vpn::elevate::is_elevated() {
            "管理员"
        } else {
            "非管理员（建隧道、改路由、写 DNS 都会失败）"
        },
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "未知".to_string()),
        worker.session.dir.display()
    ));

    if worker.request.connector_path.is_empty() {
        return worker.fail("没有可用的连接器，请先在 VPN 页确认连接器状态");
    }

    // 只剩 openconnect 一条通道，连接器路径一定是 openconnect 可执行文件
    let code = worker.run_openconnect();

    // 兜底：任何路径退出前都确保布线被回收。
    // 规则模式没有「路由表账本」，但它有解析器配置、隧道路由和后台解析线程要收，
    // 所以这几个条件都要算上，否则异常退出会把 `/etc/resolver` / NRPT 规则留在地上。
    if worker.split.is_some() || worker.full.is_some() || !worker.vpn_server_ips.is_empty() {
        worker.cleanup_routes();
    }
    let _ = worker.session.write_status(&worker.status);
    code
}

/// 把 `host:port` 形式的服务器地址解析成 IPv4 列表。
///
/// 只做 IPv4：脚本加的防环主机路由、以及 `route -host` 的清理都只认 IPv4 地址；
/// 解析不出来（DNS 不通、地址非法）就返回空列表，让调用方跳过清理而不是报错。
fn resolve_server_ips(server: &str) -> Vec<String> {
    let host = match server.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => server,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Vec::new();
    }

    use std::net::{IpAddr, ToSocketAddrs};
    let mut out: Vec<String> = Vec::new();
    if let Ok(addrs) = (host, 0u16).to_socket_addrs() {
        for addr in addrs {
            if let IpAddr::V4(v4) = addr.ip() {
                let text = v4.to_string();
                if !out.contains(&text) {
                    out.push(text);
                }
            }
        }
    }
    out
}

/// 捕获 TUN 抢流量用的两条半默认路由。
pub fn capture_route_cidrs() -> Vec<String> {
    vec!["0.0.0.0/1".to_string(), "128.0.0.0/1".to_string()]
}

/// 过滤掉捕获 TUN 自己的地址，避免给自建网卡加服务器主机路由。
pub fn filter_server_ips(server_ips: &[String], capture_ip: &str) -> Vec<String> {
    server_ips.iter().filter(|ip| ip.as_str() != capture_ip).cloned().collect()
}

/// 给 openconnect 用的脚本路径：路径里含空格时改放到不含空格的目录。
///
/// openconnect 执行 `--script` 的方式是「把整条值当成一条 shell 命令行交给 /bin/sh」
/// （`openconnect --help` 原文即 "Shell command line for using a vpnc-compatible
/// config script"），因此路径里只要有一个空格就会被 /bin/sh 拆成两个词，现场表现：
///
/// ```text
/// /bin/sh: /Users/xxx/Library/Application: is a directory
/// Script '…/vpnc-script.sh' returned error 126
/// ```
///
/// macOS 的会话目录恰好落在 `~/Library/Application Support/…`（必含空格），所以路径
/// 含空格时把脚本改放到不含空格的临时目录。只搬「脚本文件本身」：脚本写日志用的是
/// 带引号的 `"$SESSION_DIR/script.log"`，会话目录里的 script.log 位置不变。
///
/// Windows 的 openconnect 用 cscript 跑 .js，参数走的是另一条路，保持原样不动。
fn script_path_for_openconnect(
    session_script_dir: &Path,
    file_name: &str,
) -> Result<PathBuf, String> {
    #[cfg(not(target_os = "windows"))]
    {
        if session_script_dir.to_string_lossy().contains(' ') {
            let dir = std::env::temp_dir()
                .join(crate::vpn::APP_DIR_NAME)
                .join(crate::vpn::session::SCRIPT_DIR);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("创建脚本目录失败 {}: {}", dir.display(), e))?;
            return Ok(dir.join(file_name));
        }
    }
    Ok(session_script_dir.join(file_name))
}

/// 写出 openconnect 的路由脚本，并确保它有可执行位。
///
/// openconnect 把 `--script` 的值当成一条 shell 命令行交给 `/bin/sh` 执行，脚本最终是被
/// `/bin/sh` 按路径 exec 的：少了可执行位就会得到 `Permission denied` 与
/// `returned error 126`。这与「路径含空格被 /bin/sh 拆词」是两类不同成因的 126，
/// 后者在 `script_path_for_openconnect` 里规避。Windows 下由 cscript 解释执行，
/// 不存在这个位，保持原样。
fn write_script_file(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content)
        .map_err(|e| format!("写入路由脚本失败 {}: {}", path.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(path)
            .map_err(|e| format!("读取路由脚本权限失败 {}: {}", path.display(), e))?
            .permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(path, perm)
            .map_err(|e| format!("设置路由脚本可执行位失败 {}: {}", path.display(), e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn generated_script_gets_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ws-worker-script-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("vpnc-script.sh");
        write_script_file(&path, "#!/bin/sh\necho hi\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_ne!(
            mode & 0o111,
            0,
            "路由脚本必须带可执行位，否则 openconnect 会报 error 126: {:o}",
            mode
        );
    }

    #[test]
    fn next_hop_uses_first_usable_address() {
        // 复刻现场：隧道地址 198.51.100.39/22 -> 198.51.100.1
        assert_eq!(
            tunnel_next_hop("198.51.100.39", "255.255.252.0"),
            "198.51.100.1"
        );
    }

    #[test]
    fn next_hop_falls_back_to_own_address_for_tiny_prefix() {
        // /32 或 /31 没有「网络地址 + 1」可用，退回隧道自身地址
        assert_eq!(tunnel_next_hop("10.10.0.2", "255.255.255.255"), "10.10.0.2");
        assert_eq!(tunnel_next_hop("10.10.0.2", "32"), "10.10.0.2");
        assert_eq!(tunnel_next_hop("10.10.0.2", "31"), "10.10.0.2");
    }

    #[test]
    fn next_hop_accepts_prefix_length_notation() {
        assert_eq!(tunnel_next_hop("198.51.100.39", "22"), "198.51.100.1");
    }

    #[test]
    fn next_hop_empty_ip_is_empty() {
        assert_eq!(tunnel_next_hop("", "24"), "");
    }

    #[test]
    fn tail_summary_picks_error_lines() {
        let session = VpnSession::new(std::env::temp_dir().join("ws-worker-test"));
        let request = VpnRequest {
            conn_identity: String::new(),
            proxy_mode: "rule".to_string(),
            profile_id: "p".into(),
            profile_name: "n".into(),
            server: "s".into(),
            username: "u".into(),
            password: "p".into(),
            protocol: "anyconnect".into(),
            connector: crate::vpn::models::ConnectorKind::Auto,
            connector_path: "x".into(),
            rules: vec![],
            default_policy: "DIRECT".into(),
            extra_args: vec![],
            script_path: "s".into(),
            hook_exe: "h".into(),
            created_at: String::new(),
        };
        let mut worker = Worker::new(session, request);
        worker.tail.push("CSTP connected".into());
        worker.tail.push("Login failed.".into());
        assert!(worker.tail_summary().contains("Login failed"));
    }

    #[test]
    fn split_plan_only_nudges_supported_kinds() {
        // 规则能不能生效，现在完全取决于 split::plan 能翻译出什么动作
        let rules = vec![
            crate::vpn::models::VpnRule {
                id: "r1".into(),
                kind: "DOMAIN-SUFFIX".into(),
                value: "example.com".into(),
                policy: "VPN".into(),
                remark: String::new(),
                enabled: true,
            },
            crate::vpn::models::VpnRule {
                id: "r2".into(),
                kind: "DOMAIN-KEYWORD".into(),
                value: "corp".into(),
                policy: "VPN".into(),
                remark: String::new(),
                enabled: true,
            },
        ];
        let plan = crate::vpn::split::plan(&rules, "DIRECT");
        assert_eq!(plan.domains, vec!["example.com".to_string()]);
        assert!(
            plan.notes.iter().any(|n| n.contains("DOMAIN-KEYWORD")),
            "不支持的规则类型必须给出可执行的建议：{:?}",
            plan.notes
        );
    }

    #[test]
    fn tunnel_ready_requires_ip_and_interface() {
        let session = VpnSession::new(std::env::temp_dir().join("ws-worker-test2"));
        let request = VpnRequest {
            conn_identity: String::new(),
            proxy_mode: "rule".to_string(),
            profile_id: "p".into(),
            profile_name: "n".into(),
            server: "s".into(),
            username: "u".into(),
            password: "p".into(),
            protocol: "anyconnect".into(),
            connector: crate::vpn::models::ConnectorKind::Auto,
            connector_path: "x".into(),
            rules: vec![],
            default_policy: "DIRECT".into(),
            extra_args: vec![],
            script_path: "s".into(),
            hook_exe: "h".into(),
            created_at: String::new(),
        };
        let mut worker = Worker::new(session, request);
        assert!(!worker.tunnel_ready());
        worker.status.vpn_ip = "198.51.100.39".into();
        assert!(!worker.tunnel_ready());
        worker.status.tun_index = 21;
        assert!(worker.tunnel_ready());
    }

    #[test]
    fn tun_setup_failure_gets_actionable_hint() {
        let session = VpnSession::new(std::env::temp_dir().join("ws-worker-test3"));
        let request = VpnRequest {
            conn_identity: String::new(),
            proxy_mode: "rule".to_string(),
            profile_id: "p".into(),
            profile_name: "n".into(),
            server: "s".into(),
            username: "u".into(),
            password: "p".into(),
            protocol: "anyconnect".into(),
            connector: crate::vpn::models::ConnectorKind::Auto,
            connector_path: "x".into(),
            rules: vec![],
            default_policy: "DIRECT".into(),
            extra_args: vec![],
            script_path: "s".into(),
            hook_exe: "h".into(),
            created_at: String::new(),
        };
        let mut worker = Worker::new(session, request);
        // 正常日志不该给出驱动提示，免得误导
        assert!(worker.tun_setup_hint().is_none());

        // openconnect 建网卡失败时的原文，用户现场就长这样
        worker.tail.push("Set up tun device failed".into());
        let hint = worker.tun_setup_hint().expect("应该给出建网卡失败的提示");
        // 内置版本自带 wintun，不能一上来就让用户去装驱动
        assert!(hint.contains("wintun.dll"), "实际: {}", hint);
        assert!(hint.contains("管理员"), "实际: {}", hint);

        // 权限不足是 Wintun 的专属报错，要能单独认出来
        worker.tail.clear();
        worker.tail.push(
            "Access denied creating Wintun adapter. Are you running with Administrator privileges?"
                .into(),
        );
        let hint = worker.tun_setup_hint().expect("权限不足也该给出提示");
        assert!(hint.contains("管理员"), "实际: {}", hint);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn script_path_moves_out_of_spaced_dir() {
        // 复刻现场：会话目录落在 ~/Library/Application Support/…，必含空格。
        // openconnect 把 --script 交给 /bin/sh，空格会被拆成两个词：
        //   /bin/sh: /Users/xxx/Library/Application: is a directory
        //   Script '…/vpnc-script.sh' returned error 126
        let spaced = PathBuf::from("/Users/someone/Library/Application Support/com.ws.workbench/vpn/script");
        let path = script_path_for_openconnect(&spaced, "vpnc-script.sh").unwrap();
        assert!(
            !path.to_string_lossy().contains(' '),
            "含空格的脚本路径必须被搬走，实际: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "vpnc-script.sh",
            "文件名要保持不变"
        );
    }

    #[test]
    fn script_path_keeps_unspaced_dir() {
        let dir = PathBuf::from("/tmp/ws-no-space/script");
        let path = script_path_for_openconnect(&dir, "vpnc-script.sh").unwrap();
        assert_eq!(
            path,
            dir.join("vpnc-script.sh"),
            "不含空格的路径原样返回，不做无谓搬迁"
        );
    }

    #[test]
    fn capture_routes_are_the_two_halves() {
        assert_eq!(
            capture_route_cidrs(),
            vec!["0.0.0.0/1".to_string(), "128.0.0.0/1".to_string()]
        );
    }

    #[test]
    fn vpn_server_ips_excludes_capture_tun_address() {
        let ips = vec!["1.1.1.1".to_string(), "10.10.0.9".to_string()];
        assert_eq!(filter_server_ips(&ips, "10.10.0.9"), vec!["1.1.1.1".to_string()]);
    }
}
