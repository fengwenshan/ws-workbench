//! 本机网络接口信息（MAC / IP），给设置页的「运行环境」展示用。
//!
//! 取值方式按平台分两套：
//! - Windows：PowerShell 的 `Get-NetAdapter` / `Get-NetIPAddress` 输出 JSON。
//!   沿用项目里的统一约定 —— 不解析本地化的命令行文本，只读 CIM / 对象里的英文属性名
//! - macOS / Linux：`getifaddrs`（nix 封装，项目已有依赖），一次遍历同时拿到地址、
//!   子网掩码以及 AF_LINK / AF_PACKET 里的 MAC
//!
//! 输出前统一过滤：回环、IPv6 链路本地（fe80::/10）和其他不可用地址不返回；
//! 只有 MAC、没有可用 IP 的虚拟网卡（macOS 的 awdl0 / llw0、Windows 的 WSL 虚拟网卡）
//! 也不返回 —— 设置页是给人查「本机 IP / MAC」的，列一堆虚拟接口只会干扰。
//! 例外是物理网卡：即使当前没拿到 IP，也保留一行（只显示 MAC），
//! 因为网线没插时用户常常正需要这个 MAC。

use std::net::{IpAddr, Ipv6Addr};
#[cfg(any(unix, test))]
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// 一个 IP 地址（含前缀长度，展示成 `192.168.1.5/24` 这种形式）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkAddress {
    pub ip: String,
    /// `IPv4` / `IPv6`
    pub family: String,
    /// 前缀长度；取不到时为 None
    #[serde(default)]
    pub prefix: Option<u8>,
}

/// 一块网卡及其地址
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub name: String,
    /// 大写、冒号分隔（`94:83:C4:00:00:00`）；utun / TUN 这类没有 MAC 的网卡为 None
    #[serde(default)]
    pub mac: Option<String>,
    pub addresses: Vec<NetworkAddress>,
}

/// 设置页「运行环境 → 网络」的数据
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub interfaces: Vec<NetworkInterface>,
}

impl NetworkInfo {
    pub fn current() -> Result<Self, String> {
        let mut interfaces = platform_interfaces()?;
        normalize(&mut interfaces);
        Ok(Self { interfaces })
    }
}

/// 统一收尾：MAC 规范化、地址过滤/排序/去重、接口名排序
fn normalize(interfaces: &mut Vec<NetworkInterface>) {
    for iface in interfaces.iter_mut() {
        iface.mac = iface.mac.as_deref().and_then(normalize_mac);
        iface.addresses.retain(is_usable_address);
        iface.addresses.sort_by(|a, b| {
            (family_rank(&a.family), &a.ip).cmp(&(family_rank(&b.family), &b.ip))
        });
        iface.addresses.dedup_by(|a, b| a.family == b.family && a.ip == b.ip);
    }
    // 既没有地址、也没有 MAC 的接口没有展示价值
    interfaces.retain(|iface| !iface.addresses.is_empty() || iface.mac.is_some());
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
}

/// IPv4 排在 IPv6 前面
fn family_rank(family: &str) -> u8 {
    if family.eq_ignore_ascii_case("ipv4") {
        0
    } else {
        1
    }
}

/// MAC 统一成大写、冒号分隔；`00:00:00:00:00:00` 和格式不对的一律当作没有
fn normalize_mac(raw: &str) -> Option<String> {
    let parts: Vec<String> = raw
        .split([':', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| part.trim().to_ascii_uppercase())
        .collect();
    if parts.len() != 6 || parts.iter().any(|part| part.len() != 2 || !part.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    let joined = parts.join(":");
    if joined == "00:00:00:00:00:00" {
        None
    } else {
        Some(joined)
    }
}

/// 判断一个地址值不值得展示
fn is_usable_address(address: &NetworkAddress) -> bool {
    let Ok(ip) = address.ip.parse::<IpAddr>() else {
        return false;
    };
    match ip {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_multicast() || v4.is_unspecified()),
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || is_ipv6_link_local(&v6))
        }
    }
}

/// fe80::/10：链路本地地址只在同一段链路上有意义，对用户没有参考价值
fn is_ipv6_link_local(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(any(unix, test))]
fn ipv4_prefix(mask: Ipv4Addr) -> u8 {
    u32::from(mask).count_ones() as u8
}

#[cfg(any(unix, test))]
fn ipv6_prefix(mask: Ipv6Addr) -> u8 {
    u128::from(mask).count_ones() as u8
}

#[cfg(any(unix, test))]
fn format_mac(bytes: &[u8; 6]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{:02X}", byte))
        .collect::<Vec<_>>()
        .join(":")
}

