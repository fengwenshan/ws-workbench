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
use crate::vpn::learn;
use crate::vpn::models::{
    AppliedRoute, LearnReport, LearnSummary, NetworkSnapshot, VpnMode, VpnPhase, VpnRequest,
    VpnStatus,
};
use crate::vpn::route::{self, RouteOutcome, RouteTarget};
use crate::vpn::script;
use crate::vpn::session::VpnSession;

/// 路由守护的巡检间隔
const GUARD_INTERVAL: Duration = Duration::from_secs(15);
/// 等待隧道就绪的最长时间
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

/// worker 退出码
pub const EXIT_OK: i32 = 0;
pub const EXIT_NO_REQUEST: i32 = 2;
pub const EXIT_FAILED: i32 = 3;

/// 我们主动加过的路由，断开时要逐条回收
struct AppliedRouteRecord {
    cidr: String,
    target: RouteTarget,
    outcome: RouteOutcome,
}

struct Worker {
    session: VpnSession,
    request: VpnRequest,
    status: VpnStatus,
    snapshot: NetworkSnapshot,
    applied: Vec<AppliedRouteRecord>,
    /// 「以公司网络为主」时默认路由已被脚本交给隧道，断开时必须交还
    default_route_taken: bool,
    /// 脚本是否已把 VPN DNS 写到本机（macOS 走 scutil/networksetup），断开时同样要还原
    dns_applied: bool,
    /// 脚本下发 DNS 时用的物理网络服务名，断开的兜底还原要用
    dns_service: String,
    /// 连接时解析出的 VPN 服务器 IP，用来清理脚本残留的防环主机路由
    vpn_server_ips: Vec<String>,
    /// 最近几行输出，失败时作为诊断信息
    tail: Vec<String>,
    /// script.log 已经读过的行数（脚本的 stdout 接不到管道，只能靠这个文件回传）
    script_log_lines: usize,
    /// 学习模式累积下来的观察报告，落盘在会话目录的 learn.json
    learn_report: LearnReport,
    /// 采样连续失败次数，超过阈值自动停用学习模式
    learn_failures: u32,
}

