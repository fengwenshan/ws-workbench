//! fake-ip DNS 假地址池（纯逻辑）
//!
//! DNS 应答时给域名分配一个池内假 IP，并建立 `假IP -> 域名` 与 `域名 -> 假IP`
//! 双向映射；连接到达 TUN 时用假 IP 反查域名，交给规则引擎做域名匹配。
//!
//! 池内地址复用 `route.rs` 的纯 IPv4 工具。`fake_ip_filter` 命中的域名不做伪造，
//! 由上层走真解析（本地服务 / 特殊应用的兼容性关键）。
//!
//! 本模块不读时钟、不碰文件：所有与时间相关的方法都显式接收 `now`（秒），
//! 便于确定性地单测。

use std::collections::HashMap;

use super::route;

/// 一条 `fake_ip_filter` 规则：`*.example.com`（后缀）或 `keyword`（包含）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum FilterRule {
    /// 形如 `*.example.com`：命中 `example.com` 及其子域。
    Suffix(String),
    /// 其它形式：按「域名包含该串」匹配。
    Keyword(String),
}

impl FilterRule {
    fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_ascii_lowercase();
        if t.is_empty() {
            return None;
        }
        if let Some(rest) = t.strip_prefix("*.") {
            let rest = rest.trim_start_matches('.');
            if rest.is_empty() {
                return None;
            }
            Some(FilterRule::Suffix(rest.to_string()))
        } else {
            Some(FilterRule::Keyword(t))
        }
    }

    fn matches(&self, domain: &str) -> bool {
        let domain = domain.to_ascii_lowercase();
        match self {
            FilterRule::Suffix(suffix) => {
                domain == *suffix || domain.ends_with(&format!(".{}", suffix))
            }
            FilterRule::Keyword(keyword) => domain.contains(keyword.as_str()),
        }
    }
}

/// 池中一条映射。
#[derive(Debug, Clone)]
struct Entry {
    /// 归属域名（小写）。
    domain: String,
    /// 过期时刻（秒，`now` 语义）。
    expires_at: u64,
    /// 最近访问时刻（秒），用于 LRU 淘汰。
    last_access: u64,
}

/// 假 IP 池。
#[derive(Debug, Clone)]
pub struct FakeIpPool {
    /// 网络地址（主机序 u32）。
    network: u32,
    /// 前缀长度。
    prefix: u8,
    /// 可用地址数（已扣除网络地址与广播地址）。
    capacity: u32,
    filters: Vec<FilterRule>,
    /// 假 IP（主机序 u32）-> 条目。
    by_ip: HashMap<u32, Entry>,
    /// 域名（小写）-> 假 IP。
    by_domain: HashMap<String, u32>,
    /// 分配游标（相对 `network` 的偏移），加速寻找空位。
    cursor: u32,
}

impl FakeIpPool {
    /// 用 `fake_ip_range`（CIDR）与 `fake_ip_filter` 构造池。
    ///
    /// 前缀长度需在 `8..=30`（过小会得到非 IPv4 可用范围，过大则没有可用地址）。
    pub fn new(range: &str, filter: &[String]) -> Result<Self, String> {
        let (network, prefix) = route::parse_cidr(range)?;
        if !(8..=30).contains(&prefix) {
            return Err(format!(
                "fake_ip_range {} 前缀长度需在 8~30 之间",
                range
            ));
        }
        let total: u32 = 1u32 << (32 - prefix);
        let capacity = total - 2; // 跳过网络地址与广播地址
        let filters = filter.iter().filter_map(|s| FilterRule::parse(s)).collect();
        Ok(FakeIpPool {
            network,
            prefix,
            capacity,
            filters,
            by_ip: HashMap::new(),
            by_domain: HashMap::new(),
            cursor: 1,
        })
    }

    /// 池容量（可用地址数）。
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// 当前条目数。
    pub fn len(&self) -> usize {
        self.by_ip.len()
    }

    /// 池是否为空。
    pub fn is_empty(&self) -> bool {
        self.by_ip.is_empty()
    }

    /// 该地址是否落在本池范围内。
    pub fn contains(&self, ip: u32) -> bool {
        let mask = route::prefix_to_mask(self.prefix);
        (ip & mask) == self.network
    }

    /// 域名是否命中 `fake_ip_filter`（命中则不伪造，应走真解析）。
    pub fn is_filtered(&self, domain: &str) -> bool {
        self.filters.iter().any(|f| f.matches(domain))
    }

    /// 为域名分配假 IP。
    ///
    /// - 命中 `fake_ip_filter` 时返回 `None`（上层应改走真解析）。
    /// - 域名已分配过则返回原地址，并刷新 TTL 与最近访问时间。
    /// - 池满时淘汰最近最少访问（LRU）的一条。
    pub fn allocate(&mut self, domain: &str, now: u64, ttl: u64) -> Option<u32> {
        if self.is_filtered(domain) {
            return None;
        }
        let key = domain.to_ascii_lowercase();
        if let Some(&ip) = self.by_domain.get(&key) {
            if let Some(entry) = self.by_ip.get_mut(&ip) {
                entry.expires_at = now.saturating_add(ttl);
                entry.last_access = now;
            }
            return Some(ip);
        }
        let ip = self.pick_slot()?;
        self.by_ip.insert(
            ip,
            Entry {
                domain: key.clone(),
                expires_at: now.saturating_add(ttl),
                last_access: now,
            },
        );
        self.by_domain.insert(key, ip);
        Some(ip)
    }

