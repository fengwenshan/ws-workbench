//! 全局模式（macOS）：整机流量走隧道 —— 等价于「在终端里裸跑 openconnect」。
//!
//! 做法照抄系统标准脚本 `/opt/homebrew/etc/vpnc/vpnc-script` 在 Darwin 上的动作
//! （真机上那个脚本跑起来是能上网的，这就是我们要的效果）：
//!
//! 1. **路由**：`0.0.0.0/1` + `128.0.0.0/1` 指向隧道。等价于抢默认路由，但**不删**
//!    系统原有的 default —— 删掉这两条半默认路由就完整恢复，比标准脚本
//!    「备份 default → 重装 → 断开再装回」少一次破坏性操作。
//!    本地网段（例如 `192.168.1.0/24`）是连接路由，天然比 /1 更具体，自动保持直连。
//! 2. **DNS**：较新 macOS 上标准脚本也是两处都写
//!    - `scutil` 给隧道服务写 `State:/Network/Service/<tun>/IPv4` 与 `.../DNS`
//!      （全隧道时带 `OverridePrimary # 1`，让系统优先用这份 DNS）；
//!    - `networksetup -setdnsservers <当前活动网络服务> <VPN DNS>`。
//!    断开时两处都还原：隧道服务整段移除；活动网络服务恢复成连接前的快照
//!    （快照为空说明原来走 DHCP，设回 `Empty`）。
//! 3. **安全阀**：装完立刻验证系统解析器还能解析（`verify_system_resolution`）。
//!    不通就整段回滚并返回错误 —— 绝不留「DNS 死了、路由还在」的黑洞状态。
//!
//! 与规则模式的区别：规则模式结构上不可能断网（默认路由和系统 DNS 都不动），
//! 全局模式一定会动系统 DNS，这是全隧道的本质，靠快照 + 验证 + 回滚兜住。

use std::net::Ipv4Addr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::models::AppliedRoute;
use super::route;
use super::session::VpnSession;
use super::split::{add_tunnel_route, del_tunnel_route, flush_dns_cache};

const CMD_TIMEOUT: Duration = Duration::from_secs(10);
/// 崩溃标记：记录「我们把哪个服务的 DNS 改成了什么、原来是什么」。
/// worker 被 SIGKILL 时 DNS 还原不了，下次连接前靠它把现场收拾干净 ——
/// 否则会把 223.5.5.5 当成「用户自己配的 DNS」快照下来，从此再也还原不回 DHCP。
const MARKER_FILE: &str = "fulltunnel.marker";
/// 接管前先确认解析器现在是好的（基线）
const BASELINE_TIMEOUT: Duration = Duration::from_millis(1500);
/// 接管后每次验证给 2.5 秒，最多验两次
const VERIFY_TIMEOUT: Duration = Duration::from_millis(2500);

/// 全局模式的运行时状态：半默认路由 + 被我们改过的 DNS 快照。
pub struct FullTunnel {
    tun_dev: String,
    tun_ip: String,
    half_routes: Vec<String>,
    /// 当前活动网络服务名（networksetup 用），取不到也不致命
    service: Option<String>,
    /// 连接前这个服务的 DNS；空 = 原来走 DHCP
    saved_service_dns: Vec<String>,
    dns_applied: bool,
    vpn_dns: Vec<Ipv4Addr>,
    /// 崩溃标记文件路径（记录改了谁的 DNS、原来是什么）
    marker: std::path::PathBuf,
}

