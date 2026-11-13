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

/// 换网后最多自动重连几次。换 Wi-Fi 时地址会连着变好几次（断开旧地址、
/// 拿到新地址、DHCP 续约），次数留够覆盖抖动，但不至于无限重连。
const MAX_NETWORK_RESTARTS: u32 = 3;

/// 一趟隧道运行的结局。
enum TunnelRun {
    /// 正常收尾：退出码原样交给进程
    Exited(i32),
    /// 换网导致这趟隧道不成立：布线已回收，外层按新出口重连
    Restart,
}

/// 判断某个 PID 对应的进程是否还在。
///
/// worker 用来看主进程是否还活着：任务管理器强杀是「进程直接消失」，
/// 这条判断比心跳文件更及时，也不受磁盘写失败影响。
#[cfg(target_os = "windows")]
fn process_running(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, E_INVALIDARG, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    // SAFETY: 句柄只在本函数内使用，用完立刻关闭。
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(handle) => {
                let state = WaitForSingleObject(handle, 0);
                let _ = CloseHandle(handle);
                // WAIT_OBJECT_0 = 已退出；WAIT_TIMEOUT = 还在跑；
                // WAIT_FAILED（句柄状态异常）按「不确定」处理成还活着。
                state != WAIT_OBJECT_0
            }
            // 只有「PID 不存在」（OpenProcess 报 ERROR_INVALID_PARAMETER，
            // 即 HRESULT_FROM_WIN32(87) = E_INVALIDARG）才判定已退出。
            // 权限不足 / 被杀软挡了句柄时进程其实还在，若当成已退出，worker
            // 会立刻自己拆隧道（界面明明活着）。这种情况交给心跳判据兜底。
            Err(err) => err.code() != E_INVALIDARG,
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn process_running(pid: u32) -> bool {
    // SAFETY: 信号 0 只做存在性/权限检查，不会给目标进程发任何信号。
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

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
    /// 最近几行输出，失败时作为诊断信息
    tail: Vec<String>,
    /// script.log 已经读过的行数（脚本的 stdout 接不到管道，只能靠这个文件回传）
    script_log_lines: usize,
    /// 读取失败只记一次，避免每 200ms 刷一行
    script_log_error_logged: bool,
    /// 脚本写完 `done=connect` 才代表接口地址/MTU 已配置完；否则路由可能
    /// 装在一个还没地址的接口上。
    script_done: bool,
    /// 当前生效的代理模式（可由主进程通过 mode 文件改）
    applied_mode: ProxyMode,
    /// 当前生效的规则与兜底走向（可由主进程通过 rules.json 改）
    current_rules: Vec<VpnRule>,
    current_default_policy: String,
    /// 本会话见过的 DNS 应答：规则热更新时用它补注入「现在该走隧道」的地址
    /// （应用可能缓存着旧答案，不会再发查询）
    memo: crate::vpn::split::DnsMemo,
    /// 上次读到的规则原文，内容没变就不重新编译
    last_rules_raw: Option<String>,
    last_mode_check: Instant,
    last_rules_check: Instant,
    last_alive_check: Instant,
    /// 连续几次看到心跳过期。见 `check_parent_alive`：一次过期不算数。
    stale_alive_checks: u32,
    /// 心跳过期：主进程已经不在了，循环里看到就把隧道收掉
    parent_lost: bool,
    /// 网络出口变更通知（仅 Windows；没有时保持原来的 15s 路由守护）
    netwatch: Option<crate::vpn::netwatch::Watcher>,
    /// 上次处理过的变更代号
    net_generation: u64,
    /// 换网后需要重连：主循环看到就撤布线、重新拉起 openconnect
    restart_requested: bool,
}

impl Worker {
    fn new(session: VpnSession, request: VpnRequest) -> Self {
        let applied_mode = ProxyMode::parse(&request.proxy_mode).unwrap_or(ProxyMode::Rule);
        let stamp = now();
        let current_rules: Vec<VpnRule> =
            request.rules.iter().filter(|r| r.enabled).cloned().collect();
        let current_default_policy = request.default_policy.clone();
        let status = VpnStatus {
            profile_id: request.profile_id.clone(),
            profile_name: request.profile_name.clone(),
            conn_identity: request.conn_identity.clone(),
            // 让界面知道这条会话实际按哪个模式在跑（界面上的选择只代表「下次连接」）
            applied_mode: applied_mode.as_str().to_string(),
            pid: std::process::id(),
            started_at: stamp.clone(),
            updated_at: stamp,
            ..VpnStatus::default()
        };
        let netwatch = crate::vpn::netwatch::spawn();
        let net_generation = netwatch
            .as_ref()
            .map(|watcher| watcher.generation())
            .unwrap_or(0);
        Worker {
            session,
            request,
            status,
            snapshot: NetworkSnapshot::default(),
            vpn_server_ips: Vec::new(),
            degraded_reason: None,
            split: None,
            full: None,
            tail: Vec::new(),
            script_log_lines: 0,
            script_log_error_logged: false,
            script_done: false,
            applied_mode,
            current_rules,
            current_default_policy,
            memo: crate::vpn::split::DnsMemo::default(),
            last_rules_raw: None,
            last_mode_check: Instant::now(),
            last_rules_check: Instant::now(),
            last_alive_check: Instant::now(),
            stale_alive_checks: 0,
            parent_lost: false,
            netwatch,
            net_generation,
            restart_requested: false,
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
        // 失败现场最重要：先抓一眼网卡/路由/NRPT，再清理。
        #[cfg(target_os = "windows")]
        self.dump_network_state("失败时");
        self.cleanup_routes();
        let _ = self.session.write_status(&self.status);
        // 认证失败、超时、脚本失败这些路径以前直接把 request.json 留在盘上，
        // 而里面有明文密码。失败一次就擦掉，不能等下一次「正常断开」。
        self.session.scrub_request();
        EXIT_FAILED
    }

    /// Windows：把当前网卡、IP、路由、NRPT 状态写进 worker.log。
    ///
    /// 现场机器拿不到管理员 shell 时，这是唯一能远程判断「路由装没装、
    /// NRPT 有没有生效」的证据。命令本身也会进 powershell.log。
    #[cfg(target_os = "windows")]
    fn dump_network_state(&mut self, label: &str) {
        let idx = self.status.tun_index;
        // 还没拿到 tunidx（认证失败、连接器不存在等）时也要留一份全局网络状态，
        // 否则失败现场只剩 NRPT/Dnscache，最关键的网卡和默认路由反而没有。
        let iface = if idx == 0 {
            r#"'--- adapters ---';
Get-NetAdapter | Format-Table Name,InterfaceDescription,Status,LinkSpeed,ifIndex -AutoSize | Out-String -Width 220;
'--- ipv4 ---';
Get-NetIPAddress -AddressFamily IPv4 | Format-Table IPAddress,PrefixLength,InterfaceIndex,InterfaceAlias -AutoSize | Out-String -Width 220;
'--- routes ---';
Get-NetRoute -AddressFamily IPv4 | Format-Table DestinationPrefix,NextHop,InterfaceIndex,RouteMetric -AutoSize | Out-String -Width 220;
'--- dns ---';
Get-DnsClientServerAddress | Format-Table InterfaceIndex,InterfaceAlias,AddressFamily,ServerAddresses -AutoSize | Out-String -Width 220;
"#
            .to_string()
        } else {
            format!(
                r#"'--- adapter ---';
Get-NetAdapter -InterfaceIndex {idx} | Format-List Name,InterfaceDescription,Status,LinkSpeed;
'--- ipv4 ---';
Get-NetIPAddress -InterfaceIndex {idx} -AddressFamily IPv4 | Format-List IPAddress,PrefixLength,PrefixOrigin;
'--- routes ---';
Get-NetRoute -InterfaceIndex {idx} -AddressFamily IPv4 | Format-Table DestinationPrefix,NextHop,RouteMetric -AutoSize | Out-String -Width 220;
'--- dns ---';
Get-DnsClientServerAddress -InterfaceIndex {idx} -AddressFamily IPv4 | Format-List ServerAddresses;
"#,
                idx = idx
            )
        };
        let script = format!(
            r#"$ErrorActionPreference='SilentlyContinue';
{iface}'--- nrpt ---';
Get-DnsClientNrptRule | Where-Object -Property DisplayName -Like 'ws-workbench:*' | Format-List DisplayName,Namespace,NameServers;
'--- dnscache ---';
Get-Service -Name Dnscache | Format-List Status,StartType
"#,
            iface = iface
        );
        match crate::winps::run(&script) {
            Ok(text) => self.log(&format!("[诊断] {} 网络状态:\n{}", label, text.trim())),
            Err(e) => self.log(&format!("[诊断] {} 读取网络状态失败：{}", label, e)),
        }
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
                "done" => {
                    self.script_done = value.trim() == "connect";
                    self.log(&format!(
                        "[脚本] 配置阶段结束：{}（{}）",
                        value,
                        if self.script_done {
                            "接口地址/MTU 已完成，可以装路由"
                        } else {
                            "非 connect，不触发接管"
                        }
                    ));
                }
                "error" => {
                    // 脚本主动回报的致命错误；openconnect 随后会以非零退出，
                    // 这里先原样打出来，方便没有 worker.log 时也能定位。
                    self.log(&format!("[脚本] 脚本报告错误：{}", value));
                }
                // 服务端下发的默认域名与 split-include 网段：**只记录、只展示，
                // 不参与分流**。走 VPN 的唯一依据是用户规则，否则「规则空着也能
                // 连内网」会变成界面上解释不了的隐式行为。
                "defdomain" => {
                    if let Some(domain) = crate::vpn::split::normalize_domain(value.trim()) {
                        if !self.status.server_scope.domains.contains(&domain) {
                            self.status.server_scope.domains.push(domain.clone());
                            self.log(&format!(
                                "[分流] 服务端下发内网域 {}：仅记录展示，不自动并入分流",
                                domain
                            ));
                        }
                    }
                }
                "split" => {
                    if let Ok((net, prefix)) = route::parse_cidr(value.trim()) {
                        let cidr = format!("{}/{}", std::net::Ipv4Addr::from(net), prefix);
                        if !self.status.server_scope.cidrs.contains(&cidr) {
                            self.status.server_scope.cidrs.push(cidr.clone());
                            self.log(&format!(
                                "[分流] 服务端下发网段 {}：仅记录展示，不自动并入分流",
                                cidr
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn tunnel_ready(&self) -> bool {
        self.script_done
            && !self.status.vpn_ip.is_empty()
            && (self.status.tun_index > 0 || !self.status.tun_dev.is_empty())
    }

    /// 读取脚本自己写的 `script.log`，这是隧道参数的真正来源。
    ///
    /// openconnect 执行脚本时用的是 `CreateProcessW(..., bInheritHandles=FALSE, ...)`，
    /// 脚本的标准输出**没有**接到 openconnect 的管道上，所以我们从子进程 stdout 里
    /// 永远读不到脚本的回报。脚本因此把同一份回报追加写进会话目录的 script.log，
    /// 由这里增量读取——不这么做 `tunnel_ready()` 永远不会为真，界面就会一直停在
    /// 「连接中」，直到 CONNECT_TIMEOUT（120s）才报超时。
    fn ingest_script_log(&mut self) {
        let path = self.session.script_log_path();
        // 文件还没出现是正常的启动瞬间，不要把它记成错误。
        if !path.exists() {
            return;
        }
        let text = match crate::vpn::script::read_log(&path) {
            Ok(text) => text,
            Err(e) => {
                // 文件存在却读失败（被占用/权限）才值得记，且只记一次。
                if !self.script_log_error_logged {
                    self.script_log_error_logged = true;
                    self.log(&format!("[脚本] 读取 script.log 失败：{}", e));
                }
                return;
            }
        };
        if self.script_log_error_logged {
            self.script_log_error_logged = false;
            self.log("[脚本] script.log 已可读取");
        }
        // 只消费「以换行结尾」的完整行。脚本用 FSO WriteLine 逐行追加，但读的
        // 那一瞬间仍可能拿到写了一半的行（尤其 UTF-16 下半个字符）；把半行当完整
        // 行处理会推进游标，`done=connect` / `tunidx=` 这类关键行就永远丢了，
        // 表现为「隧道建好了但界面一直停在连接中，120s 后超时」。
        let complete = match text.rfind('\n') {
            Some(index) => &text[..=index],
            None => "",
        };
        let lines: Vec<&str> = complete.lines().collect();
        // 新脚本在 connect 时会把上一份日志改名成 script.prev.log，文件行数
        // 可能比旧会话少；不重置游标就会永远读不到新文件里的 marker。
        if lines.len() < self.script_log_lines {
            self.log(&format!(
                "[脚本] script.log 从 {} 行变为 {} 行（已重建/截断），从头读取",
                self.script_log_lines,
                lines.len()
            ));
            self.script_log_lines = 0;
        }
        if lines.len() <= self.script_log_lines {
            return;
        }
        // 只取新增的完整行
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

    /// 把「认证失败」翻译成下一步能做的事。
    ///
    /// AnyConnect 服务端拒绝账号密码时只说一句 `Login failed`，而 worker 是往 stdin
    /// 写一次密码就关掉的，所以第二次提示密码时直接读到 EOF——那行
    /// `Password:fgetws (stdin): No error` 就是 EOF 的产物，不是「程序把密码弄丢了」。
    /// 光看日志很容易往「密码没传进去」的方向排查，其实是要先确认账号本身。
    fn auth_hint(&self) -> Option<String> {
        let text = self.tail.join("\n").to_lowercase();
        let rejected = text.contains("login failed")
            || text.contains("failed to complete authentication")
            || text.contains("authentication failed")
            || text.contains("invalid password")
            || text.contains("username or password");
        if !rejected {
            return None;
        }
        Some(String::from(
            "服务端拒绝了这个账号或密码：openconnect 只尝试一次，第二次要密码时 stdin 已经结束，\
             所以后面那句 `fgetws (stdin)` 不代表密码没送到。先确认这几件事：\
             用官方 AnyConnect 客户端或网页登录同一个账号，确认密码没改、账号没被锁；\
             账号属于非默认认证组时，在「额外参数」里加一行 `--authgroup=<组名>`；\
             需要短信/令牌二次验证的账号要在密码后面追加一次动态码。\
             想看服务端返回的原文，在「额外参数」里加一行 `-v` 再连一次。",
        ))
    }

    // ======================= 心跳与守护 =======================

    /// 心跳守卫：主进程被强杀时没人来收尾，心跳停了就自己走正常停止流程。
    ///
    /// 判据要宽松一点：心跳就是「每 2s 重写一个空文件」，被杀软短暂独占、
    /// 或者系统卡一下，mtime 落后个几秒都很正常。真机现场出现过隧道刚建好
    /// （CSTP connected）就被一次过期判定掐掉的情况——所以这里要求连续两次
    /// （间隔 ≥1s）都过期，并且把实测的秒数写进日志，下次能一眼分辨是
    /// 「主进程真没了」还是「心跳抖动」。
    fn check_parent_alive(&mut self) {
        // 先看主进程 PID：任务管理器「结束任务」时进程立刻消失，这条路径不用等
        // 心跳过期。心跳是给「PID 文件还没写进来 / 拿不到」的情况兜底的。
        if let Some(pid) = self.session.read_gui_pid() {
            if !process_running(pid) {
                if !self.parent_lost {
                    self.parent_lost = true;
                    self.log("[worker] 主进程已退出（PID 已消失），自动收尾并退出");
                }
                return;
            }
        }
        if self.last_alive_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_alive_check = Instant::now();
        let stale = match self.session.alive_age() {
            // 心跳文件还没出现：给启动留 10s 宽限期
            None => elapsed_since(&self.status.started_at) > Duration::from_secs(10),
            Some(age) => age > Duration::from_secs(10),
        };
        if !stale {
            self.stale_alive_checks = 0;
            return;
        }
        self.stale_alive_checks += 1;
        if self.stale_alive_checks < 2 {
            return;
        }
        if self.parent_lost {
            return;
        }
        self.parent_lost = true;
        let age = self
            .session
            .alive_age()
            .map(|age| age.as_secs())
            .unwrap_or(u64::MAX);
        self.log(&format!(
            "[worker] 主进程心跳已停止（{}s 未刷新），自动收尾并退出",
            age
        ));
    }

    /// 换网检测：系统报告有网卡地址变更后，比对物理默认出口是否真的变了。
    ///
    /// 只比「网关 + 接口索引」：换 Wi-Fi 时这两样一定变（新网段的新网关），
    /// 而隧道网卡自己上下线、DHCP 续租不会改变物理默认出口 —— 那些事件
    /// 不该触发重连，否则一连上就会自己把自己踢掉。
    fn poll_network_change(&mut self) {
        let Some(generation) = self.netwatch.as_ref().map(|watcher| watcher.generation()) else {
            return;
        };
        if generation == self.net_generation {
            return;
        }
        self.net_generation = generation;
        // 换网时地址事件会连着来好几条（旧地址删除、新地址加入），先等系统稳定，
        // 顺便把这段窗口里堆积的事件一起吃掉。
        thread::sleep(Duration::from_millis(1200));
        if let Some(watcher) = self.netwatch.as_ref() {
            self.net_generation = watcher.generation();
        }

        let before = self.snapshot.clone();
        self.take_snapshot();
        if !network_exit_changed(&before, &self.snapshot) {
            // 隧道网卡上下线、DHCP 续租都会走到这里，不用重连
            return;
        }
        self.log(&format!(
            "[网络] 物理出口变化：{} via {} (if#{}) -> {} via {} (if#{})",
            before.default_if_alias,
            before.default_gateway,
            before.default_if_index,
            self.snapshot.default_if_alias,
            self.snapshot.default_gateway,
            self.snapshot.default_if_index
        ));
        if self.status.phase != VpnPhase::Connected {
            // 连接过程中换网：这条连接按旧出口建，失败由 connect 超时 / 脚本失败处理，
            // 不必在这里打断（打断反而会让用户看到一次莫名其妙的「正在重连」）。
            self.log("[网络] 连接还在进行，本次不触发重连");
            return;
        }
        self.restart_requested = true;
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
        // 隧道还没就绪时不能真的布线：start_split_rules 会因为没有 tun_dev 直接失败，
        // 而 switch_mode 结尾会把阶段写成「已连接」——界面显示连上了，实际一条路由都没有。
        // 连接过程中只记住目标模式，由 start_engine 在隧道就绪后按新模式布线。
        if self.status.phase != VpnPhase::Connected || !self.tunnel_ready() {
            self.applied_mode = target;
            self.status.applied_mode = target.as_str().to_string();
            let _ = self.session.write_status(&self.status);
            self.log(&format!(
                "[模式] 连接还在进行，先记为 {} 模式，隧道就绪后按它布线",
                target.as_str()
            ));
            return;
        }
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
        self.apply_reloaded_rules();
    }

    /// 规则热更新后的动作：规则模式下只要隧道就绪就重新布线。
    ///
    /// 不能只看 `self.split.is_some()`：严格规则模式下「连接时规则为空」不会接管
    /// 任何流量，split 保持 None；此时用户新增规则必须真的把分流拉起来，否则规则
    /// 永远不生效，只能断开重连（macOS 现场踩过：空规则连上后加
    /// `DOMAIN-SUFFIX example.com`，内网打不开，重连才好）。所以判据是
    /// 「当前是不是规则模式 + 隧道是否就绪」，而不是分流是否已经在跑。
    fn apply_reloaded_rules(&mut self) {
        match self.applied_mode {
            ProxyMode::Rule => {
                // 必须等连接流程把布线做完（phase=Connected）再热更新：连接过程中
                // apply_mode 还在布线，这时抢先 restart 会和它重复创建一份
                // SplitTunnel，前一份的转发线程/路由不会被回收。
                if self.status.phase == VpnPhase::Connected && self.tunnel_ready() {
                    self.log(&format!(
                        "[规则] 已更新 {} 条规则，正在重新下发路由注入分流",
                        self.current_rules.len()
                    ));
                    self.restart_split();
                } else {
                    self.log(&format!(
                        "[规则] 已更新 {} 条规则，连接完成后按最新规则布线",
                        self.current_rules.len()
                    ));
                }
            }
            mode => self.log(&format!(
                "[规则] 已更新 {} 条规则，当前是{}模式（规则不参与布线），切回规则模式后生效",
                self.current_rules.len(),
                mode.as_str()
            )),
        }
    }

    /// 路由守护：VPN 客户端或系统可能在后台把我们的路由改掉，这里定期补回来
    fn guard_routes(&mut self) {
        // 全局模式：守护半默认路由 + 隧道 DNS
        if let Some(full) = self.full.as_ref() {
            full.guard();
            return;
        }
        // 规则模式：守护隧道路由 + 按域名解析配置。
        // 顺带把「绕过分流的既有连接」（典型是 nginx 这类长连接代理）报出来：
        // 路由改不了已建立的连接，只能在日志里点名，否则表现成「规则没生效」。
        let messages = match self.split.as_mut() {
            Some(split) => split.guard(),
            None => Vec::new(),
        };
        for line in messages {
            self.log(&line);
        }
    }

    // ======================= 路由注入式分流（规则模式） =======================

    /// 严格规则模式的分流计划：只由用户规则决定。
    ///
    /// 服务端下发的默认域名 / split-include 网段不进这里——它们只写进
    /// `status.server_scope` 供界面和日志对照。这个边界是产品要求：用户
    /// 要能仅凭规则列表解释「为什么某条流量进了 VPN」。
    fn rule_plan(&self) -> crate::vpn::split::SplitPlan {
        crate::vpn::split::plan(&self.current_rules, &self.current_default_policy)
    }

    /// 规则模式的落地实现：不捕获、不抢默认路由、不改系统 DNS，只做两类布线——
    ///
    /// 1. **IP 规则**（`IP-CIDR`）：命中的网段直接装成隧道路由，不经过 DNS，
    ///    连接完成即生效；「直连」例外装一条更具体的直连路由（macOS 与系统同前缀
    ///    冲突时用 `route change` 覆盖、断开还原，见 `split::ip_route_plan`）；
    /// 2. **域名规则**：把命中「走 VPN」的域名交给 VPN DNS 解析：macOS 写
    ///    `/etc/resolver/<域名>` + 本机转发器，Windows 写 NRPT 规则（见 `split.rs` /
    ///    `win_dns.rs`），再把解析出来的内网 IP 装 `/32` 隧道路由（macOS 在转发时
    ///    注入，Windows 由后台解析线程注入）。
    ///
    /// 默认路由和系统 DNS 一律不动，所以结构上不可能再出现「一连上就全网断」。
    /// 任何一步失败都只是「本次分流不生效」，隧道照旧，网络不受影响。
    fn start_split_rules(&mut self) -> Result<(), String> {
        let plan = self.rule_plan();
        for note in &plan.notes {
            self.log(&format!("[分流] {}", note));
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
            "[分流] 路由注入模式：域名 {:?}，IP 规则 {:?}，VPN DNS {:?}，隧道 {}",
            plan.domains,
            plan.ip_rules
                .iter()
                .map(|rule| rule.cidr())
                .collect::<Vec<_>>(),
            vpn_dns,
            tun_dev
        ));
        // 自己带 DNS 客户端的程序（典型是 nginx 的 `resolver` 指令、数据库驱动）
        // 会绕过 NRPT / 本机转发器直接问 8.8.8.8 之类的外部服务器，拿到的地址
        // 自然不在规则里。这条提示只在有域名规则时给，否则纯 IP 规则用不到。
        if !plan.domains.is_empty() {
            self.log(
                "[分流] 提示：自己配了 DNS 的程序（如 nginx 的 resolver 指令）不经过本机规则解析；\
                 把它指到 127.0.0.1:53 才能拿到按规则解析的内网地址",
            );
        }
        // VPN 服务器自己的公网 IP 绝不能进隧道：否则 openconnect 的 TLS/DTLS
        // 会被自己的路由抓回隧道打环（现场日志出现过「内网 IP 203.0.113.10
        // 已注入隧道」，那正是服务器地址）。
        let server_ips: Vec<std::net::Ipv4Addr> = self
            .vpn_server_ips
            .iter()
            .filter_map(|ip| ip.trim().parse().ok())
            .collect();
        match crate::vpn::split::SplitTunnel::apply(
            &self.session,
            &tun_dev,
            self.status.tun_index,
            &vpn_dns,
            &server_ips,
            &self.snapshot,
            self.memo.clone(),
            plan,
        ) {
            Ok(split) => {
                self.log(&format!(
                    "[分流] 路由注入分流已生效：{} 个域名 + {} 条 IP 路由走隧道 {}，\
                     {} 条固定隧道路由；默认路由与系统 DNS 均未改动（系统解析器不受影响）",
                    split.domain_count(),
                    split.ip_route_count(),
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
            let domains = split.domain_count();
            let ip_routes = split.ip_route_count();
            split.teardown();
            self.log(&format!(
                "[模式] 已撤掉规则分流的布线（{} 个域名、{} 条 IP 路由与动态注入的内网 IP 一并回收）",
                domains, ip_routes
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
        let fallback = if target == ProxyMode::Direct {
            "隧道已建立（直连模式，未接管任何流量）"
        } else {
            "隧道已建立"
        };
        let message = self.connected_message(fallback);
        self.set_phase(VpnPhase::Connected, &message);
    }

    /// 规则改了之后重新下发分流（域名名单可能变）。
    fn restart_split(&mut self) {
        if let Some(mut old) = self.split.take() {
            old.teardown();
        }
        self.degraded_reason = None;
        self.status.routes.clear();
        if let Err(e) = self.start_split_rules() {
            self.log(&format!("[分流] 重新下发分流失败：{}", e));
        }
        let message = self.connected_message("隧道已建立");
        self.set_phase(VpnPhase::Connected, &message);
    }

    /// 连接成功后写进状态栏的一句话：全局 / 分流 / 降级 / 兜底。
    ///
    /// 首次连接、运行期切模式、规则热更新三个入口共用，避免同一件事写出三套
    /// 措辞；`fallback` 由调用方给 —— 各入口对「什么都没接管」的说法不同。
    fn connected_message(&self, fallback: &str) -> String {
        if self.full.is_some() {
            return "隧道已建立，全局模式生效（全部流量走 VPN）".to_string();
        }
        if let Some(split) = self.split.as_ref() {
            return format!(
                "隧道已建立，路由注入分流已生效（{} 个域名 / {} 条 IP 路由 → {}）",
                split.domain_count(),
                split.ip_route_count(),
                split.tun_dev()
            );
        }
        if let Some(reason) = self.degraded_reason.as_ref() {
            return format!("已连接（未接管系统流量：{}）", reason);
        }
        fallback.to_string()
    }

    // ======================= 按代理模式布线 =======================

    /// 隧道就绪后按本次生效的代理模式布线。
    ///
    /// 三种模式都是「本地布线」，不再开捕获网卡：
    /// - 直连：隧道留着，什么都不改；
    /// - 规则：按域名的 DNS + 路由注入（`split.rs`），默认路由与系统 DNS 不动；
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
        if !self.request.connector_path.is_empty() {
            let (version_text, version_ok) = crate::process::run_with_timeout(
                Path::new(&self.request.connector_path),
                &["--version"],
                Duration::from_secs(10),
            );
            if version_ok {
                let first = version_text.lines().next().unwrap_or("").trim();
                self.log(&format!("[隧道] openconnect 版本：{}", first));
            } else {
                self.log(&format!(
                    "[隧道] 读取 openconnect 版本失败：{}",
                    version_text.lines().next().unwrap_or("").trim()
                ));
            }
        }
        let (file_name, content) = script::generate(&script::ScriptContext {
            session_dir: &self.session.dir,
            original_gateway: &self.snapshot.default_gateway,
        });
        let script_path = script_path_for_openconnect(&self.session.script_dir(), file_name)?;
        write_script_file(&script_path, &content)?;
        self.log(&format!("[脚本] 已生成 {}", script_path.display()));

        let mut request = self.request.clone();
        request.script_path = script_path.to_string_lossy().to_string();
        // 认证失败时最该先排除的是密码本身有没有问题，但密码又不能落进日志。
        // 只打三个不泄密的特征：长度、是否全 ASCII、首尾有没有空白。
        self.log(&format!(
            "[诊断] 密码长度 {}，{}，{}",
            request.password.chars().count(),
            if request.password.is_ascii() {
                "全 ASCII"
            } else {
                "含非 ASCII 字符（服务端与本地编码不一致时会被判错）"
            },
            if request.password.trim().len() == request.password.len() {
                "首尾无空白"
            } else {
                "首尾有空白（会一并送进服务端）"
            }
        ));
        // 返回真正下发的那份参数：self.request.script_path 还是空的，
        // 拿它拼日志会打出一句 `--script= vpn.example.com:1218` 误导排查
        let args = connector::openconnect_args(&request);
        let child = connector::spawn_openconnect(&request)?;
        Ok((child, args))
    }

    /// 跑一趟隧道：拉起 openconnect，守到它退出或收到停止请求。
    ///
    /// 返回 [`TunnelRun::Restart`] 表示「换网了，这趟不算数」：布线已经回收，
    /// 外层循环会按新出口重新拉起一次（见 [`Worker::run_openconnect`]）。
    fn run_tunnel(&mut self) -> TunnelRun {
        self.take_snapshot();
        // 连接超时按「这一趟」算，不看 status.started_at：换网重连时 started_at
        // 还是第一次连接的时间，用它会让第二趟一进循环就判超时。
        let attempt_started = Instant::now();

        let (mut child, spawn_args) = match self.connect_openconnect() {
            Ok(pair) => pair,
            Err(e) => return TunnelRun::Exited(self.fail(&e)),
        };
        self.log(&format!(
            "[隧道] openconnect 已启动，pid {}，程序 {}，参数 {}",
            child.id(),
            self.request.connector_path,
            spawn_args.join(" ")
        ));

        // Windows：把 openconnect 绑到「随 worker 一起退出」的作业对象上。
        // worker 被任务管理器强杀 / 崩溃时不会执行清理代码，只有这个作业对象能
        // 保证 openconnect 跟着结束，隧道不会留在后台。
        #[cfg(target_os = "windows")]
        let _tunnel_job = match crate::vpn::win_job::KillOnCloseJob::attach(&child) {
            Ok(job) => Some(job),
            Err(err) => {
                self.log(&format!(
                    "[隧道] 无法让 openconnect 随 worker 一起退出（{err}），继续连接"
                ));
                None
            }
        };

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
            self.poll_network_change();

            if self.parent_lost || self.session.is_stop_requested() {
                if !self.parent_lost {
                    self.log("[worker] 收到停止请求");
                }
                break;
            }

            // 换网后原隧道已经不成立：先把这趟的布线收干净，再交给外层按新出口重连。
            // 不能只重装路由就算完 —— openconnect 的 TCP 连接绑在旧地址上，
            // 它自己空闲时根本察觉不到链路已断（现场：状态一直「已连接」，内网全不通）。
            if self.restart_requested {
                self.restart_requested = false;
                self.log("[网络] 换网后原隧道已失效，断开并按新出口重连");
                let _ = child.kill();
                let _ = child.wait();
                self.drain_output(&rx);
                self.cleanup_routes();
                self.set_phase(VpnPhase::Connecting, "网络已切换，正在按新出口重连");
                return TunnelRun::Restart;
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
                         如需排查请把会话目录里的 worker.log、script.log、powershell.log 一并提供。",
                        raw, hint
                    );
                    return TunnelRun::Exited(self.fail(&reason));
                }
            }

            if !routes_applied && self.tunnel_ready() {
                if let Err(e) = self.start_engine() {
                    // 接管失败必须把隧道一起收掉，否则留下一个没人管辖的
                    // openconnect 进程，网络可能处于黑洞状态。
                    let _ = child.kill();
                    let _ = child.wait();
                    return TunnelRun::Exited(self.fail(&e));
                }
                #[cfg(target_os = "windows")]
                self.dump_network_state("接管成功后");
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
                        let fallback = if self.applied_mode == ProxyMode::Direct {
                            "隧道已建立（直连模式，未接管任何流量）"
                        } else {
                            "隧道已建立（未接管系统流量）"
                        };
                        let message = self.connected_message(fallback);
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
                        TunnelRun::Exited(EXIT_OK)
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
                        // 认证被拒同理：日志里只有一句 Login failed，看不出该改什么
                        if let Some(hint) = self.auth_hint() {
                            reason.push('。');
                            reason.push_str(&hint);
                        }
                        TunnelRun::Exited(self.fail(&reason))
                    };
                }
                Ok(None) => {}
                Err(e) => return TunnelRun::Exited(self.fail(&format!("等待 openconnect 失败: {}", e))),
            }

            if !routes_applied && attempt_started.elapsed() > CONNECT_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                let reason = format!(
                    "连接超时（{}s）：{}",
                    CONNECT_TIMEOUT.as_secs(),
                    self.tail_summary()
                );
                return TunnelRun::Exited(self.fail(&reason));
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
        TunnelRun::Exited(EXIT_OK)
    }

    /// 外层：跑隧道，换网就在同一进程里按新出口重连，其它结局直接交退出码。
    ///
    /// 为什么在 worker 内部重连，而不是让界面「断开 -> 再连一次」：
    /// 重连需要的密码就在 worker 手里的请求里，界面那边还要考虑用户有没有改配置、
    /// 规则是否已经热更新过；而且界面看到的是「连接中」而不是一次断开再连接，
    /// 不会闪出一条「已断开」通知。重连次数有上限，避免换网抖动时无限重连。
    fn run_openconnect(&mut self) -> i32 {
        let mut attempt = 0u32;
        loop {
            // 重连间隙里用户可能已经点了断开 / 主进程已经退出：别再拉起一趟
            if self.parent_lost || self.session.is_stop_requested() {
                return EXIT_OK;
            }
            match self.run_tunnel() {
                TunnelRun::Exited(code) => return code,
                TunnelRun::Restart => {
                    attempt += 1;
                    if attempt > MAX_NETWORK_RESTARTS {
                        return self.fail(&format!(
                            "换网后连续 {} 次重连都没能连上，请检查网络后手动重新连接",
                            MAX_NETWORK_RESTARTS
                        ));
                    }
                    self.log(&format!(
                        "[网络] 第 {} 次按新出口重连（最多 {} 次）",
                        attempt, MAX_NETWORK_RESTARTS
                    ));
                    // 给系统一点时间把新网卡的路由/DNS 稳定下来，否则刚拉起就失败
                    thread::sleep(Duration::from_millis(800));
                }
            }
        }
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

/// 物理出口是否换了。
///
/// Windows 比接口索引 + 网关：换 Wi-Fi 时两者都会变。macOS 拿不到接口索引
/// （恒为 0），只剩网关和接口名可比 —— 同一台机器换 Wi-Fi 时接口名都是 en0、
/// 网关却几乎一定不同；DHCP 续租把网关改掉的情况也该重连，方向是对的。
fn network_exit_changed(before: &NetworkSnapshot, after: &NetworkSnapshot) -> bool {
    if before.default_gateway.is_empty() && after.default_gateway.is_empty() {
        return false;
    }
    if before.default_gateway != after.default_gateway {
        return true;
    }
    if before.default_if_index != 0
        && after.default_if_index != 0
        && before.default_if_index != after.default_if_index
    {
        return true;
    }
    false
}

/// 两个 exe 路径是否指同一个程序文件。
///
/// 只用于启动时的一致性告警：Windows 盘符/路径大小写不敏感，斜杠方向也可能
/// 因来源不同而不一致；macOS/Linux 的路径大小写敏感，不能一律小写比较。
fn same_program(a: &Path, b: &Path) -> bool {
    if a.as_os_str().is_empty() || b.as_os_str().is_empty() {
        return false;
    }
    #[cfg(target_os = "windows")]
    let normalize = |path: &Path| path.to_string_lossy().replace('\\', "/").to_lowercase();
    #[cfg(not(target_os = "windows"))]
    let normalize = |path: &Path| path.to_string_lossy().to_string();
    normalize(a) == normalize(b)
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
    // worker 是独立进程（macOS 走 osascript 提权、Windows 走计划任务），入口环境里
    // 没有 LANG/LC_ALL；openconnect 会按系统语言翻译输出，而这里要靠
    // "Login failed" / "CSTP connected" / "Set up tun device failed" 这类英文行
    // 判断认证与隧道状态。整棵进程树钉死 C locale（worker 不拉起用户可见的 GUI 程序）。
    crate::process::pin_child_locale();

    let session = VpnSession::new(session_dir.to_path_buf());
    if let Err(e) = session.ensure() {
        eprintln!("[worker] {}", e);
        return EXIT_NO_REQUEST;
    }

    session.append_log(&format!("[worker] 启动，pid {}", std::process::id()));
    session.append_log(&format!(
        "[worker] 版本 {}，系统 {} {}，计算机 {}，用户 {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "未知".to_string()),
        std::env::var("USERNAME").unwrap_or_else(|_| "未知".to_string())
    ));

    // 先设好，才能把 cleanup_stale / restore_stale 里的 PowerShell 也记进
    // powershell.log（这些清理命令失败是“内网域名全解析不了”的常见原因）。
    #[cfg(target_os = "windows")]
    {
        let ps_log = session.dir.join("powershell.log");
        std::env::set_var("WS_VPN_PS_LOG", &ps_log);
        session.append_log(&format!(
            "[worker] PowerShell 完整日志：{}；脚本完整日志：{}（UTF-16LE）",
            ps_log.display(),
            session.script_log_path().display()
        ));
    }

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
    //
    // 现在把主进程的 exe 也一起打出来：便携版/安装版共用同一份会话目录时，
    // 计划任务可能还指向另一个旧 exe，日志里两个路径一不一样立刻就能看出来。
    let self_exe = std::env::current_exe().ok();
    let main_exe = Path::new(&worker.request.hook_exe);
    worker.log(&format!(
        "[worker] 提权状态：{}，程序 {}，主程序 {}，会话目录 {}",
        if crate::vpn::elevate::is_elevated() {
            "管理员"
        } else {
            "非管理员（建隧道、改路由、写 DNS 都会失败）"
        },
        self_exe
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "未知".to_string()),
        if main_exe.as_os_str().is_empty() {
            "未知".to_string()
        } else {
            main_exe.display().to_string()
        },
        worker.session.dir.display()
    ));
    if let Some(self_exe) = self_exe.as_deref() {
        if !main_exe.as_os_str().is_empty() && !same_program(self_exe, main_exe) {
            worker.log(
                "[worker] 警告：worker 与主进程不是同一个程序文件，计划任务很可能还指向旧版本。\
                 本次会按这份 worker 的旧逻辑运行（心跳判定、分流行为都可能与主程序不一致），\
                 请先在 VPN 页移除并重新注册免 UAC 计划任务。",
            );
        }
    }

    if worker.request.connector_path.is_empty() {
        return worker.fail("没有可用的连接器，请先在 VPN 页确认连接器状态");
    }

    // 先把阶段落成「准备中」。默认状态是 Disconnected，采集网络快照时会原样写盘，
    // 主进程的心跳线程若正好读到这一份，会误以为会话已经结束而停止续命。
    worker.set_phase(VpnPhase::Preparing, "正在准备连接");

    // 只剩 openconnect 一条通道，连接器路径一定是 openconnect 可执行文件
    let code = worker.run_openconnect();

    // 兜底：任何路径退出前都确保布线被回收。
    // 规则模式没有「路由表账本」，但它有解析器配置、隧道路由和后台解析线程要收，
    // 所以这几个条件都要算上，否则异常退出会把 `/etc/resolver` / NRPT 规则留在地上。
    if worker.split.is_some() || worker.full.is_some() || !worker.vpn_server_ips.is_empty() {
        worker.cleanup_routes();
    }
    let _ = worker.session.write_status(&worker.status);
    // 同上：连接请求里有明文密码，无论从哪条分支退出都不要留盘。
    worker.session.scrub_request();
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
        let dir = crate::test_support::TempDir::new("worker-script");
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
    fn ingest_script_log_reads_utf16_and_recovers_after_rotation() {
        let dir = crate::test_support::TempDir::new("worker-script-log-utf16");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
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

        let text = "WS-WORKBENCH ip4=10.10.0.2\n\
                    WS-WORKBENCH tunidx=7\n\
                    WS-WORKBENCH tundev=测试网卡\n\
                    WS-WORKBENCH done=connect\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(worker.session.script_log_path(), &bytes).unwrap();
        worker.ingest_script_log();
        assert_eq!(worker.status.vpn_ip, "10.10.0.2");
        assert_eq!(worker.status.tun_index, 7);
        assert_eq!(worker.status.tun_dev, "测试网卡");
        assert!(worker.script_done);
        assert!(worker.tunnel_ready());

        // 模拟 connect 时脚本把旧日志改成 script.prev.log，新日志更短：
        // 游标必须回退到 0，否则新文件的内容全被当成「已读」。
        let shorter = "WS-WORKBENCH ip4=10.9.0.3\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in shorter.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(worker.session.script_log_path(), &bytes).unwrap();
        worker.ingest_script_log();
        assert_eq!(worker.status.vpn_ip, "10.9.0.3");
    }

    /// 读到「写了一半的行」时不能消费它：那会把游标推过去，`done=connect`
    /// 这类关键回报永远丢失（现场表现为隧道已建好但界面一直停在连接中）。
    #[test]
    fn ingest_script_log_waits_for_a_complete_line() {
        let dir = crate::test_support::TempDir::new("worker-script-log-partial");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        let mut worker = Worker::new(session, request_with_rules(vec![]));

        // 半行：没有换行结尾
        let partial = "WS-WORKBENCH ip4=10.10.0.2";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in partial.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(worker.session.script_log_path(), &bytes).unwrap();
        worker.ingest_script_log();
        assert_eq!(worker.status.vpn_ip, "", "半行不能被当成完整行处理");

        // 补齐换行并追加后续行，两行都必须读到
        let complete = "WS-WORKBENCH ip4=10.10.0.2\nWS-WORKBENCH done=connect\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in complete.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(worker.session.script_log_path(), &bytes).unwrap();
        worker.ingest_script_log();
        assert_eq!(worker.status.vpn_ip, "10.10.0.2");
        assert!(worker.script_done);
    }

    #[test]
    fn tail_summary_picks_error_lines() {
        let dir = crate::test_support::TempDir::new("worker-session");
        let session = VpnSession::new(dir.path().to_path_buf());
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
    fn process_running_detects_self_and_missing_pid() {
        assert!(process_running(std::process::id()), "自己的进程必须判成存活");
        // 999999 远大于系统 pid_max，必然不存在（不能拿负数，Unix 上会被解释成 kill(-1)）
        assert!(
            !process_running(999_999),
            "不存在的 PID 必须判成已退出，否则强杀界面后 worker 不会收尾"
        );
    }

    #[test]
    fn single_stale_heartbeat_does_not_kill_the_tunnel() {
        // 现场：CSTP 刚连上，2 秒后 worker 就宣布「心跳已停止」把隧道收了。
        // 现在要求连续两次过期才判定，单次抖动不能误杀。
        let dir = crate::test_support::TempDir::new("worker-session-alive");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        session.touch_alive().unwrap();
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
        let mut worker = Worker::new(session.clone(), request);

        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30);
        {
            let file = std::fs::File::options()
                .write(true)
                .open(session.alive_path())
                .unwrap();
            file.set_modified(old).unwrap();
        }

        // Worker::new 里 last_alive_check 是「现在」，第一次调用会因为是刚检查过而跳过，
        // 所以每次都把上次检查时间往回拨，模拟每秒一次的轮询。
        worker.last_alive_check = std::time::Instant::now() - std::time::Duration::from_secs(2);
        worker.check_parent_alive();
        assert!(!worker.parent_lost, "第一次读到过期不能直接判定主进程没了");

        worker.last_alive_check = std::time::Instant::now() - std::time::Duration::from_secs(2);
        worker.check_parent_alive();
        assert!(worker.parent_lost, "连续两次过期才算真的没了");
    }

    #[test]
    fn dead_gui_pid_is_detected_immediately() {
        // 界面进程被强杀时 PID 文件里的进程已经不在了，worker 必须立刻收尾，
        // 不能等到心跳过期（否则用户会觉得「进程都结束了 VPN 还连着」）。
        let dir = crate::test_support::TempDir::new("worker-session-pid");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        session.write_gui_pid(999_999).unwrap();
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
        // 第一次调用就该判定，不需要等第二次心跳
        worker.check_parent_alive();
        assert!(worker.parent_lost, "PID 已消失时必须立刻判定主进程没了");
    }

    #[test]
    fn auth_failure_produces_actionable_hint() {
        // 现场日志：Login failed → 再问一次密码 → fgetws(stdin) EOF。
        // 提示必须说清「不是密码没送到」，否则下一个人还会往这个方向查。
        let dir = crate::test_support::TempDir::new("worker-session-auth");
        let session = VpnSession::new(dir.path().to_path_buf());
        let request = VpnRequest {
            conn_identity: String::new(),
            proxy_mode: "rule".to_string(),
            profile_id: "p".into(),
            profile_name: "n".into(),
            server: "s".into(),
            username: "u".into(),
            password: "12345678".into(),
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
        worker.tail.push("Please enter your password.".into());
        worker.tail.push("Login failed.".into());
        worker
            .tail
            .push("Password:fgetws (stdin): No error".into());
        let hint = worker.auth_hint().expect("认证失败必须给出提示");
        assert!(hint.contains("fgetws"));
        assert!(hint.contains("--authgroup"), "提示里要写出可以试的参数");
        assert!(hint.contains("-v"), "提示里要写出看服务端原文的办法");

        // 不是认证失败时不能误报成认证问题
        let clean_dir = crate::test_support::TempDir::new("worker-session-auth2");
        let mut clean = Worker::new(
            VpnSession::new(clean_dir.path().to_path_buf()),
            {
                let mut request = worker.request.clone();
                request.profile_id = "p2".into();
                request
            },
        );
        clean.tail.push("CSTP connected".into());
        assert!(clean.auth_hint().is_none());
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
        let dir = crate::test_support::TempDir::new("worker-session-2");
        let session = VpnSession::new(dir.path().to_path_buf());
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
        // 地址已经回报，但脚本还没写完 done：不能抢跑装路由
        assert!(!worker.tunnel_ready());
        worker.script_done = true;
        assert!(worker.tunnel_ready());
    }

    #[test]
    fn tun_setup_failure_gets_actionable_hint() {
        let dir = crate::test_support::TempDir::new("worker-session-3");
        let session = VpnSession::new(dir.path().to_path_buf());
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
    fn same_program_compares_paths_the_way_the_platform_does() {
        #[cfg(target_os = "windows")]
        {
            assert!(same_program(
                Path::new(r"D:\App\ws.exe"),
                Path::new("d:/app/WS.EXE")
            ));
            assert!(!same_program(
                Path::new(r"D:\App\ws.exe"),
                Path::new(r"D:\App2\ws.exe")
            ));
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert!(same_program(
                Path::new("/opt/ws/ws-workbench"),
                Path::new("/opt/ws/ws-workbench")
            ));
            // macOS/Linux 大小写敏感，不能把两个不同目录当成同一个
            assert!(!same_program(
                Path::new("/opt/ws/ws-workbench"),
                Path::new("/opt/WS/ws-workbench")
            ));
        }
        assert!(!same_program(Path::new(""), Path::new("x")));
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

    fn request_with_rules(rules: Vec<VpnRule>) -> VpnRequest {
        VpnRequest {
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
            rules,
            default_policy: "DIRECT".into(),
            extra_args: vec![],
            script_path: "s".into(),
            hook_exe: "h".into(),
            created_at: String::new(),
        }
    }

    /// 严格规则模式：服务端下发的内网范围只用于展示，绝不自动并入分流计划。
    #[test]
    fn server_scope_is_visible_but_never_routed() {
        let dir = crate::test_support::TempDir::new("worker-server-scope");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        let mut worker = Worker::new(session, request_with_rules(vec![]));

        worker.handle_line("WS-WORKBENCH defdomain=example.com.");
        worker.handle_line("WS-WORKBENCH split=198.51.100.7/22");
        worker.handle_line("WS-WORKBENCH split=198.51.100.9/22");

        assert_eq!(
            worker.status.server_scope.domains,
            vec!["example.com".to_string()],
            "服务端下发的默认域名要归一化后展示在状态里"
        );
        assert_eq!(
            worker.status.server_scope.cidrs,
            vec!["198.51.100.0/22".to_string()],
            "服务端下发的网段要归一化并去重后展示"
        );

        let plan = worker.rule_plan();
        assert!(
            plan.domains.is_empty(),
            "空规则时不能因为服务端下发了默认域名就分流"
        );

        // 换个域名行为必须一模一样：服务端推 example.org 也只展示、不分流
        worker.handle_line("WS-WORKBENCH defdomain=example.org");
        worker.handle_line("WS-WORKBENCH split=10.10.0.0/16");
        assert_eq!(
            worker.status.server_scope.domains,
            vec!["example.com".to_string(), "example.org".to_string()]
        );
        assert_eq!(
            worker.status.server_scope.cidrs,
            vec!["198.51.100.0/22".to_string(), "10.10.0.0/16".to_string()]
        );
        let plan = worker.rule_plan();
        assert!(plan.domains.is_empty(), "服务端下发的范围永远不参与分流");
    }

    #[test]
    fn only_user_rules_build_the_plan() {
        let rule = VpnRule {
            id: "r1".into(),
            kind: "DOMAIN-SUFFIX".into(),
            value: "corp.example.com".into(),
            policy: "VPN".into(),
            remark: String::new(),
            enabled: true,
        };
        let dir = crate::test_support::TempDir::new("worker-user-rule-plan");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        let mut worker = Worker::new(session, request_with_rules(vec![rule]));

        worker.handle_line("WS-WORKBENCH defdomain=example.com");
        let plan = worker.rule_plan();
        assert_eq!(
            plan.domains,
            vec!["corp.example.com".to_string()],
            "分流计划只能来自用户规则，不能混进服务端下发的域"
        );
    }

    /// 现场（macOS）：空规则连上后新增 `DOMAIN-SUFFIX example.com`，内网打不开，
    /// 必须断开重连才行。
    ///
    /// 根因是规则热更新只看「split 是否已经起来」：严格规则模式下空规则时
    /// split 恒为 None，于是新增规则被当成「切回规则模式后生效」直接跳过。
    #[test]
    fn adding_rules_after_an_empty_rule_connect_reapplies_split() {
        let dir = crate::test_support::TempDir::new("worker-hot-add-rules");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        let mut request = request_with_rules(vec![]);
        // 不真的做 DNS 解析：server 用回环地址，免得测试卡在主机名解析上
        request.server = "127.0.0.1".into();
        let mut worker = Worker::new(session.clone(), request);

        // 连接已建立，但严格规则模式下空规则不接管任何流量
        worker.status.phase = VpnPhase::Connected;
        worker.status.vpn_ip = "198.51.100.149".into();
        worker.status.tun_dev = "utun-test".into();
        worker.script_done = true;
        assert!(worker.tunnel_ready());
        assert!(worker.split.is_none(), "空规则时不应有分流");

        // 连接后新增一条域名后缀规则：主进程写 rules.json，worker 轮询到变化
        let rule = VpnRule {
            id: "r1".into(),
            kind: "DOMAIN-SUFFIX".into(),
            value: "example.com".into(),
            policy: "VPN".into(),
            remark: String::new(),
            enabled: true,
        };
        session.write_rules(&[rule], "DIRECT").unwrap();
        worker.last_rules_check = Instant::now() - Duration::from_secs(1);
        worker.poll_rules_change();

        assert_eq!(worker.current_rules.len(), 1);
        let log = session.read_log_tail(80).join("\n");
        assert!(
            log.contains("正在重新下发路由注入分流"),
            "新增规则后必须真的重新下发，而不能因为 split 还没起来就跳过；日志：\n{log}"
        );
    }

    /// 非规则模式连接时也要把规则装进 worker：用户运行期切回规则模式时
    /// 直接可用，不必断开重连。
    ///
    /// 现场：直连模式连接后切规则模式，日志「域名 []，IP 规则 []」，
    /// 原因是前端连接时只给规则模式发规则，worker 的规则表是空的。
    #[test]
    fn rules_are_loaded_even_when_connecting_in_direct_mode() {
        let dir = crate::test_support::TempDir::new("worker-direct-keeps-rules");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        let rule = VpnRule {
            id: "r1".into(),
            kind: "DOMAIN-SUFFIX".into(),
            value: "example.com".into(),
            policy: "VPN".into(),
            remark: String::new(),
            enabled: true,
        };
        let mut request = request_with_rules(vec![rule]);
        request.proxy_mode = "direct".into();
        let worker = Worker::new(session, request);

        assert_eq!(worker.current_rules.len(), 1, "直连模式也要保留规则");
        assert_eq!(worker.rule_plan().domains, vec!["example.com".to_string()]);
    }

    /// 连接流程还没走完（phase 不是 Connected）时改规则不能抢先布线：
    /// 否则会和 apply_mode 重复创建一份 SplitTunnel。
    #[test]
    fn rules_change_during_connect_waits_for_wiring() {
        let dir = crate::test_support::TempDir::new("worker-hot-rules-connecting");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        let mut request = request_with_rules(vec![]);
        request.server = "127.0.0.1".into();
        let mut worker = Worker::new(session.clone(), request);
        worker.status.phase = VpnPhase::ConfiguringRoutes;
        worker.status.vpn_ip = "198.51.100.149".into();
        worker.status.tun_dev = "utun-test".into();
        worker.script_done = true;
        assert!(worker.tunnel_ready());

        let rule = VpnRule {
            id: "r1".into(),
            kind: "DOMAIN-SUFFIX".into(),
            value: "example.com".into(),
            policy: "VPN".into(),
            remark: String::new(),
            enabled: true,
        };
        session.write_rules(&[rule], "DIRECT").unwrap();
        worker.last_rules_check = Instant::now() - Duration::from_secs(1);
        worker.poll_rules_change();

        let log = session.read_log_tail(80).join("\n");
        assert!(
            log.contains("连接完成后按最新规则布线"),
            "连接中改规则应等连接流程收尾；日志：\n{log}"
        );
        assert!(worker.split.is_none(), "连接中不应抢先创建分流");
    }

    /// 连接中切模式同理：不能立刻布线（tun 还没就绪，布线必失败），更不能
    /// 顺手把阶段写成「已连接」——那样界面显示连上了，实际一条路由都没有。
    #[test]
    fn mode_change_during_connect_waits_for_tunnel() {
        let dir = crate::test_support::TempDir::new("worker-hot-mode-connecting");
        let session = VpnSession::new(dir.path().to_path_buf());
        session.ensure().unwrap();
        let mut request = request_with_rules(vec![]);
        request.server = "127.0.0.1".into();
        request.proxy_mode = "rule".into();
        let mut worker = Worker::new(session.clone(), request);
        worker.status.phase = VpnPhase::Connecting;
        assert!(!worker.tunnel_ready());

        session.write_mode("global").unwrap();
        worker.last_mode_check = Instant::now() - Duration::from_secs(1);
        worker.poll_mode_change();

        assert_eq!(
            worker.applied_mode,
            ProxyMode::Global,
            "目标模式要记住，隧道就绪后按它布线"
        );
        assert_eq!(
            worker.status.applied_mode, "global",
            "状态里的生效模式要跟着目标走"
        );
        assert_eq!(
            worker.status.phase,
            VpnPhase::Connecting,
            "隧道还没就绪，不能把阶段写成已连接"
        );
        assert!(worker.full.is_none(), "还没就绪时不应安装全局路由");
    }

    fn snapshot(alias: &str, gateway: &str, index: u32) -> NetworkSnapshot {
        NetworkSnapshot {
            default_gateway: gateway.to_string(),
            default_if_index: index,
            default_if_alias: alias.to_string(),
            local_subnets: vec![],
        }
    }

    /// 换 Wi-Fi：新网段对应新网关，必须判定为「出口变了」并触发重连。
    #[test]
    fn switching_wifi_network_is_detected() {
        let before = snapshot("WLAN", "192.168.1.1", 12);
        let after = snapshot("WLAN", "10.0.0.1", 12);
        assert!(
            network_exit_changed(&before, &after),
            "网关变了就是换网，哪怕还是同一块网卡"
        );
    }

    /// 同一网关换到了另一块网卡（有线插上、Wi-Fi 退居备用）也算换网。
    #[test]
    fn switching_interface_with_same_gateway_is_detected() {
        let before = snapshot("WLAN", "192.168.1.1", 12);
        let after = snapshot("以太网", "192.168.1.1", 7);
        assert!(network_exit_changed(&before, &after));
    }

    /// 隧道网卡自己上下线、DHCP 续租不改默认出口：不能重连，
    /// 否则一连上就会被自己的地址变更事件踢掉，陷入「连上-重连」循环。
    #[test]
    fn tunnel_churn_does_not_look_like_a_network_switch() {
        let same = snapshot("WLAN", "192.168.1.1", 12);
        assert!(!network_exit_changed(&same, &same));
        // macOS 没有接口索引（恒为 0），网关没变就不算换网
        let mac_before = snapshot("en0", "192.168.1.1", 0);
        let mac_after = snapshot("en0", "192.168.1.1", 0);
        assert!(!network_exit_changed(&mac_before, &mac_after));
    }

    /// 两份快照都没拿到物理默认出口（例如全部流量已被 VPN 接管）时，
    /// 空网关不应被当成「变了」。
    #[test]
    fn missing_gateway_on_both_sides_is_not_a_change() {
        let before = NetworkSnapshot::default();
        let after = NetworkSnapshot::default();
        assert!(!network_exit_changed(&before, &after));
    }
}