impl Worker {
    fn new(session: VpnSession, request: VpnRequest) -> Self {
        let mut status = VpnStatus::default();
        status.profile_id = request.profile_id.clone();
        status.profile_name = request.profile_name.clone();
        status.mode = request.mode;
        status.pid = std::process::id();
        status.started_at = now();
        status.updated_at = now();
        // 开了学习模式就先在状态里挂一个空摘要，界面上能立刻显示「学习中」
        if request.learn {
            status.learn = Some(LearnSummary {
                enabled: true,
                ..Default::default()
            });
        }
        let learn_report = LearnReport {
            enabled: request.learn,
            ..Default::default()
        };
        Worker {
            session,
            request,
            status,
            snapshot: NetworkSnapshot::default(),
            applied: Vec::new(),
            default_route_taken: false,
            dns_applied: false,
            dns_service: String::new(),
            vpn_server_ips: Vec::new(),
            tail: Vec::new(),
            script_log_lines: 0,
            learn_report,
            learn_failures: 0,
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
                // 脚本接管/交还默认路由的回报；断开时据此决定要不要还原
                "default_route" => self.default_route_taken = value == "vpn",
                // 脚本把 VPN DNS 下发到本机了；断开时据此还原 DNS（脚本 disconnect 分支
                // 在 SIGKILL 下不会执行，只能靠这里兜底）
                "dns_applied" => self.dns_applied = true,
                "dns_service" => self.dns_service = value,
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

    // ======================= 分流路由 =======================

    fn vpn_target(&self) -> RouteTarget {
        #[cfg(target_os = "windows")]
        {
            RouteTarget::new(
                self.status.tun_index,
                &self.status.tun_dev,
                &self.status.vpn_gateway,
            )
        }
        #[cfg(not(target_os = "windows"))]
        {
            RouteTarget::new(0, &self.status.tun_dev, "")
        }
    }

    fn local_target(&self) -> RouteTarget {
        #[cfg(target_os = "windows")]
        {
            RouteTarget::new(
                self.snapshot.default_if_index,
                &self.snapshot.default_if_alias,
                &self.snapshot.default_gateway,
            )
        }
        #[cfg(not(target_os = "windows"))]
        {
            RouteTarget::new(0, &self.snapshot.default_if_alias, &self.snapshot.default_gateway)
        }
    }

    fn record_applied(&mut self, cidr: &str, target: &RouteTarget, via: &str, outcome: RouteOutcome) {
        self.status.routes.push(AppliedRoute {
            cidr: cidr.to_string(),
            via: via.to_string(),
            next_hop: target.next_hop.clone(),
            iface: target.display(),
            owned: outcome == RouteOutcome::Created,
        });
        self.applied.push(AppliedRouteRecord {
            cidr: cidr.to_string(),
            target: target.clone(),
            outcome,
        });
    }

    /// 按分流模式下发路由
    fn apply_routes(&mut self) {
        self.status.routes.clear();
        let mode = self.request.mode;

        match mode {
            VpnMode::CustomerFirst => {
                // 只有规则表里的公司网段走隧道，其余（客户内网、互联网）保持本地默认路由
                let target = self.vpn_target();
                let rules = self.request.include_rules.clone();
                if rules.is_empty() {
                    self.log("[路由] 客户网络为主模式下规则表为空，不会有任何流量走 VPN");
                }
                for cidr in rules {
                    match route::add_route(&cidr, &target, 1) {
                        Ok(()) => {
                            self.log(&format!("[路由] {} -> VPN ({})", cidr, target.display()));
                            self.record_applied(&cidr, &target, "vpn", RouteOutcome::Created);
                        }
                        Err(e) => self.log(&format!("[路由] {} 下发失败: {}", cidr, e)),
                    }
                }
            }
            VpnMode::CompanyFirst => {
                // 默认路由已经由脚本交给隧道（company_first 会接管 default），
                // 这里只需要把客户侧网段抢回本地
                let target = self.local_target();
                let mut list = self.request.exclude_rules.clone();
                for subnet in self.snapshot.local_subnets.clone() {
                    if !list.contains(&subnet) {
                        list.push(subnet);
                    }
                }
                for cidr in list {
                    match route::ensure_route(&cidr, &target, 1) {
                        Ok(outcome) => {
                            let action = if outcome == RouteOutcome::Created {
                                "新增"
                            } else {
                                "调跃点"
                            };
                            self.log(&format!(
                                "[路由] {}({}) {} -> 本地 ({})",
                                cidr,
                                action,
                                target.next_hop,
                                target.display()
                            ));
                            self.record_applied(&cidr, &target, "local", outcome);
                        }
                        Err(e) => self.log(&format!("[路由] {} 直连例外失败: {}", cidr, e)),
                    }
                }
            }
        }
        let _ = self.session.write_status(&self.status);
    }

    /// 路由守护：VPN 客户端或系统可能在后台把我们的路由改掉，这里定期补回来
    fn guard_routes(&mut self) {
        let mode = self.request.mode;
        let vpn_target = self.vpn_target();
        let local_target = self.local_target();

        let checks: Vec<(String, RouteTarget)> = self
            .applied
            .iter()
            .filter(|record| record.outcome == RouteOutcome::Created)
            .map(|record| {
                let target = match mode {
                    VpnMode::CustomerFirst => vpn_target.clone(),
                    VpnMode::CompanyFirst => local_target.clone(),
                };
                (record.cidr.clone(), target)
            })
            .collect();

        let mut repaired = 0usize;
        for (cidr, target) in checks {
            match route::route_exists(&cidr, &target) {
                Ok(true) => {}
                Ok(false) => {
                    if route::add_route(&cidr, &target, 1).is_ok() {
                        repaired += 1;
                        self.log(&format!("[路由守护] {} 被改动，已重新下发", cidr));
                    }
                }
                Err(_) => {}
            }
        }
        if repaired > 0 {
            let _ = self.session.write_status(&self.status);
        }
    }

    // ======================= 学习模式 =======================

    /// 隧道本身和本程序发起的连接都不是分流目标，采样时直接排除
    fn self_process_names() -> Vec<String> {
        ["openconnect.exe", "openconnect", "ws-tools.exe", "ws-tools"]
            .iter()
            .map(|name| name.to_string())
            .collect()
    }

    /// 当前生效的规则表。两种模式的规则列表互斥，取并集即可
    fn known_rules(&self) -> Vec<String> {
        let mut rules = self.request.include_rules.clone();
        for cidr in &self.request.exclude_rules {
            if !rules.contains(cidr) {
                rules.push(cidr.clone());
            }
        }
        rules
    }

    /// 主进程请求清空采集结果时，重置内存里的报告并删掉文件。
    ///
    /// 主进程不能直接删 `learn.json`：报告本体在 worker 内存里，
    /// 下一轮采样会把它整份写回去，用户看到的就是「清空了又自己回来」。
    fn poll_learn_clear(&mut self) {
        if !self.session.is_learn_clear_requested() {
            return;
        }
        self.learn_report = LearnReport {
            enabled: self.request.learn,
            ..Default::default()
        };
        self.learn_failures = 0;
        self.session.reset_learn();
        self.publish_learn(None);
        self.log("[学习] 已按要求清空采集结果");
    }

    /// 采样一轮已建立的连接，并入报告，再把摘要写进状态文件
    fn learn_tick(&mut self) {
        if !self.request.learn {
            return;
        }
        let vpn_ips = if self.status.vpn_ip.is_empty() {
            Vec::new()
        } else {
            vec![self.status.vpn_ip.clone()]
        };
        let exclude = self.snapshot.local_subnets.clone();

        match learn::sample(&vpn_ips, &exclude, &Self::self_process_names()) {
            Ok(observations) => {
                self.learn_failures = 0;
                let rules = self.known_rules();
                learn::merge(&mut self.learn_report, &observations, &rules, &now());
                if let Err(e) = self.session.write_learn(&self.learn_report) {
                    self.log(&format!("[学习] 写学习报告失败: {}", e));
                }
                self.publish_learn(None);
            }
            Err(e) => {
                self.learn_failures = self.learn_failures.saturating_add(1);
                self.log(&format!(
                    "[学习] 采样失败（第 {} 次）: {}",
                    self.learn_failures, e
                ));
                // 连着几次都不行说明这台机器上取不到数，继续每 15 秒刷一条
                // 失败日志毫无意义，直接停用，把原因留在状态里给用户看。
                if self.learn_failures >= learn::MAX_CONSECUTIVE_FAILURES {
                    self.request.learn = false;
                    self.learn_report.enabled = false;
                    // 已经采到的结果不删，用户可能还想看一眼
                    let _ = self.session.write_learn(&self.learn_report);
                    self.log("[学习] 连续采样失败，已自动停用学习模式");
                }
                self.publish_learn(Some(e));
            }
        }
    }

    /// 把摘要写进 `status.json`。
    ///
    /// 明细（每台主机、每个候选网段）只走 `learn.json`：`status.json` 每 600ms
    /// 被主进程 diff 一次，塞几十 KB 进去会让状态轮询变成负担。
    fn publish_learn(&mut self, error: Option<String>) {
        if !self.request.learn && self.learn_report.scans == 0 {
            self.status.learn = None;
        } else {
            let mut summary = learn::summarize(&self.learn_report);
            summary.enabled = self.request.learn;
            if let Some(e) = error {
                summary.error = e;
            }
            self.status.learn = Some(summary);
        }
        self.status.updated_at = now();
        if let Err(e) = self.session.write_status(&self.status) {
            self.log(&format!("[worker] 写状态失败: {}", e));
        }
    }

    /// 回收本程序添加的路由，并把默认路由交还给连接前的物理网关
    fn cleanup_routes(&mut self) {
        let records: Vec<AppliedRouteRecord> = self.applied.drain(..).collect();
        for record in records {
            let result = match record.outcome {
                RouteOutcome::Created => route::delete_route(&record.cidr, &record.target),
                RouteOutcome::Updated => route::reset_route_metric(&record.cidr, &record.target, 256),
            };
            if let Err(e) = result {
                self.log(&format!("[清理] {} 回收失败: {}", record.cidr, e));
            }
        }
        // 停止隧道走的是 SIGKILL，脚本的 disconnect 分支不会执行，
        // 默认路由只能在这里交还，否则断开后整机失去默认路由
        if self.default_route_taken {
            self.default_route_taken = false;
            let gateway = self.snapshot.default_gateway.clone();
            match route::restore_default_route(&gateway) {
                Ok(()) => self.log("[清理] 默认路由已交还给物理网关"),
                Err(e) => self.log(&format!("[清理] 默认路由还原失败: {}", e)),
            }
        }
        // 脚本给 VPN 服务器加的防环主机路由不在这份记录里，SIGKILL 停止时脚本的
        // disconnect 分支不会执行，它会一直残留并可能让下次连接选源失败，这里一并清掉。
        for ip in std::mem::take(&mut self.vpn_server_ips) {
            if let Err(e) = route::delete_host_route(&ip) {
                self.log(&format!("[清理] 服务器主机路由 {} 回收失败: {}", ip, e));
            }
        }
        // DNS 同理：脚本的 disconnect 分支在 SIGKILL 下不会执行，
        // 这里必须兜底还原，否则断开后 /etc/resolv.conf 还指着内网 DNS，
        // 上不了网也解析不出公网域名。默认路由已交还，正好能问到物理网络服务名。
        if self.dns_applied {
            self.dns_applied = false;
            let dir = self.session.dir.clone();
            let service = self.dns_service.clone();
            let tun = self.status.tun_dev.clone();
            script::restore_vpn_dns(&dir, &service, &tun);
            self.log("[清理] VPN DNS 已还原");
        }
        self.status.routes.clear();
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
            "[清理] 已刷新 VPN 服务器主机路由 {}，避免残留路由导致选源失败",
            ips.join(", ")
        ));
    }