    /// 反查假 IP 对应的域名。
    pub fn lookup_domain(&self, fake_ip: u32) -> Option<&str> {
        self.by_ip.get(&fake_ip).map(|e| e.domain.as_str())
    }

    /// 反查域名对应的假 IP。
    pub fn lookup_ip(&self, domain: &str) -> Option<u32> {
        self.by_domain.get(&domain.to_ascii_lowercase()).copied()
    }

    /// 取一个空位；池满则淘汰 LRU 条目后复用其地址。
    fn pick_slot(&mut self) -> Option<u32> {
        let last_usable = self.capacity; // 偏移 1..=capacity
        let mut offset = self.cursor;
        let mut scanned = 0u32;
        while scanned < last_usable {
            if offset > last_usable {
                offset = 1;
            }
            let candidate = self.network.wrapping_add(offset);
            if !self.by_ip.contains_key(&candidate) {
                self.cursor = offset + 1;
                return Some(candidate);
            }
            offset += 1;
            scanned += 1;
        }
        // 池满：淘汰最近最少访问的一条，复用其地址
        let victim = self
            .by_ip
            .iter()
            .min_by_key(|(_, e)| e.last_access)
            .map(|(ip, _)| *ip)?;
        let entry = self.by_ip.remove(&victim)?;
        self.by_domain.remove(&entry.domain);
        self.cursor = victim.wrapping_sub(self.network).wrapping_add(1);
        Some(victim)
    }

    /// 回收已过期条目，返回回收数量。
    pub fn reclaim_expired(&mut self, now: u64) -> usize {
        let expired: Vec<u32> = self
            .by_ip
            .iter()
            .filter(|(_, e)| e.expires_at <= now)
            .map(|(ip, _)| *ip)
            .collect();
        for ip in &expired {
            if let Some(entry) = self.by_ip.remove(ip) {
                self.by_domain.remove(&entry.domain);
            }
        }
        expired.len()
    }

    /// 断开重连时清空整池。
    pub fn clear(&mut self) {
        self.by_ip.clear();
        self.by_domain.clear();
        self.cursor = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_distinct_addresses_and_maps_both_ways() {
        let mut pool = FakeIpPool::new("198.18.0.0/24", &[]).unwrap();
        let a = pool.allocate("a.com", 0, 60).unwrap();
        let b = pool.allocate("b.com", 0, 60).unwrap();
        assert_ne!(a, b);
        assert_eq!(pool.lookup_domain(a), Some("a.com"));
        assert_eq!(pool.lookup_ip("b.com"), Some(b));
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn same_domain_reuses_address() {
        let mut pool = FakeIpPool::new("198.18.0.0/24", &[]).unwrap();
        let a = pool.allocate("a.com", 0, 60).unwrap();
        let again = pool.allocate("a.com", 10, 60).unwrap();
        assert_eq!(a, again);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn allocated_address_is_inside_range() {
        let mut pool = FakeIpPool::new("198.18.0.0/24", &[]).unwrap();
        let a = pool.allocate("a.com", 0, 60).unwrap();
        assert!(pool.contains(a));
    }

    #[test]
    fn filtered_domains_are_not_faked() {
        let filter = vec!["*.lan".to_string(), "time.windows.com".to_string()];
        let mut pool = FakeIpPool::new("198.18.0.0/24", &filter).unwrap();
        assert!(pool.is_filtered("router.lan"));
        assert!(pool.is_filtered("lan"));
        assert!(pool.is_filtered("foo.time.windows.com"));
        assert!(!pool.is_filtered("example.com"));
        assert_eq!(pool.allocate("router.lan", 0, 60), None);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn rejects_invalid_range() {
        assert!(FakeIpPool::new("198.18.0.0/31", &[]).is_err());
        assert!(FakeIpPool::new("not-a-cidr", &[]).is_err());
    }

    #[test]
    fn reclaim_expired_removes_entries() {
        let mut pool = FakeIpPool::new("198.18.0.0/24", &[]).unwrap();
        let a = pool.allocate("a.com", 0, 10).unwrap();
        pool.allocate("b.com", 0, 100).unwrap();
        assert_eq!(pool.reclaim_expired(10), 1);
        assert_eq!(pool.lookup_domain(a), None);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn evicts_least_recently_used_when_full() {
        let mut pool = FakeIpPool::new("198.18.0.0/30", &[]).unwrap(); // 2 个可用地址
        assert_eq!(pool.capacity(), 2);
        let a = pool.allocate("a.com", 1, 100).unwrap();
        let b = pool.allocate("b.com", 2, 100).unwrap();
        // 刷新 a 的最近访问时间（3 > 2）
        let _ = pool.allocate("a.com", 3, 100).unwrap();
        // 池满：分配 c 应淘汰最近最少访问的 b
        let c = pool.allocate("c.com", 4, 100).unwrap();
        assert_eq!(c, b);
        assert_eq!(pool.lookup_domain(a), Some("a.com"));
        assert_eq!(pool.lookup_domain(b), Some("c.com"));
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn clear_empties_pool() {
        let mut pool = FakeIpPool::new("198.18.0.0/24", &[]).unwrap();
        pool.allocate("a.com", 0, 60).unwrap();
        pool.clear();
        assert!(pool.is_empty());
        assert_eq!(pool.lookup_ip("a.com"), None);
    }
}