/// 物理网卡名称前缀。macOS 是 en0 / en1，Linux 是 eth0 / wlan0 / wlp2s0 / enp3s0 这类。
/// 仅用于「没有可用 IP 时要不要保留这行」的判断，不影响有 IP 的接口。
#[cfg(any(unix, test))]
fn looks_physical(name: &str) -> bool {
    const PREFIXES: &[&str] = &["en", "eth", "em", "wl", "wlan", "enp", "eno", "ens", "wlp"];
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// MAC 的第一个字节里 0x02 是「本地管理」位：置位说明这个地址是软件生成的
/// （虚拟机网卡、macOS 的 llw0 / awdl0、私有 Wi-Fi 地址）。
/// 这类网卡没有可用 IP 时只显示 MAC 反而会干扰，所以不当成物理网卡保留。
#[cfg(any(unix, test))]
fn is_globally_administered_mac(mac: &str) -> bool {
    let first = mac.split(':').next().unwrap_or_default();
    u8::from_str_radix(first, 16)
        .map(|byte| byte & 0x02 == 0)
        .unwrap_or(false)
}

// ============================== macOS / Linux ==============================

#[cfg(unix)]
fn platform_interfaces() -> Result<Vec<NetworkInterface>, String> {
    use std::collections::BTreeMap;

    use nix::ifaddrs::getifaddrs;
    use nix::net::if_::InterfaceFlags;
    use nix::sys::socket::{AddressFamily, SockaddrLike};

    /// 遍历过程中按网卡名合并多条 getifaddrs 记录
    struct Acc {
        up: bool,
        loopback: bool,
        mac: Option<String>,
        addresses: Vec<NetworkAddress>,
    }

    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    for ifaddr in getifaddrs().map_err(|err| format!("读取网卡信息失败: {}", err))? {
        let entry = acc.entry(ifaddr.interface_name.clone()).or_insert_with(|| Acc {
            up: false,
            loopback: false,
            mac: None,
            addresses: Vec::new(),
        });
        if ifaddr.flags.contains(InterfaceFlags::IFF_UP) {
            entry.up = true;
        }
        if ifaddr.flags.contains(InterfaceFlags::IFF_LOOPBACK) {
            entry.loopback = true;
        }

        let Some(address) = ifaddr.address.as_ref() else {
            continue;
        };
        match address.family() {
            Some(AddressFamily::Inet) => {
                let Some(sin) = address.as_sockaddr_in() else {
                    continue;
                };
                let prefix = ifaddr
                    .netmask
                    .as_ref()
                    .and_then(|mask| mask.as_sockaddr_in())
                    .map(|mask| ipv4_prefix(mask.ip()));
                entry.addresses.push(NetworkAddress {
                    ip: sin.ip().to_string(),
                    family: "IPv4".to_string(),
                    prefix,
                });
            }
            Some(AddressFamily::Inet6) => {
                let Some(sin6) = address.as_sockaddr_in6() else {
                    continue;
                };
                let prefix = ifaddr
                    .netmask
                    .as_ref()
                    .and_then(|mask| mask.as_sockaddr_in6())
                    .map(|mask| ipv6_prefix(mask.ip()));
                entry.addresses.push(NetworkAddress {
                    ip: sin6.ip().to_string(),
                    family: "IPv6".to_string(),
                    prefix,
                });
            }
            // 链路层记录里才有 MAC：macOS 是 AF_LINK，Linux 是 AF_PACKET
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Some(family) if is_link_family(family) => {
                if let Some(link) = address.as_link_addr() {
                    if let Some(bytes) = link.addr() {
                        entry.mac = Some(format_mac(&bytes));
                    }
                }
            }
            _ => {}
        }
    }

    let interfaces = acc
        .into_iter()
        .filter_map(|(name, mut entry)| {
            entry.addresses.retain(is_usable_address);
            if entry.loopback || !entry.up {
                return None;
            }
            // 有可用 IP 就展示；物理网卡（且 MAC 不是软件随机出来的）即使没 IP
            // 也留一行，方便查 MAC
            let physical = entry
                .mac
                .as_deref()
                .map(|mac| looks_physical(&name) && is_globally_administered_mac(mac))
                .unwrap_or(false);
            if entry.addresses.is_empty() && !physical {
                return None;
            }
            Some(NetworkInterface {
                name,
                mac: entry.mac,
                addresses: entry.addresses,
            })
        })
        .collect();
    Ok(interfaces)
}

#[cfg(target_os = "macos")]
fn is_link_family(family: nix::sys::socket::AddressFamily) -> bool {
    family == nix::sys::socket::AddressFamily::Link
}

#[cfg(target_os = "linux")]
fn is_link_family(family: nix::sys::socket::AddressFamily) -> bool {
    family == nix::sys::socket::AddressFamily::Packet
}

// ============================== Windows ==============================

/// 取「已启用且有地址」的网卡及其全部 IP。
///
/// 用结构化 JSON 而不是 `ipconfig` 文本：后者在中文系统上字段名会变，
/// 而且一个网卡多 IP 时分不清哪行属于哪块网卡。
#[cfg(target_os = "windows")]
const WINDOWS_SCRIPT: &str = r#"
$adapters = @(Get-NetAdapter -ErrorAction Stop | Where-Object { $_.Status -eq 'Up' })
$out = foreach ($adapter in $adapters) {
    $addrs = @(Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -ErrorAction SilentlyContinue | Where-Object {
        $family = "$($_.AddressFamily)"
        $family -eq 'IPv4' -or ($family -eq 'IPv6' -and $_.IPAddress -notlike 'fe80:*' -and $_.IPAddress -ne '::1')
    } | ForEach-Object {
        [pscustomobject]@{
            ip = $_.IPAddress
            family = "$($_.AddressFamily)"
            prefix = [int]$_.PrefixLength
        }
    })
    if ($addrs.Count -eq 0 -and -not $adapter.HardwareInterface) { continue }
    [pscustomobject]@{
        name = $adapter.Name
        mac = $adapter.MacAddress
        hardware = [bool]$adapter.HardwareInterface
        addresses = @($addrs)
    }
}
ConvertTo-Json -InputObject @($out) -Depth 5 -Compress
"#;

#[cfg(target_os = "windows")]
fn platform_interfaces() -> Result<Vec<NetworkInterface>, String> {
    let value = crate::winps::run_json(WINDOWS_SCRIPT)?;
    Ok(from_windows_json(value))
}

/// PowerShell 输出的一条网卡记录
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default, Deserialize)]
struct WindowsAdapter {
    #[serde(default, deserialize_with = "crate::winps::null_as_default")]
    name: String,
    #[serde(default, deserialize_with = "crate::winps::null_as_default")]
    mac: String,
    #[serde(default)]
    hardware: bool,
    #[serde(default)]
    addresses: serde_json::Value,
}