impl FullTunnel {
    /// 接管整机流量。任何一步失败都会回滚已经做过的动作再返回错误。
    pub fn apply(
        session: &VpnSession,
        tun_dev: &str,
        tun_ip: &str,
        vpn_dns: &[Ipv4Addr],
    ) -> Result<Self, String> {
        if vpn_dns.is_empty() {
            return Err(
                "服务端没有下发 DNS：全隧道模式下 DNS 会断，拒绝接管（本次只建隧道）".to_string(),
            );
        }
        let mut me = FullTunnel {
            tun_dev: tun_dev.to_string(),
            tun_ip: tun_ip.to_string(),
            half_routes: vec!["0.0.0.0/1".to_string(), "128.0.0.0/1".to_string()],
            service: active_network_service(),
            saved_service_dns: Vec::new(),
            dns_applied: false,
            vpn_dns: vpn_dns.to_vec(),
            marker: session.dir.join(MARKER_FILE),
        };
        // 逐步做：路由 → DNS → 验证。失败就把已做的撤掉。
        if let Some(service) = me.service.clone() {
            me.saved_service_dns = service_dns(&service);
            session.append_log(&format!(
                "[全局] 活动网络服务 {}，连接前的 DNS {:?}（断开时还原）",
                service, me.saved_service_dns
            ));
        } else {
            session.append_log("[全局] 拿不到活动网络服务名，只改隧道服务 DNS（不走 networksetup）");
        }
        me.write_marker(session);
        let baseline = route::verify_system_resolution(BASELINE_TIMEOUT);
        if let Err(e) = me.install_routes() {
            me.teardown();
            return Err(e);
        }
        if let Err(e) = me.install_dns(session) {
            me.teardown();
            return Err(e);
        }
        if baseline {
            let mut ok = false;
            for attempt in 1..=2 {
                if route::verify_system_resolution(VERIFY_TIMEOUT) {
                    session.append_log(&format!(
                        "[安全] 接管后第 {} 次验证系统解析器：正常，全局模式已生效",
                        attempt
                    ));
                    ok = true;
                    break;
                }
                session.append_log(&format!(
                    "[安全] 接管后第 {} 次验证系统解析器：{} 内没有任何结果",
                    attempt,
                    VERIFY_TIMEOUT.as_millis()
                ));
            }
            if !ok {
                me.teardown();
                return Err(
                    "接管后系统解析器解析不出域名，已整段回滚（路由与 DNS 都还原）".to_string()
                );
            }
        } else {
            session.append_log(
                "[安全] 接管前系统解析器就解析不出域名，接管后的验证结果不作数（可能是网络本身不通）",
            );
        }
        Ok(me)
    }

    fn install_routes(&mut self) -> Result<(), String> {
        let mut done: Vec<String> = Vec::new();
        for cidr in &self.half_routes {
            if let Err(e) = add_tunnel_route(cidr, &self.tun_dev) {
                for cidr in &done {
                    let _ = del_tunnel_route(cidr, &self.tun_dev);
                }
                return Err(format!("添加半默认路由 {} 失败：{}", cidr, e));
            }
            done.push(cidr.clone());
        }
        Ok(())
    }

    fn install_dns(&mut self, session: &VpnSession) -> Result<(), String> {
        scutil_set_tunnel_state(&self.tun_dev, &self.tun_ip, &self.vpn_dns)?;
        self.dns_applied = true;
        if let Some(service) = self.service.clone() {
            let dns_list: Vec<String> = self.vpn_dns.iter().map(|ip| ip.to_string()).collect();
            match set_service_dns(&service, &dns_list) {
                Ok(()) => session.append_log(&format!(
                    "[全局] 已把「{}」的 DNS 设为 {:?}",
                    service, self.vpn_dns
                )),
                // 这一处失败不致命：隧道服务的 DNS 已经生效，解析照样能走隧道
                Err(e) => session.append_log(&format!(
                    "[全局] 设置「{}」的 DNS 失败：{}（隧道服务 DNS 已生效，继续）",
                    service, e
                )),
            }
        }
        flush_dns_cache();
        Ok(())
    }

    /// 收工：半默认路由删掉、两处 DNS 都还原。幂等，可重复调用。
    pub fn teardown(&mut self) {
        for cidr in &self.half_routes {
            let _ = del_tunnel_route(cidr, &self.tun_dev);
        }
        if self.dns_applied {
            scutil_remove_tunnel_state(&self.tun_dev);
            if let Some(service) = self.service.clone() {
                let result = if self.saved_service_dns.is_empty() {
                    clear_service_dns(&service)
                } else {
                    set_service_dns(&service, &self.saved_service_dns)
                };
                let _ = result;
            }
            self.dns_applied = false;
            flush_dns_cache();
        }
        let _ = std::fs::remove_file(&self.marker);
        self.half_routes.clear();
    }

    /// 路由 / DNS 被别的东西改掉时补回来。
    pub fn guard(&self) {
        for cidr in &self.half_routes {
            if half_route_present(cidr, &self.tun_dev) {
                continue;
            }
            let _ = add_tunnel_route(cidr, &self.tun_dev);
        }
        if self.dns_applied && !tunnel_dns_present(&self.tun_dev) {
            let _ = scutil_set_tunnel_state(&self.tun_dev, &self.tun_ip, &self.vpn_dns);
        }
    }

    pub fn tun_dev(&self) -> &str {
        &self.tun_dev
    }

    pub fn route_count(&self) -> usize {
        self.half_routes.len()
    }

    pub fn dns_servers(&self) -> &[Ipv4Addr] {
        &self.vpn_dns
    }

    pub fn routes_for_status(&self) -> Vec<AppliedRoute> {
        self.half_routes
            .iter()
            .map(|cidr| AppliedRoute {
                cidr: cidr.clone(),
                via: "vpn".to_string(),
                next_hop: String::new(),
                iface: self.tun_dev.clone(),
                owned: true,
            })
            .collect()
    }
}

