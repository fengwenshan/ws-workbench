//! 代理状态结构、绕过列表匹配与警告文案（平台无关，纯逻辑）。

/// 系统代理的当前状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyState {
    /// 系统代理是否启用（PAC 不在检测范围，见各平台实现）
    pub enabled: bool,
    /// 代理服务器，形如 `127.0.0.1:7897`
    pub servers: Vec<String>,
    /// 绕过条目（已 trim、已拆分）
    pub bypass: Vec<String>,
}

impl ProxyState {
    /// 日志里展示用的代理地址；没有服务器时返回空串。
    pub fn label(&self) -> String {
        self.servers.join("、")
    }
}

/// 绕过列表是否覆盖某个域名。
///
/// Windows 的 `ProxyOverride`（`;` 分隔）与 macOS `scutil` 的 `ExceptionsList`
/// 语义接近，这里按两边都成立的规则判断：
///
/// - `<local>`：主机名里没有点（内网单标签名）；
/// - 含 `*`：通配匹配，`*.risencn.com` 覆盖 `oa.risencn.com` 但**不**覆盖裸域名
///   `risencn.com`（与 WinINET 一致）；
/// - CIDR（`169.254/16`、`10.0.0.0/8`）：只有主机本身就是该网段的 IP 才算覆盖，
///   域名一律不算（这正是现场「IP 能开、域名不行」的由来）；
/// - 其余：主机名相等，或以 `.条目` 结尾（macOS 的单条 `risencn.com` 常被当作
///   后缀语义使用）。
pub fn bypass_covers(bypass: &[String], host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return true;
    }
    let ip = host.parse::<std::net::Ipv4Addr>().ok();
    bypass.iter().any(|raw| {
        let entry = raw.trim().trim_end_matches(';').trim().to_ascii_lowercase();
        if entry.is_empty() {
            return false;
        }
        if entry == "<local>" {
            return !host.contains('.');
        }
        if let Some((network, prefix)) = parse_cidr(&entry) {
            return ip.map(|ip| in_network(ip, network, prefix)).unwrap_or(false);
        }
        if entry.contains('*') {
            return glob_match(&entry, &host);
        }
        host == entry || host.ends_with(&format!(".{}", entry))
    })
}

/// 生成「系统代理会绕开按域名分流」的警告；没有风险时返回 `None`。
///
/// `domains` 传本次会话需要**按域名解析**（走 VPN）的域名。只要有一个不在
/// 绕过列表里，浏览器把请求交给代理后就会用代理自己的 DNS 解析，拿到公网
/// 地址后连不上内网服务。
pub fn warning_for(domains: &[String], state: &ProxyState) -> Option<String> {
    if !state.enabled || domains.is_empty() {
        return None;
    }
    let uncovered: Vec<&str> = domains
        .iter()
        .map(String::as_str)
        .filter(|domain| !bypass_covers(&state.bypass, domain))
        .collect();
    if uncovered.is_empty() {
        return None;
    }
    let list = uncovered.join("、");
    let suggestions = uncovered
        .iter()
        .map(|domain| format!("*.{}", domain))
        .collect::<Vec<_>>()
        .join("、");
    Some(format!(
        "系统代理 {} 会绕开按域名分流：{} 不在代理绕过列表里，\
         浏览器走代理时会用代理自己的 DNS 解析，拿到公网地址后连不上内网服务。\
         把 {} 加进系统代理的绕过列表（或停用系统代理）再重连",
        state.label(),
        list,
        suggestions
    ))
}

/// `169.254/16` / `10.0.0.0/8` 这类条目 → (网络号, 前缀长度)。
fn parse_cidr(entry: &str) -> Option<(u32, u8)> {
    let (addr, prefix) = entry.split_once('/')?;
    let prefix: u8 = prefix.trim().parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let addr: std::net::Ipv4Addr = addr.trim().parse().ok()?;
    Some((u32::from(addr), prefix))
}