/// PowerShell 输出的一条地址记录
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default, Deserialize)]
struct WindowsAddress {
    #[serde(default, deserialize_with = "crate::winps::null_as_default")]
    ip: String,
    #[serde(default, deserialize_with = "crate::winps::null_as_default")]
    family: String,
    #[serde(default)]
    prefix: Option<u8>,
}

/// `ConvertTo-Json` 在只有一条记录时会输出对象而不是数组，
/// 这里统一经 `parse_list` 处理；硬件网卡即使暂时没有 IP 也保留（便于查 MAC）。
#[cfg(any(target_os = "windows", test))]
fn from_windows_json(value: serde_json::Value) -> Vec<NetworkInterface> {
    crate::winps::parse_list::<WindowsAdapter>(value)
        .into_iter()
        .filter_map(|adapter| {
            let addresses: Vec<NetworkAddress> =
                crate::winps::parse_list::<WindowsAddress>(adapter.addresses)
                    .into_iter()
                    .map(|address| NetworkAddress {
                        ip: address.ip,
                        family: normalize_family(&address.family),
                        prefix: address.prefix,
                    })
                    .collect();
            let mac = normalize_mac(&adapter.mac);
            if addresses.is_empty() && !(adapter.hardware && mac.is_some()) {
                return None;
            }
            Some(NetworkInterface {
                name: adapter.name,
                mac,
                addresses,
            })
        })
        .collect()
}

