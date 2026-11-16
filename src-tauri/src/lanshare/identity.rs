//! 设备身份：主物理网卡的 MAC 地址。
//!
//! 设计文档：`docs/superpowers/specs/2026-11-16-lan-share-chat-redesign.md` §3。
//!
//! 为什么用 MAC：随机 UUID 会随重装 / 清配置变化，IP 又会随 DHCP / VPN 变化，
//! 物理网卡地址是局域网内最稳定的「这台机器是谁」的标识。取不到时调用方回退
//! 到持久化指纹（旧 UUID），保证身份不会变成空串。
//!
//! 只做纯函数挑选，网络信息仍由 [`crate::netinfo::NetworkInfo`] 提供，方便单测
//! 用假网卡列表覆盖各种平台命名。

use crate::netinfo::NetworkInterface;

/// 虚拟 / 隧道 / 容器网卡名字前缀或片段：这些网卡的 MAC 不是物理网卡地址，
/// 而且会随 VPN、虚拟机、Docker 启停变化，不能用来认机器。
const VIRTUAL_PREFIXES: &[&str] = &[
    "utun", "tun", "tap", "awdl", "llw", "bridge", "vmnet", "veth", "docker", "br-", "tailscale",
    "zerotier", "wsl", "vethernet", "hyper-v", "virtualbox", "loopback", "lo0", "lo",
];

/// 常见物理网卡名（越靠前越优先）：macOS `en0`、Windows「以太网 / WLAN」、Linux `eth0` / `wlan0`。
const PHYSICAL_PREFIXES: &[&str] = &["en", "eth", "wlan", "wl", "enp", "eno", "ens", "em"];

/// 当前机器的主物理网卡 MAC（大写冒号分隔）。读不到网卡 / 全是虚拟网卡时返回 None。
pub(crate) fn primary_mac() -> Option<String> {
    let info = crate::netinfo::NetworkInfo::current().ok()?;
    pick_primary_mac(&info.interfaces)
}

/// 从网卡列表里挑主物理网卡 MAC（纯函数，便于测试）。
///
/// 规则：
/// 1. 先过滤虚拟网卡、没有 MAC 的网卡、以及非正常 MAC；
/// 2. 有 IPv4 地址的优先（没 IP 的网卡可能是没插网线的备用口）；
/// 3. 再按常见物理网卡名 / 接口名排序，保证同一台机器每次挑到同一块网卡。
pub(crate) fn pick_primary_mac(interfaces: &[NetworkInterface]) -> Option<String> {
    interfaces
        .iter()
        .filter_map(|interface| {
            if is_virtual_interface(&interface.name) {
                return None;
            }
            let mac = normalize_mac(interface.mac.as_deref()?)?;
            let has_ipv4 = interface
                .addresses
                .iter()
                .any(|address| address.family.eq_ignore_ascii_case("ipv4"));
            let name_rank = physical_name_rank(&interface.name);
            Some(((if has_ipv4 { 0 } else { 1 }, name_rank, interface.name.clone()), mac))
        })
        .min_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, mac)| mac)
}

/// 规范化 MAC：接受 `aa:bb:cc:dd:ee:ff` / `AA-BB-...` / 无分隔 12 位十六进制，
/// 统一成大写冒号分隔；全 0 或长度不对返回 None。
pub(crate) fn normalize_mac(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let compact: String = trimmed
        .chars()
        .filter(|ch| *ch != ':' && *ch != '-' && *ch != '.')
        .collect();
    if compact.len() != 12 || !compact.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let compact = compact.to_ascii_uppercase();
    if compact == "000000000000" {
        return None;
    }
    let bytes: Vec<&str> = (0..6).map(|index| &compact[index * 2..index * 2 + 2]).collect();
    Some(bytes.join(":"))
}

fn is_virtual_interface(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return true;
    }
    VIRTUAL_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix) || name.contains(prefix))
}

/// 物理网卡名排序权重：越小越优先
fn physical_name_rank(name: &str) -> usize {
    let name = name.trim().to_ascii_lowercase();
    if name == "en0" || name == "eth0" || name == "wlan0" {
        return 0;
    }
    if PHYSICAL_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
        return 1;
    }
    if name.contains("以太网") || name.contains("wi-fi") || name.contains("wifi") || name.contains("wlan")
    {
        return 2;
    }
    3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netinfo::NetworkAddress;

    fn iface(name: &str, mac: Option<&str>, ipv4: Option<&str>) -> NetworkInterface {
        NetworkInterface {
            name: name.to_string(),
            mac: mac.map(str::to_string),
            addresses: ipv4
                .map(|ip| {
                    vec![NetworkAddress {
                        ip: ip.to_string(),
                        family: "IPv4".to_string(),
                        prefix: Some(24),
                    }]
                })
                .unwrap_or_default(),
        }
    }

    #[test]
    fn normalize_accepts_common_formats() {
        assert_eq!(
            normalize_mac("a4:83:e7:12:34:56").as_deref(),
            Some("A4:83:E7:12:34:56")
        );
        assert_eq!(
            normalize_mac("A4-83-E7-12-34-56").as_deref(),
            Some("A4:83:E7:12:34:56")
        );
        assert_eq!(
            normalize_mac("a483e7123456").as_deref(),
            Some("A4:83:E7:12:34:56")
        );
        assert_eq!(normalize_mac("00:00:00:00:00:00"), None);
        assert_eq!(normalize_mac("000000000000"), None);
        assert_eq!(normalize_mac(""), None);
        assert_eq!(normalize_mac("not-a-mac"), None);
        assert_eq!(normalize_mac("a4:83:e7:12:34"), None);
    }

    #[test]
    fn picks_physical_interface_with_ipv4() {
        let interfaces = vec![
            iface("utun3", Some("AA:BB:CC:DD:EE:01"), Some("10.8.0.6")),
            iface("awdl0", Some("AA:BB:CC:DD:EE:02"), None),
            iface("en1", Some("AA:BB:CC:DD:EE:03"), None),
            iface("en0", Some("A4:83:E7:12:34:56"), Some("192.168.1.110")),
        ];
        assert_eq!(pick_primary_mac(&interfaces).as_deref(), Some("A4:83:E7:12:34:56"));
    }

    #[test]
    fn prefers_known_physical_name_when_several_have_ip() {
        let interfaces = vec![
            iface("en7", Some("AA:BB:CC:DD:EE:07"), Some("192.168.1.7")),
            iface("en0", Some("AA:BB:CC:DD:EE:00"), Some("192.168.1.10")),
        ];
        assert_eq!(pick_primary_mac(&interfaces).as_deref(), Some("AA:BB:CC:DD:EE:00"));
    }

    #[test]
    fn falls_back_to_physical_without_ip() {
        let interfaces = vec![
            iface("docker0", Some("02:42:AC:11:00:02"), Some("172.17.0.1")),
            iface("en0", Some("A4:83:E7:12:34:56"), None),
        ];
        assert_eq!(pick_primary_mac(&interfaces).as_deref(), Some("A4:83:E7:12:34:56"));
    }

    #[test]
    fn returns_none_without_physical_mac() {
        let interfaces = vec![
            iface("utun0", Some("AA:BB:CC:DD:EE:01"), None),
            iface("llw0", Some("AA:BB:CC:DD:EE:02"), None),
            iface("lo0", None, Some("127.0.0.1")),
        ];
        assert_eq!(pick_primary_mac(&interfaces), None);
        assert_eq!(pick_primary_mac(&[]), None);
    }
}