fn in_network(ip: std::net::Ipv4Addr, network: u32, prefix: u8) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (u32::from(ip) & mask) == (network & mask)
}

/// 只支持 `*` 的简单通配（绕过列表里只会出现 `*`）。
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    let head = parts[0];
    if !head.is_empty() {
        if !rest.starts_with(head) {
            return false;
        }
        rest = &rest[head.len()..];
    }
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(index) => rest = &rest[index + part.len()..],
            None => return false,
        }
    }
    let tail = parts[parts.len() - 1];
    if tail.is_empty() {
        return true;
    }
    rest.len() >= tail.len() && rest.ends_with(tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn state(bypass: &[&str]) -> ProxyState {
        ProxyState {
            enabled: true,
            servers: vec!["127.0.0.1:7897".to_string()],
            bypass: entries(bypass),
        }
    }

    #[test]
    fn glob_matches_suffix_and_prefix() {
        assert!(glob_match("*.risencn.com", "oa.risencn.com"));
        assert!(!glob_match("*.risencn.com", "risencn.com"));
        assert!(glob_match("192.168.*", "192.168.18.41"));
        assert!(!glob_match("192.168.*", "10.0.0.1"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("*.a.com", "a.com.evil"));
    }

    #[test]
    fn suffix_entry_covers_subdomains() {
        assert!(bypass_covers(&entries(&["risencn.com"]), "oa.risencn.com"));
        assert!(bypass_covers(&entries(&["risencn.com"]), "risencn.com"));
        assert!(!bypass_covers(&entries(&["risencn.com"]), "notrisencn.com"));
    }

    #[test]
    fn cidr_entry_covers_only_ips() {
        let bypass = entries(&["192.168.*", "169.254/16"]);
        assert!(bypass_covers(&bypass, "192.168.18.41"));
        assert!(bypass_covers(&bypass, "169.254.1.1"));
        // 域名不是 IP，网段条目不该被当成覆盖
        assert!(!bypass_covers(&bypass, "oa.risencn.com"));
    }

    #[test]
    fn local_entry_covers_single_label_hosts() {
        let bypass = entries(&["<local>"]);
        assert!(bypass_covers(&bypass, "intranet"));
        assert!(!bypass_covers(&bypass, "oa.risencn.com"));
    }

    #[test]
    fn wildcard_entry_does_not_cover_bare_domain() {
        // WinINET 语义：*.x.com 只匹配子域，所以要连裸域名一起给建议
        assert!(!bypass_covers(&entries(&["*.risencn.com"]), "risencn.com"));
        assert!(bypass_covers(&entries(&["*.risencn.com", "risencn.com"]), "risencn.com"));
    }

    #[test]
    fn blank_and_stray_semicolon_are_ignored() {
        let bypass = entries(&["", " ; ", "localhost"]);
        assert!(!bypass_covers(&bypass, "oa.risencn.com"));
        assert!(bypass_covers(&bypass, "localhost"));
    }

    #[test]
    fn warning_is_none_without_proxy_or_domains() {
        let mut off = state(&[]);
        off.enabled = false;
        assert_eq!(warning_for(&["oa.risencn.com".to_string()], &off), None);
        assert_eq!(warning_for(&[], &state(&[])), None);
    }

    #[test]
    fn warning_names_uncovered_domains() {
        let message = warning_for(
            &["oa.risencn.com".to_string(), "cas.risencn.com".to_string()],
            &state(&["192.168.*"]),
        )
        .expect("系统代理开着、域名没在绕过列表里，应当报警");
        assert!(message.contains("127.0.0.1:7897"), "{}", message);
        assert!(message.contains("oa.risencn.com"), "{}", message);
        assert!(message.contains("*.risencn.com"), "{}", message);
    }

    #[test]
    fn warning_is_none_when_all_domains_bypassed() {
        let message = warning_for(
            &["oa.risencn.com".to_string()],
            &state(&["*.risencn.com", "risencn.com"]),
        );
        assert_eq!(message, None);
    }
}