#[cfg(any(target_os = "windows", test))]
fn normalize_family(family: &str) -> String {
    if family.eq_ignore_ascii_case("ipv4") {
        "IPv4".to_string()
    } else if family.eq_ignore_ascii_case("ipv6") {
        "IPv6".to_string()
    } else {
        family.to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn macs_are_normalized_to_upper_colon_form() {
        assert_eq!(normalize_mac("aa-bb-cc-dd-ee-ff").as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(normalize_mac("AA:bb:CC:dd:EE:ff").as_deref(), Some("AA:BB:CC:DD:EE:FF"));
    }

    #[test]
    fn only_globally_administered_macs_count_as_physical() {
        // 真实网卡：第一个字节 0x94 / 0x3C
        assert!(is_globally_administered_mac("94:83:C4:00:00:00"));
        assert!(is_globally_administered_mac("3C:22:FB:00:00:00"));
        // 本地管理位（0x02）置位：虚拟网卡、私有 Wi-Fi 地址
        assert!(!is_globally_administered_mac("02:12:5D:7D:D1:A3"));
        assert!(!is_globally_administered_mac("96:FA:02:34:13:4D"));
        assert!(!is_globally_administered_mac(""));
    }

    #[test]
    fn invalid_and_zero_macs_are_dropped() {
        assert_eq!(normalize_mac(""), None);
        assert_eq!(normalize_mac("00:00:00:00:00:00"), None);
        assert_eq!(normalize_mac("00-00-00-00-00-00"), None);
        assert_eq!(normalize_mac("aa:bb:cc:dd:ee"), None);
        assert_eq!(normalize_mac("zz:bb:cc:dd:ee:ff"), None);
    }

    #[test]
    fn unusable_addresses_are_filtered() {
        assert!(!is_usable_address(&addr("127.0.0.1", "IPv4")));
        assert!(!is_usable_address(&addr("::1", "IPv6")));
        assert!(!is_usable_address(&addr("fe80::1", "IPv6")));
        assert!(!is_usable_address(&addr("ff02::1", "IPv6")));
        assert!(!is_usable_address(&addr("not-an-ip", "IPv4")));
        assert!(!is_usable_address(&addr("0.0.0.0", "IPv4")));

        assert!(is_usable_address(&addr("192.168.1.5", "IPv4")));
        assert!(is_usable_address(&addr("2408:8207:1234::1", "IPv6")));
    }

    #[test]
    fn normalize_sorts_ipv4_first_and_removes_duplicates() {
        let mut interfaces = vec![NetworkInterface {
            name: "en0".to_string(),
            mac: Some("aa-bb-cc-dd-ee-ff".to_string()),
            addresses: vec![
                addr("fe80::1", "IPv6"),
                addr("2408:8207::1", "IPv6"),
                addr("192.168.1.5", "IPv4"),
                addr("192.168.1.5", "IPv4"),
            ],
        }];
        normalize(&mut interfaces);
        let iface = &interfaces[0];
        assert_eq!(iface.mac.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        let ips: Vec<&str> = iface.addresses.iter().map(|a| a.ip.as_str()).collect();
        assert_eq!(ips, vec!["192.168.1.5", "2408:8207::1"]);
        assert_eq!(iface.addresses[0].family, "IPv4");
        assert_eq!(iface.addresses[1].family, "IPv6");
    }

    #[test]
    fn empty_interfaces_are_dropped() {
        let mut interfaces = vec![
            NetworkInterface {
                name: "empty0".to_string(),
                mac: None,
                addresses: vec![addr("127.0.0.1", "IPv4")],
            },
            NetworkInterface {
                name: "en0".to_string(),
                mac: Some("AA:BB:CC:DD:EE:FF".to_string()),
                addresses: vec![],
            },
        ];
        normalize(&mut interfaces);
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].name, "en0");
    }

    #[test]
    fn windows_json_single_object_becomes_one_interface() {
        let value = json!({
            "name": "WLAN",
            "mac": "AA-BB-CC-DD-EE-FF",
            "hardware": true,
            "addresses": {
                "ip": "192.168.1.5",
                "family": "IPv4",
                "prefix": 24
            }
        });
        let mut interfaces = from_windows_json(value);
        normalize(&mut interfaces);
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].name, "WLAN");
        assert_eq!(interfaces[0].mac.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(interfaces[0].addresses.len(), 1);
        assert_eq!(interfaces[0].addresses[0].ip, "192.168.1.5");
        assert_eq!(interfaces[0].addresses[0].prefix, Some(24));
    }

    #[test]
    fn windows_virtual_adapter_without_addresses_is_dropped() {
        let value = json!([
            {
                "name": "vEthernet (WSL)",
                "mac": "00-15-5D-00-00-01",
                "hardware": false,
                "addresses": []
            },
            {
                "name": "以太网",
                "mac": "AA-BB-CC-DD-EE-FF",
                "hardware": true,
                "addresses": []
            }
        ]);
        let interfaces = from_windows_json(value);
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].name, "以太网");
        assert!(interfaces[0].addresses.is_empty());
    }

    #[test]
    fn current_never_returns_loopback_or_link_local() {
        let info = NetworkInfo::current().expect("读取本机网络信息");
        for iface in &info.interfaces {
            assert!(!iface.name.is_empty());
            for address in &iface.addresses {
                assert!(!address.ip.starts_with("127."));
                assert!(!address.ip.starts_with("fe80:"));
                assert!(!address.ip.contains("::1"));
            }
        }
    }

    fn addr(ip: &str, family: &str) -> NetworkAddress {
        NetworkAddress {
            ip: ip.to_string(),
            family: family.to_string(),
            prefix: None,
        }
    }
}