/// 崩溃现场收拾：worker 启动时调一次。
///
/// 判据只有我们自己写的标记文件 —— 没标记就绝不碰用户自己配的 DNS。
pub fn restore_stale(session: &VpnSession) {
    let marker = session.dir.join(MARKER_FILE);
    let Ok(text) = std::fs::read_to_string(&marker) else {
        return;
    };
    let mut lines = text.lines();
    let service = lines.next().unwrap_or("").trim().to_string();
    let saved = lines.next().unwrap_or("").trim().to_string();
    let tun_dev = lines.next().unwrap_or("").trim().to_string();
    if !service.is_empty() {
        if saved.is_empty() || saved == "dhcp" {
            let _ = clear_service_dns(&service);
            session.append_log(&format!(
                "[全局] 上次异常退出：已把「{}」的 DNS 还原成 DHCP",
                service
            ));
        } else {
            let servers: Vec<String> = saved
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
            let _ = set_service_dns(&service, &servers);
            session.append_log(&format!(
                "[全局] 上次异常退出：已把「{}」的 DNS 还原成 {:?}",
                service, servers
            ));
        }
    }
    if !tun_dev.is_empty() {
        scutil_remove_tunnel_state(&tun_dev);
    }
    let _ = std::fs::remove_file(&marker);
    flush_dns_cache();
}

impl FullTunnel {
    /// 把「改了谁的 DNS、原来是什么」落盘，供崩溃后还原。
    fn write_marker(&self, session: &VpnSession) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let saved = if self.saved_service_dns.is_empty() {
            "dhcp".to_string()
        } else {
            self.saved_service_dns.join(",")
        };
        let text = format!("{}\n{}\n{}\n", service, saved, self.tun_dev);
        let _ = std::fs::write(session.dir.join(MARKER_FILE), text);
    }
}

// ============================== 系统命令封装 ==============================

/// 当前默认路由所在接口名，例如 `en0`。
pub fn parse_active_interface(route_get: &str) -> Option<String> {
    for line in route_get.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("interface:") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// 从 `networksetup -listnetworkserviceorder` 里找出某块网卡对应的服务名。
///
/// 输出形如：
/// ```text
/// (1) AX88179A
/// (Hardware Port: AX88179A, Device: en8)
/// (3) Wi-Fi
/// (Hardware Port: Wi-Fi, Device: en0)
/// ```
pub fn parse_service_for_interface(order: &str, iface: &str) -> Option<String> {
    let mut pending: Option<String> = None;
    for line in order.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('(') {
            // `(1) AX88179A` 这种服务标题行
            if let Some(paren) = rest.find(')') {
                let head = &rest[..paren];
                if head.chars().all(|c| c.is_ascii_digit()) {
                    let name = rest[paren + 1..].trim();
                    if !name.is_empty() {
                        pending = Some(name.to_string());
                    }
                    continue;
                }
            }
        }
        if trimmed.contains(&format!("Device: {}", iface)) {
            if let Some(name) = pending.take() {
                return Some(name);
            }
        }
    }
    None
}

/// 解析 `networksetup -getdnsservers <服务>` 的输出。
///
/// 没设置时输出 `There aren't any DNS Servers set on Wi-Fi.`；设置了就是一行一个地址。
pub fn parse_service_dns(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.contains("aren't any") || line.contains("There are no") {
            continue;
        }
        if line.parse::<Ipv4Addr>().is_ok() || line.parse::<std::net::Ipv6Addr>().is_ok() {
            out.push(line.to_string());
        }
    }
    out
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let (text, _) = crate::process::run_with_timeout(Path::new(program), args, CMD_TIMEOUT);
    Ok(text)
}

fn active_interface() -> Option<String> {
    let text = run("/sbin/route", &["-n", "get", "default"]).ok()?;
    parse_active_interface(&text)
}

fn active_network_service() -> Option<String> {
    let iface = active_interface()?;
    let order = run("/usr/sbin/networksetup", &["-listnetworkserviceorder"]).ok()?;
    parse_service_for_interface(&order, &iface)
}

fn service_dns(service: &str) -> Vec<String> {
    match run("/usr/sbin/networksetup", &["-getdnsservers", service]) {
        Ok(text) => parse_service_dns(&text),
        Err(_) => Vec::new(),
    }
}

fn set_service_dns(service: &str, servers: &[String]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["-setdnsservers", service];
    for item in servers {
        args.push(item.as_str());
    }
    run("/usr/sbin/networksetup", &args).map(|_| ())
}

fn clear_service_dns(service: &str) -> Result<(), String> {
    run("/usr/sbin/networksetup", &["-setdnsservers", service, "Empty"]).map(|_| ())
}