    fn connect_openconnect(&mut self) -> Result<(Child, Vec<String>), String> {
        self.clear_stale_server_routes();
        let (file_name, content) = script::generate(&script::ScriptContext {
            session_dir: &self.session.dir,
            mode: self.request.mode,
            use_vpn_dns: self.request.use_vpn_dns,
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
            "[隧道] openconnect 已启动，pid {}，参数 {}",
            child.id(),
            spawn_args.join(" ")
        ));

        let rx = pump_child_output(&mut child);
        self.set_phase(VpnPhase::Connecting, "正在建立隧道并等待服务端下发地址");

        let mut routes_applied = false;
        let mut guard_at = Instant::now() + GUARD_INTERVAL;

        loop {
            self.drain_output(&rx);
            // 清空请求可能在任何时候到达（连接中/已连接/仅连着没建隧道），
            // 所以放在循环顶部判一次，而不是塞进 15 秒那一跳
            self.poll_learn_clear();

            if self.session.is_stop_requested() {
                self.log("[worker] 收到停止请求");
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
                self.set_phase(
                    VpnPhase::ConfiguringRoutes,
                    &format!(
                        "隧道就绪 {}，正在按「{}」下发分流路由",
                        self.status.vpn_ip,
                        self.request.mode.label()
                    ),
                );
                self.apply_routes();
                routes_applied = true;
                self.set_phase(
                    VpnPhase::Connected,
                    &format!("已连接（{}）", self.request.mode.label()),
                );
                // 隧道刚起来就先采一轮，别让用户干等 15 秒
                if self.request.learn {
                    self.log("[学习] 学习模式已开启：每 15 秒采集一次实际连通的远端地址");
                    self.learn_tick();
                }
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
                self.learn_tick();
                guard_at = Instant::now() + GUARD_INTERVAL;
            }

            thread::sleep(Duration::from_millis(200));
        }

        // 正常停止流程
        self.set_phase(VpnPhase::Disconnecting, "正在断开并还原路由");
        let _ = child.kill();
        let _ = child.wait();
        self.drain_output(&rx);
        self.cleanup_routes();
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
    chrono::Local::now().to_rfc3339()
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

    let mut worker = Worker::new(session, request);

    if worker.request.connector_path.is_empty() {
        return worker.fail("没有可用的连接器，请先在 VPN 页确认连接器状态");
    }

    // 只剩 openconnect 一条通道，连接器路径一定是 openconnect 可执行文件
    let code = worker.run_openconnect();

    // 兜底：任何路径退出前都确保路由被回收
    if !worker.applied.is_empty() {
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
            profile_id: "p".into(),
            profile_name: "n".into(),
            server: "s".into(),
            username: "u".into(),
            password: "p".into(),
            protocol: "anyconnect".into(),
            connector: crate::vpn::models::ConnectorKind::Auto,
            connector_path: "x".into(),
            mode: VpnMode::CustomerFirst,
            include_rules: vec![],
            exclude_rules: vec![],
            use_vpn_dns: false,
            learn: false,
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
    fn tunnel_ready_requires_ip_and_interface() {
        let session = VpnSession::new(std::env::temp_dir().join("ws-worker-test2"));
        let request = VpnRequest {
            profile_id: "p".into(),
            profile_name: "n".into(),
            server: "s".into(),
            username: "u".into(),
            password: "p".into(),
            protocol: "anyconnect".into(),
            connector: crate::vpn::models::ConnectorKind::Auto,
            connector_path: "x".into(),
            mode: VpnMode::CustomerFirst,
            include_rules: vec![],
            exclude_rules: vec![],
            use_vpn_dns: false,
            learn: false,
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
            profile_id: "p".into(),
            profile_name: "n".into(),
            server: "s".into(),
            username: "u".into(),
            password: "p".into(),
            protocol: "anyconnect".into(),
            connector: crate::vpn::models::ConnectorKind::Auto,
            connector_path: "x".into(),
            mode: VpnMode::CustomerFirst,
            include_rules: vec![],
            exclude_rules: vec![],
            use_vpn_dns: false,
            learn: false,
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
        let spaced = PathBuf::from("/Users/someone/Library/Application Support/com.ws.tools/vpn/script");
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
}