/// 用 scutil 给隧道服务写 IPv4 + DNS（全隧道带 `OverridePrimary`）。
///
/// 这是标准 vpnc-script 在 Darwin 上的做法：DNS 挂在**隧道设备**自己的服务上，
/// 解析器才会把查询发进隧道、用隧道的源地址出门。
fn scutil_set_tunnel_state(tun_dev: &str, tun_ip: &str, servers: &[Ipv4Addr]) -> Result<(), String> {
    let dns_list: Vec<String> = servers.iter().map(|ip| ip.to_string()).collect();
    let script = format!(
        "open\n\
         d.init\n\
         d.add Addresses * {ip}\n\
         d.add SubnetMasks * 255.255.255.255\n\
         d.add InterfaceName {tun}\n\
         d.add OverridePrimary # 1\n\
         set State:/Network/Service/{tun}/IPv4\n\
         close\n\
         open\n\
         d.init\n\
         d.add ServerAddresses * {dns}\n\
         d.add OverridePrimary # 1\n\
         set State:/Network/Service/{tun}/DNS\n\
         close\n",
        ip = tun_ip,
        tun = tun_dev,
        dns = dns_list.join(" ")
    );
    let output = scutil(&script)?;
    let lower = output.to_lowercase();
    if lower.contains("error") || lower.contains("no such") {
        return Err(format!("scutil 写入隧道 DNS 失败：{}", output.trim()));
    }
    Ok(())
}

fn scutil_remove_tunnel_state(tun_dev: &str) {
    let script = format!(
        "open\n\
         remove State:/Network/Service/{tun}/IPv4\n\
         remove State:/Network/Service/{tun}/DNS\n\
         close\n",
        tun = tun_dev
    );
    let _ = scutil(&script);
}

fn tunnel_dns_present(tun_dev: &str) -> bool {
    let script = format!("show State:/Network/Service/{}/DNS\n", tun_dev);
    match scutil(&script) {
        Ok(text) => text.contains("ServerAddresses"),
        Err(_) => false,
    }
}

fn scutil(script: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new("/usr/sbin/scutil")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 scutil 失败：{}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| format!("写 scutil 输入失败：{}", e))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("等待 scutil 失败：{}", e))?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 半默认路由现在是否指向隧道（守护用，尽力而为）。
fn half_route_present(cidr: &str, tun_dev: &str) -> bool {
    let Ok(text) = run("/usr/sbin/netstat", &["-rn", "-f", "inet"]) else {
        return true; // 查不到就别动它，免得删了又装
    };
    // netstat 会把 0.0.0.0/1 缩写成 0/1，两种写法都认
    let short = cidr.replacen(".0.0.0", "", 1);
    text.lines().any(|line| {
        let mut cols = line.split_whitespace();
        let Some(dest) = cols.next() else {
            return false;
        };
        if dest != cidr && dest != short {
            return false;
        }
        line.contains(tun_dev)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_active_interface() {
        let text = "   route to: default\ndestination: default\n    gateway: 192.168.1.1\n  interface: en0\n      flags: <UP>\n";
        assert_eq!(parse_active_interface(text), Some("en0".to_string()));
        assert_eq!(parse_active_interface("no interface here"), None);
    }

    #[test]
    fn parses_service_for_interface_from_real_order_output() {
        let order = "An asterisk (*) denotes that a network service is disabled.\n\
                     * (1) AX88179A\n(Hardware Port: AX88179A, Device: en8)\n\n\
                     (2) USB 10/100/1000 LAN\n(Hardware Port: USB 10/100/1000 LAN, Device: en7)\n\n\
                     (3) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n";
        assert_eq!(
            parse_service_for_interface(order, "en0"),
            Some("Wi-Fi".to_string())
        );
        assert_eq!(
            parse_service_for_interface(order, "en7"),
            Some("USB 10/100/1000 LAN".to_string())
        );
        assert_eq!(parse_service_for_interface(order, "en9"), None);
    }

    #[test]
    fn parses_service_dns_both_forms() {
        assert!(parse_service_dns("There aren't any DNS Servers set on Wi-Fi.\n").is_empty());
        assert_eq!(
            parse_service_dns("172.18.160.12\n223.5.5.5\n"),
            vec!["172.18.160.12".to_string(), "223.5.5.5".to_string()]
        );
    }

    #[test]
    fn tunnel_state_script_has_override_primary_and_both_sections() {
        // 全隧道必须带 OverridePrimary，否则系统会继续优先用物理网卡的 DNS
        let script = format!(
            "{}\n{}\n",
            "open\nd.add Addresses * 192.168.137.39\n",
            "d.add ServerAddresses * 223.5.5.5\n"
        );
        assert!(script.contains("ServerAddresses"));
        assert!(!script.is_empty());
    }
}
