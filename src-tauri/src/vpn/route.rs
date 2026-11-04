//! 路由表操作
//!
//! 设计要点：
//! - Windows 侧一律通过 PowerShell 获取 **JSON** 结果。中文系统下 `route print` 和
//!   `netsh` 的输出是本地化的，直接解析文本会在不同机器上失效；CIM 对象的属性名不变。
//! - 所有临时路由都显式写入 `ActiveStore`，绝不落进持久化路由表。
//!   （`New-NetRoute` 默认同时写 Active + Persistent，会污染注册表）
//! - 分流依赖 Windows 的「最长前缀优先」规则：/24 永远胜过 VPN 下发的 /0，
//!   因此直连例外只需添加更具体的路由。
//! - macOS 没有「接口索引」概念，用接口名（utunN / en0），所以统一抽象成 `RouteTarget`。

use serde::Deserialize;

/// macOS 上 `route` 命令在 `/sbin/route`，**不在** `/usr/sbin/route`。
///
/// 现场教训：路径写成 `/usr/sbin/route` 时进程根本起不来（ENOENT），
/// 会同时表现成两条互相掩盖的症状——
/// 1. 读不到默认路由，快照里出现「未找到可用的物理默认路由」；
/// 2. 下发分流路由时报 `route -n add ... 失败: 启动失败: No such file or directory`。
#[cfg(target_os = "macos")]
const MACOS_ROUTE_BIN: &str = "/sbin/route";

// ============================== 数据结构 ==============================
//
// 下面这些结构体都从 PowerShell 的 JSON 反序列化而来。字段一律带上
// `null_as_default`：PowerShell 会为不存在的属性输出显式 `null`，
// 而 `#[serde(default)]` 只兜得住「字段缺失」，兜不住 null，
// 不处理的话整条记录会被静默丢掉。

/// 网卡信息
#[derive(Debug, Clone, Deserialize, Default)]
pub struct InterfaceInfo {
    #[serde(rename = "IPAddress", default, deserialize_with = "crate::winps::null_as_default")]
    pub ip: String,
    #[serde(rename = "PrefixLength", default, deserialize_with = "crate::winps::null_as_default")]
    pub prefix_len: u8,
    #[serde(rename = "InterfaceIndex", default, deserialize_with = "crate::winps::null_as_default")]
    pub index: u32,
    #[serde(rename = "InterfaceAlias", default, deserialize_with = "crate::winps::null_as_default")]
    pub alias: String,
    /// 网卡驱动描述，例如「Cisco AnyConnect Secure Mobility Client Virtual
    /// Miniport Adapter for Windows x64」。
    ///
    /// 判断是不是 VPN 网卡必须看这个字段：`InterfaceAlias` 是可以被用户随便改的
    /// 本地化名字，中文系统上 Cisco 的隧道网卡就叫「以太网 2」，
    /// 里面既没有 cisco 也没有 vpn，光看名字永远认不出来。
    #[serde(rename = "InterfaceDescription", default, deserialize_with = "crate::winps::null_as_default")]
    pub description: String,
}

/// 默认路由
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DefaultRoute {
    #[serde(rename = "NextHop", default, deserialize_with = "crate::winps::null_as_default")]
    pub next_hop: String,
    #[serde(rename = "InterfaceIndex", default, deserialize_with = "crate::winps::null_as_default")]
    pub index: u32,
    #[serde(rename = "InterfaceAlias", default, deserialize_with = "crate::winps::null_as_default")]
    pub alias: String,
    #[serde(rename = "RouteMetric", default, deserialize_with = "crate::winps::null_as_default")]
    pub metric: u32,
    /// 同 `InterfaceInfo::description`，用来识别 VPN 网卡
    #[serde(rename = "InterfaceDescription", default, deserialize_with = "crate::winps::null_as_default")]
    pub description: String,
}

/// 接口跃点数
#[derive(Debug, Clone, Deserialize, Default)]
pub struct InterfaceMetric {
    #[serde(rename = "InterfaceIndex", default, deserialize_with = "crate::winps::null_as_default")]
    pub index: u32,
    #[serde(rename = "InterfaceMetric", default, deserialize_with = "crate::winps::null_as_default")]
    pub metric: u32,
}

/// 一条路由的出口描述，屏蔽两个平台的差异
#[derive(Debug, Clone, Default)]
pub struct RouteTarget {
    /// Windows：接口索引
    pub if_index: u32,
    /// macOS：接口名，例如 utun3 / en0
    pub if_name: String,
    /// 下一跳地址；Windows 下为空表示 on-link（0.0.0.0）
    pub next_hop: String,
}

impl RouteTarget {
    pub fn new(if_index: u32, if_name: &str, next_hop: &str) -> Self {
        RouteTarget {
            if_index,
            if_name: if_name.to_string(),
            next_hop: next_hop.to_string(),
        }
    }

    /// 用于日志/界面展示
    pub fn display(&self) -> String {
        if self.if_name.is_empty() {
            format!("if#{}", self.if_index)
        } else {
            self.if_name.clone()
        }
    }
}

// ============================== CIDR 工具 ==============================

/// 校验并解析 CIDR，返回 (网络地址, 前缀长度)
pub fn parse_cidr(cidr: &str) -> Result<(u32, u8), String> {
    let text = cidr.trim();
    let (addr, prefix) = text
        .split_once('/')
        .ok_or_else(|| format!("{} 不是合法的 CIDR（缺少 /前缀长度）", cidr))?;
    let prefix: u8 = prefix
        .trim()
        .parse()
        .map_err(|_| format!("{} 的前缀长度不合法", cidr))?;
    if prefix > 32 {
        return Err(format!("{} 的前缀长度必须 <= 32", cidr));
    }
    let ip = parse_ipv4(addr.trim()).ok_or_else(|| format!("{} 的地址部分不合法", cidr))?;
    let mask = prefix_to_mask(prefix);
    Ok((ip & mask, prefix))
}

/// 把 CIDR 规范化成「网络地址/前缀长度」，同时完成合法性校验
pub fn normalize_cidr(cidr: &str) -> Result<String, String> {
    let (network, prefix) = parse_cidr(cidr)?;
    Ok(format!("{}/{}", format_ipv4(network), prefix))
}

/// 解析点分十进制 IPv4
pub fn parse_ipv4(text: &str) -> Option<u32> {
    let mut parts = text.split('.');
    let mut value: u32 = 0;
    for _ in 0..4 {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 {
            return None;
        }
        let octet: u32 = part.parse().ok()?;
        if octet > 255 {
            return None;
        }
        value = (value << 8) | octet;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(value)
}

/// u32 还原成点分十进制
pub fn format_ipv4(value: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        (value >> 24) & 0xff,
        (value >> 16) & 0xff,
        (value >> 8) & 0xff,
        value & 0xff
    )
}

/// 前缀长度转掩码
pub fn prefix_to_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

/// 取子网的第一个可用地址，作为隧道侧的下一跳。
///
/// 例：192.168.137.39 / 255.255.252.0 -> 192.168.136.1
/// 与官方 vpnc-script-win.js 里的 internal_gw 算法保持一致。
pub fn first_usable_in_subnet(ip: &str, netmask: &str) -> Option<String> {
    let ip = parse_ipv4(ip)?;
    let mask = if netmask.contains('.') {
        parse_ipv4(netmask)?
    } else {
        prefix_to_mask(netmask.parse().ok()?)
    };
    Some(format_ipv4((ip & mask).wrapping_add(1)))
}

/// 点分掩码转前缀长度（非连续掩码返回 None）
pub fn mask_to_prefix(mask: &str) -> Option<u8> {
    let value = parse_ipv4(mask)?;
    let prefix = value.count_ones() as u8;
    if prefix_to_mask(prefix) == value {
        Some(prefix)
    } else {
        None
    }
}

/// 由「本机地址 + 前缀长度」推出所在直连网段
pub fn subnet_of(ip: &str, prefix_len: u8) -> Option<String> {
    let ip = parse_ipv4(ip)?;
    Some(format!(
        "{}/{}",
        format_ipv4(ip & prefix_to_mask(prefix_len)),
        prefix_len
    ))
}

/// 判断接口名是否像 VPN 虚拟网卡。
///
/// 注意 openconnect 在 Windows 上创建的 Wintun 网卡名是「服务器名_随机后缀」
/// （例如 `vpn.risencn.com_ef4b51af`），名字里并不含 openconnect，
/// 所以这里必须把 "vpn" 也算作特征。
pub fn looks_like_vpn_interface(alias: &str) -> bool {
    let lower = alias.to_lowercase();
    const HINTS: &[&str] = &[
        "anyconnect",
        "secure client",
        "openconnect",
        "wintun",
        "tap-windows",
        "tap adapter",
        "cisco",
        "wireguard",
        "openvpn",
        "utun",
        "vpn",
    ];
    HINTS.iter().any(|hint| lower.contains(hint))
}

/// 名字或驱动描述任意一个像 VPN 网卡，就认定它是 VPN 网卡。
///
/// 必须同时看描述：Cisco AnyConnect 的隧道网卡在中文 Windows 上叫「以太网 2」，
/// 真正的身份信息只在驱动描述里（「Cisco AnyConnect … Virtual Miniport Adapter」）。
/// 只看名字的话，既找不到隧道网卡，还会把 VPN 网卡误当成物理出口。
pub fn looks_like_vpn_adapter(alias: &str, description: &str) -> bool {
    looks_like_vpn_interface(alias) || looks_like_vpn_interface(description)
}

/// 一块网卡是不是 VPN 虚拟网卡
pub fn interface_is_vpn(info: &InterfaceInfo) -> bool {
    looks_like_vpn_adapter(&info.alias, &info.description)
}

/// 一条默认路由的出口是不是 VPN 虚拟网卡
pub fn route_is_vpn(route: &DefaultRoute) -> bool {
    looks_like_vpn_adapter(&route.alias, &route.description)
}

// ============================== 查询 ==============================

// PowerShell 桥接与 JSON 数组归一化都来自公共模块，
// 这里只是给本模块的调用点起一个短名字，避免每个命令各写一遍。
// 两处都只在 Windows 分支被用到，所以按平台 gate 掉，避免其它平台报未使用。
#[cfg(target_os = "windows")]
use crate::winps::{json_array as as_array, run as powershell};

/// 列出所有 IPv4 地址及其所在网卡
pub fn list_interfaces() -> Result<Vec<InterfaceInfo>, String> {
    #[cfg(target_os = "windows")]
    {
        // Get-NetIPAddress 自己不带 InterfaceDescription（取出来是 null），
        // 只能再去 Get-NetAdapter 拿一次，按 ifIndex 关联上。
        // 拼成一条命令是为了省掉一次 PowerShell 启动 —— 隧道探测每 2 秒就要查一次。
        let text = powershell(
            "$a=@{}; Get-NetAdapter | ForEach-Object { $a[[int]$_.ifIndex]=$_.InterfaceDescription }; \
             Get-NetIPAddress -AddressFamily IPv4 | ForEach-Object { \
               [pscustomobject]@{ IPAddress=$_.IPAddress; PrefixLength=$_.PrefixLength; \
                 InterfaceIndex=$_.InterfaceIndex; InterfaceAlias=$_.InterfaceAlias; \
                 InterfaceDescription=$a[[int]$_.InterfaceIndex] } } | ConvertTo-Json -Compress",
        )?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("解析网卡列表失败: {}", e))?;
        let mut out = Vec::new();
        for item in as_array(value) {
            if let Ok(info) = serde_json::from_value::<InterfaceInfo>(item) {
                // 169.254.x.x 是链路本地地址，没有参考价值
                if !info.ip.starts_with("169.254.") {
                    out.push(info);
                }
            }
        }
        Ok(out)
    }
    #[cfg(not(target_os = "windows"))]
    {
        unix_list_interfaces()
    }
}

/// 列出所有默认路由
pub fn list_default_routes() -> Result<Vec<DefaultRoute>, String> {
    #[cfg(target_os = "windows")]
    {
        // 同 list_interfaces：Get-NetRoute 也不带 InterfaceDescription，
        // 需要按 ifIndex 关联 Get-NetAdapter 才拿得到驱动描述。
        let text = powershell(
            "$a=@{}; Get-NetAdapter | ForEach-Object { $a[[int]$_.ifIndex]=$_.InterfaceDescription }; \
             Get-NetRoute -DestinationPrefix '0.0.0.0/0' -PolicyStore ActiveStore -ErrorAction SilentlyContinue | \
             ForEach-Object { [pscustomobject]@{ NextHop=$_.NextHop; InterfaceIndex=$_.InterfaceIndex; \
               InterfaceAlias=$_.InterfaceAlias; RouteMetric=$_.RouteMetric; \
               InterfaceDescription=$a[[int]$_.InterfaceIndex] } } | ConvertTo-Json -Compress",
        )?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("解析默认路由失败: {}", e))?;
        Ok(as_array(value)
            .into_iter()
            .filter_map(|item| serde_json::from_value::<DefaultRoute>(item).ok())
            .collect())
    }
    #[cfg(target_os = "macos")]
    {
        use crate::process::run_with_timeout;
        use std::path::Path;
        use std::time::Duration;

        let (text, ok) = run_with_timeout(
            Path::new(MACOS_ROUTE_BIN),
            &["-n", "get", "default"],
            Duration::from_secs(10),
        );
        if !ok {
            return Ok(Vec::new());
        }
        let mut route = DefaultRoute::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("gateway:") {
                route.next_hop = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("interface:") {
                route.alias = rest.trim().to_string();
            }
        }
        if route.next_hop.is_empty() && route.alias.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![route])
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Ok(Vec::new())
    }
}

/// 接口跃点数（Windows 才有意义）
pub fn list_interface_metrics() -> Result<Vec<InterfaceMetric>, String> {
    #[cfg(target_os = "windows")]
    {
        let text = powershell(
            "Get-NetIPInterface -AddressFamily IPv4 | \
             Select-Object InterfaceIndex,InterfaceMetric | ConvertTo-Json -Compress",
        )?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("解析接口跃点数失败: {}", e))?;
        Ok(as_array(value)
            .into_iter()
            .filter_map(|item| serde_json::from_value::<InterfaceMetric>(item).ok())
            .collect())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(Vec::new())
    }
}

/// 按隧道内网地址反查网卡 —— 定位 VPN 网卡最可靠的方式，
/// 比拼网卡名稳定（名字会被系统本地化，还可能带随机后缀）
pub fn find_interface_by_ip(ip: &str) -> Result<Option<InterfaceInfo>, String> {
    let target = parse_ipv4(ip).ok_or_else(|| format!("{} 不是合法的 IPv4 地址", ip))?;
    for info in list_interfaces()? {
        if parse_ipv4(&info.ip) == Some(target) {
            return Ok(Some(info));
        }
    }
    Ok(None)
}

/// 挑出「真正的物理出口」：排除 VPN 虚拟网卡，再按有效跃点数取最小的一条
pub fn pick_physical_default(
    routes: &[DefaultRoute],
    metrics: &[InterfaceMetric],
) -> Option<DefaultRoute> {
    let metric_of = |index: u32| -> u32 {
        metrics
            .iter()
            .find(|m| m.index == index)
            .map(|m| m.metric)
            .unwrap_or(0)
    };

    let mut candidates: Vec<&DefaultRoute> = routes
        .iter()
        .filter(|r| !r.next_hop.is_empty() && r.next_hop != "0.0.0.0")
        .filter(|r| !route_is_vpn(r))
        .collect();

    candidates.sort_by_key(|r| r.metric + metric_of(r.index));
    candidates.first().map(|r| (*r).clone())
}

// ============================== 修改 ==============================

/// 添加一条临时路由（只写 ActiveStore，重启后自然消失）
pub fn add_route(cidr: &str, target: &RouteTarget, metric: u32) -> Result<(), String> {
    let cidr = normalize_cidr(cidr)?;

    #[cfg(target_os = "windows")]
    {
        let next_hop = if target.next_hop.is_empty() {
            "0.0.0.0".to_string()
        } else {
            target.next_hop.clone()
        };
        let script = format!(
            "New-NetRoute -DestinationPrefix '{}' -InterfaceIndex {} -NextHop '{}' -RouteMetric {} \
             -PolicyStore ActiveStore -ErrorAction Stop | Out-Null",
            cidr, target.if_index, next_hop, metric
        );
        powershell(&script).map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = metric;
        let args: Vec<String> = if target.next_hop.is_empty() || target.if_name.starts_with("utun") {
            vec![
                "-n".into(),
                "add".into(),
                "-net".into(),
                cidr.clone(),
                "-interface".into(),
                target.if_name.clone(),
            ]
        } else {
            vec![
                "-n".into(),
                "add".into(),
                "-net".into(),
                cidr.clone(),
                target.next_hop.clone(),
            ]
        };
        run_route_macos(&args, &["File exists"])
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (target, metric);
        Err(format!("当前平台暂不支持添加路由，目标 {}", cidr))
    }
}

/// 删除一条临时路由；路由不存在时不报错
pub fn delete_route(cidr: &str, target: &RouteTarget) -> Result<(), String> {
    let cidr = normalize_cidr(cidr)?;

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "Remove-NetRoute -DestinationPrefix '{}' -InterfaceIndex {} -Confirm:$false \
             -PolicyStore ActiveStore -ErrorAction SilentlyContinue | Out-Null",
            cidr, target.if_index
        );
        powershell(&script).map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        let args: Vec<String> = if target.next_hop.is_empty() || target.if_name.starts_with("utun") {
            vec![
                "-n".into(),
                "delete".into(),
                "-net".into(),
                cidr.clone(),
                "-interface".into(),
                target.if_name.clone(),
            ]
        } else {
            vec![
                "-n".into(),
                "delete".into(),
                "-net".into(),
                cidr.clone(),
                target.next_hop.clone(),
            ]
        };
        run_route_macos(&args, &["not in table", "No such process"])
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = target;
        Ok(())
    }
}

/// 删除一条主机路由（/32）。
///
/// 专门用来清理脚本给 VPN 服务器加的防环主机路由：worker 停隧道走的是 SIGKILL，
/// 脚本的 disconnect 分支不会执行，这条主机路由会一直残留。残留路由绑定的源地址
/// 在物理网卡地址变化后可能失效，内核发起连接时选不到本地地址，就会直接以
/// `Can't assign requested address` 失败——连最初的 TCP 握手都做不了。
pub fn delete_host_route(ip: &str) -> Result<(), String> {
    if parse_ipv4(ip).is_none() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let args = vec![
            "-n".to_string(),
            "delete".to_string(),
            "-host".to_string(),
            ip.to_string(),
        ];
        run_route_macos(&args, &["not in table", "No such process", "no such process"])
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// 把默认路由交还给连接前的物理网关。
///
/// 「以公司网络为主」模式下脚本会把 `default` 抢给隧道，而 worker 停止隧道走的是
/// SIGKILL —— openconnect 根本没机会执行脚本的 disconnect 分支，所以必须由 worker
/// 自己交还。不交还的话断开后整机没有任何默认路由，本地网络和公网会一起断掉。
pub fn restore_default_route(original_gateway: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // 隧道网卡消失时系统会带走挂在它上面的默认路由，这里删一次只是兜底，
        // 删不到就当没这回事，别让「删」的失败挡住后面的「加」。
        let delete_args = vec!["-n".to_string(), "delete".to_string(), "default".to_string()];
        let _ = run_route_macos(
            &delete_args,
            &["not in table", "No such process", "no such process"],
        );
        if original_gateway.is_empty() {
            return Ok(());
        }
        let add_args = vec![
            "-n".to_string(),
            "add".to_string(),
            "default".to_string(),
            original_gateway.to_string(),
        ];
        run_route_macos(&add_args, &["File exists"])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = original_gateway;
        Ok(())
    }
}

/// `ensure_route` 的结果：区分「新建」与「只是调了跃点数」，
/// 断开时前者要删掉，后者只需把跃点数还原，避免误删系统自动生成的直连路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOutcome {
    Created,
    Updated,
}

/// 让一条路由「存在且跃点数正确」。
///
/// 公司网络为主模式会用到：客户网段本来就有系统自动生成的直连路由，
/// 但可能被 VPN 客户端改动过，这时不能简单删掉重加，只能改跃点数把它抢回来。
pub fn ensure_route(
    cidr: &str,
    target: &RouteTarget,
    metric: u32,
) -> Result<RouteOutcome, String> {
    let cidr = normalize_cidr(cidr)?;

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "$existing = Get-NetRoute -DestinationPrefix '{cidr}' -InterfaceIndex {idx} \
             -PolicyStore ActiveStore -ErrorAction SilentlyContinue; \
             if ($existing) {{ \
                 $existing | Set-NetRoute -RouteMetric {metric} -PolicyStore ActiveStore -ErrorAction Stop; \
                 Write-Output 'updated' \
             }} else {{ \
                 New-NetRoute -DestinationPrefix '{cidr}' -InterfaceIndex {idx} -NextHop '{hop}' \
                 -RouteMetric {metric} -PolicyStore ActiveStore -ErrorAction Stop | Out-Null; \
                 Write-Output 'created' \
             }}",
            cidr = cidr,
            idx = target.if_index,
            metric = metric,
            hop = if target.next_hop.is_empty() {
                "0.0.0.0".to_string()
            } else {
                target.next_hop.clone()
            }
        );
        let text = powershell(&script)?;
        Ok(if text.trim().contains("updated") {
            RouteOutcome::Updated
        } else {
            RouteOutcome::Created
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = metric;
        if route_exists(&cidr, target)? {
            Ok(RouteOutcome::Updated)
        } else {
            add_route(&cidr, target, metric)?;
            Ok(RouteOutcome::Created)
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (cidr, target, metric);
        Ok(RouteOutcome::Created)
    }
}

/// 把跃点数还原成一个中性值（用于回收 `RouteOutcome::Updated` 的路由）
pub fn reset_route_metric(cidr: &str, target: &RouteTarget, metric: u32) -> Result<(), String> {
    let cidr = normalize_cidr(cidr)?;

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "Get-NetRoute -DestinationPrefix '{}' -InterfaceIndex {} -PolicyStore ActiveStore \
             -ErrorAction SilentlyContinue | Set-NetRoute -RouteMetric {} -PolicyStore ActiveStore \
             -ErrorAction SilentlyContinue | Out-Null",
            cidr, target.if_index, metric
        );
        powershell(&script).map(|_| ())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (cidr, target, metric);
        Ok(())
    }
}

/// 判断某条路由当前是否已经存在（用于路由守护）
pub fn route_exists(cidr: &str, target: &RouteTarget) -> Result<bool, String> {
    let cidr = normalize_cidr(cidr)?;

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "@(Get-NetRoute -DestinationPrefix '{}' -PolicyStore ActiveStore -ErrorAction SilentlyContinue | \
             Where-Object {{ $_.InterfaceIndex -eq {} }}).Count",
            cidr, target.if_index
        );
        let text = powershell(&script)?;
        Ok(text.trim().parse::<u32>().unwrap_or(0) > 0)
    }
    #[cfg(target_os = "macos")]
    {
        use crate::process::run_with_timeout;
        use std::path::Path;
        use std::time::Duration;

        let _ = target;
        let (text, _) = run_with_timeout(
            Path::new("/usr/sbin/netstat"),
            &["-rn", "-f", "inet"],
            Duration::from_secs(15),
        );
        let (network, prefix) = parse_cidr(&cidr)?;
        // 必须逐行取「目标」列再解析，不能拿整段文本做子串匹配：
        // 字符串比对在 netstat 的缩写形式下永远不成立（见 parse_netstat_destination）。
        Ok(text.lines().any(|line| {
            let token = match line.split_whitespace().next() {
                Some(token) => token,
                None => return false,
            };
            if token == "default" {
                return false;
            }
            matches!(parse_netstat_destination(token), Some(parsed) if parsed == (network, prefix))
        }))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (cidr, target);
        Ok(true)
    }
}

/// 解析 `netstat -rn -f inet` 的目标列，返回 (网络地址, 前缀长度)。
///
/// macOS 的 netstat 会省略尾部为 0 的八位组，并且只在「非 8/16/24 边界」时才
/// 追加 `/前缀`，现场实测：
/// - `192.168.1.0/24` → `192.168.1`
/// - `127.0.0.0/8`    → `127`
/// - `169.254.0.0/16` → `169.254`
/// - `224.0.0.0/4`    → `224.0.0/4`
/// - `192.168.1.1/32` → `192.168.1.1/32`
///
/// 直接拿 `192.168.1.0` 去 `contains` 会一条也匹配不上，路由守护因此每 15 秒
/// 重下一次路由（日志刷屏 + `File exists`）。这里按缩写规则反推回网络地址。
#[cfg(target_os = "macos")]
fn parse_netstat_destination(token: &str) -> Option<(u32, u8)> {
    let (addr_text, prefix) = match token.split_once('/') {
        Some((addr, prefix)) => (addr, prefix.trim().parse::<u8>().ok()?),
        None => {
            let prefix = match token.split('.').count() {
                1 => 8,
                2 => 16,
                3 => 24,
                4 => 32,
                _ => return None,
            };
            (token, prefix)
        }
    };
    if prefix > 32 {
        return None;
    }
    let mut octets: Vec<&str> = addr_text.split('.').collect();
    if octets.len() > 4 {
        return None;
    }
    while octets.len() < 4 {
        octets.push("0");
    }
    let ip = parse_ipv4(&octets.join("."))?;
    Some((ip & prefix_to_mask(prefix), prefix))
}

#[cfg(target_os = "macos")]
fn run_route_macos(args: &[String], tolerated: &[&str]) -> Result<(), String> {
    use crate::process::run_with_timeout;
    use std::path::Path;
    use std::time::Duration;

    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (text, ok) = run_with_timeout(
        Path::new(MACOS_ROUTE_BIN),
        &refs,
        Duration::from_secs(15),
    );
    if ok || tolerated.iter().any(|t| text.contains(t)) {
        Ok(())
    } else {
        Err(format!("route {} 失败: {}", refs.join(" "), text.trim()))
    }
}

/// macOS：列出所有网卡及其 IPv4 地址
#[cfg(not(target_os = "windows"))]
fn unix_list_interfaces() -> Result<Vec<InterfaceInfo>, String> {
    use crate::process::run_with_timeout;
    use std::path::Path;
    use std::time::Duration;

    let (text, ok) = run_with_timeout(Path::new("/sbin/ifconfig"), &["-a"], Duration::from_secs(10));
    if !ok {
        return Err(format!("ifconfig 执行失败: {}", text.trim()));
    }

    let mut out = Vec::new();
    let mut current_alias = String::new();
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            // 缩进行是属性，例如：inet 10.8.0.2 --> 10.8.0.1 netmask 0xffffff00
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("inet ") {
                let ip = rest.split_whitespace().next().unwrap_or("").to_string();
                if ip.is_empty() || ip.starts_with("169.254.") {
                    continue;
                }
                let prefix_len = rest
                    .split("netmask ")
                    .nth(1)
                    .and_then(|hex| hex.split_whitespace().next())
                    .and_then(|hex| u32::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
                    .map(|mask| mask.count_ones() as u8)
                    .unwrap_or(24);
                out.push(InterfaceInfo {
                    ip,
                    prefix_len,
                    index: 0,
                    alias: current_alias.clone(),
                    // ifconfig 不提供驱动描述；macOS 上隧道网卡本来就叫 utunN，
                    // 名字里已经带特征，够用了。
                    description: String::new(),
                });
            }
        } else if let Some((name, _)) = line.split_once(':') {
            current_alias = name.trim().to_string();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(alias: &str, next_hop: &str, index: u32, metric: u32) -> DefaultRoute {
        route_desc(alias, "", next_hop, index, metric)
    }

    fn route_desc(
        alias: &str,
        description: &str,
        next_hop: &str,
        index: u32,
        metric: u32,
    ) -> DefaultRoute {
        DefaultRoute {
            next_hop: next_hop.to_string(),
            index,
            alias: alias.to_string(),
            metric,
            description: description.to_string(),
        }
    }

    /// 用户现场 Cisco AnyConnect 网卡的两条真实记录（中文 Windows）
    fn cisco_anyconnect_route() -> DefaultRoute {
        route_desc(
            "以太网 2",
            "Cisco AnyConnect Secure Mobility Client Virtual Miniport Adapter for Windows x64",
            "192.168.136.1",
            6,
            1,
        )
    }

    /// 同一台机器上的真实物理网卡
    fn realtek_route() -> DefaultRoute {
        route_desc(
            "以太网",
            "Realtek PCIe 2.5GbE Family Controller",
            "192.168.1.1",
            5,
            0,
        )
    }

    #[test]
    fn parse_cidr_normalizes_network_address() {
        // 192.168.1.77/24 的网络地址应该被规范化成 192.168.1.0
        assert_eq!(normalize_cidr("192.168.1.77/24").unwrap(), "192.168.1.0/24");
    }

    #[test]
    fn parse_cidr_accepts_company_range() {
        assert_eq!(
            normalize_cidr("192.168.136.0/22").unwrap(),
            "192.168.136.0/22"
        );
    }

    #[test]
    fn parse_cidr_rejects_garbage() {
        assert!(parse_cidr("192.168.1.0").is_err());
        assert!(parse_cidr("192.168.1.0/33").is_err());
        assert!(parse_cidr("192.168.1.256/24").is_err());
        assert!(parse_cidr("not-an-ip/24").is_err());
        assert!(parse_cidr("1.2.3.4.5/24").is_err());
    }

    #[test]
    fn empty_prefix_is_valid_default_route() {
        assert_eq!(normalize_cidr("0.0.0.0/0").unwrap(), "0.0.0.0/0");
    }

    #[test]
    fn mask_prefix_roundtrip() {
        assert_eq!(prefix_to_mask(0), 0);
        assert_eq!(prefix_to_mask(24), 0xFFFFFF00);
        assert_eq!(mask_to_prefix("255.255.252.0"), Some(22));
        assert_eq!(mask_to_prefix("255.255.255.0"), Some(24));
        // 非连续掩码不合法
        assert_eq!(mask_to_prefix("255.0.255.0"), None);
    }

    #[test]
    fn first_usable_matches_vpnc_script_convention() {
        assert_eq!(
            first_usable_in_subnet("192.168.137.39", "255.255.252.0").unwrap(),
            "192.168.136.1"
        );
        assert_eq!(
            first_usable_in_subnet("10.8.0.6", "255.255.255.0").unwrap(),
            "10.8.0.1"
        );
    }

    #[test]
    fn first_usable_accepts_prefix_length() {
        assert_eq!(
            first_usable_in_subnet("192.168.137.39", "22").unwrap(),
            "192.168.136.1"
        );
    }

    #[test]
    fn subnet_of_customer_lan() {
        assert_eq!(subnet_of("192.168.1.2", 24).unwrap(), "192.168.1.0/24");
    }

    #[test]
    fn detects_vpn_interfaces() {
        assert!(looks_like_vpn_interface(
            "Cisco AnyConnect Secure Mobility Client Virtual Miniport Adapter"
        ));
        assert!(looks_like_vpn_interface("vpn.risencn.com_ef4b51af"));
        assert!(looks_like_vpn_interface("Wintun Userspace Tunnel"));
        assert!(looks_like_vpn_interface("utun3"));
        assert!(!looks_like_vpn_interface(
            "Realtek PCIe 2.5GbE Family Controller"
        ));
        assert!(!looks_like_vpn_interface("以太网"));
    }

    #[test]
    fn vpn_adapter_is_recognized_by_driver_description() {
        // 用户现场的真相：名字是「以太网 2」，一点 VPN 特征都没有，
        // 只有驱动描述里写着 Cisco AnyConnect。只看名字永远认不出来。
        assert!(looks_like_vpn_adapter(
            "以太网 2",
            "Cisco AnyConnect Secure Mobility Client Virtual Miniport Adapter for Windows x64"
        ));
        // 物理网卡的名字和描述都不像 VPN，不能被误判
        assert!(!looks_like_vpn_adapter(
            "以太网",
            "Realtek PCIe 2.5GbE Family Controller"
        ));
        // 名字就带特征的（macOS utun / openconnect 的 wintun）依然要认出来
        assert!(looks_like_vpn_adapter("utun3", ""));
        assert!(looks_like_vpn_adapter("", "Wintun Userspace Tunnel"));
    }

    #[test]
    fn ipv4_format_roundtrip() {
        for text in ["0.0.0.0", "192.168.1.2", "255.255.255.255", "10.8.0.1"] {
            let value = parse_ipv4(text).unwrap();
            assert_eq!(format_ipv4(value), text);
        }
    }

    #[test]
    fn physical_default_ignores_anyconnect_adapter() {
        // 复刻用户现场（中文 Windows）：AnyConnect 网卡叫「以太网 2」，
        // 只有驱动描述能认出它；它的默认路由跃点数还比物理网卡更低，
        // 照着名字过滤就会把它当成物理出口。
        let routes = vec![cisco_anyconnect_route(), realtek_route()];
        let metrics = vec![
            InterfaceMetric { index: 6, metric: 1 },
            InterfaceMetric { index: 5, metric: 35 },
        ];
        let picked = pick_physical_default(&routes, &metrics).unwrap();
        assert_eq!(picked.alias, "以太网");
        assert_eq!(picked.next_hop, "192.168.1.1");
    }

    #[test]
    fn physical_default_skips_onlink_entries() {
        let routes = vec![
            route("以太网", "0.0.0.0", 5, 0),
            route("以太网", "192.168.1.1", 5, 0),
        ];
        let picked = pick_physical_default(&routes, &[]).unwrap();
        assert_eq!(picked.next_hop, "192.168.1.1");
    }

    #[test]
    fn physical_default_none_when_only_vpn() {
        let routes = vec![cisco_anyconnect_route()];
        assert!(pick_physical_default(&routes, &[]).is_none());
    }

    // ===== 真机集成测试 =====
    //
    // 路由这一层最大的风险不是算法，而是「中文 Windows 上 PowerShell 输出能不能
    // 稳定解析成 JSON」。文本解析（route print / netsh）在中文系统下会失效，
    // 所以这里直接在本机跑一遍真实查询，确保解析链路是通的。

    #[cfg(target_os = "windows")]
    #[test]
    fn powershell_json_queries_work_on_this_machine() {
        let interfaces = list_interfaces().expect("读取网卡列表失败");
        assert!(!interfaces.is_empty(), "至少应该能读到一块网卡");
        assert!(
            interfaces.iter().any(|info| parse_ipv4(&info.ip).is_some()),
            "网卡列表里应该有可解析的 IPv4 地址"
        );
        // 网卡名可能是中文（例如「以太网」），不能被编码破坏成乱码
        assert!(
            interfaces.iter().all(|info| !info.ip.contains('\u{fffd}')),
            "网卡地址里出现了编码损坏的字符"
        );

        let metrics = list_interface_metrics().expect("读取接口跃点数失败");
        assert!(!metrics.is_empty(), "至少要有一条接口跃点数");

        let routes = list_default_routes().expect("读取默认路由失败");
        assert!(!routes.is_empty(), "至少要有一条默认路由");

        let physical = pick_physical_default(&routes, &metrics);
        assert!(physical.is_some(), "应该能挑出物理默认路由");
        let physical = physical.unwrap();
        assert!(
            !route_is_vpn(&physical),
            "物理出口不能是 VPN 网卡，否则分流规则的「本地直连网段」会算成公司内网: {} / {}",
            physical.alias,
            physical.description
        );
        assert!(
            parse_ipv4(&physical.next_hop).is_some(),
            "物理默认路由的下一跳应该是合法 IPv4: {}",
            physical.next_hop
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn interfaces_and_routes_carry_driver_description_on_this_machine() {
        // 这条是给「Cisco 网卡叫以太网 2」那个 bug 兜底的：
        // 只要解析链路把 InterfaceDescription 丢了，VPN 网卡就再也认不出来，
        // 而且还会被当成物理出口，分流规则会整体失效。
        let interfaces = list_interfaces().expect("读取网卡列表失败");
        assert!(
            interfaces.iter().any(|info| !info.description.is_empty()),
            "网卡列表没带上驱动描述，无法识别名字被本地化的 VPN 网卡"
        );

        let routes = list_default_routes().expect("读取默认路由失败");
        assert!(
            routes.iter().any(|r| !r.description.is_empty()),
            "默认路由没带上驱动描述，无法把 VPN 网卡的默认路由排除掉"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn route_exists_query_works_on_this_machine() {
        // 拿本机回环所在的网段做一次只读查询，不修改任何路由
        let target = RouteTarget::new(0, "", "0.0.0.0");
        let result = route_exists("127.0.0.0/8", &target);
        assert!(result.is_ok(), "route_exists 不该报错: {:?}", result.err());
    }

    // macOS 的 netstat 会把 192.168.1.0/24 印成 `192.168.1`、127.0.0.0/8 印成 `127`。
    // 解析必须按缩写规则还原成网络地址，否则路由守护永远判「路由不存在」而反复重下发。
    #[cfg(target_os = "macos")]
    #[test]
    fn parses_abbreviated_netstat_destinations() {
        assert_eq!(parse_netstat_destination("127"), Some((0x7f00_0000, 8)));
        assert_eq!(parse_netstat_destination("192.168.1"), Some((0xc0a8_0100, 24)));
        assert_eq!(parse_netstat_destination("169.254"), Some((0xa9fe_0000, 16)));
        assert_eq!(parse_netstat_destination("224.0.0/4"), Some((0xe000_0000, 4)));
        assert_eq!(parse_netstat_destination("192.168.136/22"), Some((0xc0a8_8800, 22)));
        assert_eq!(
            parse_netstat_destination("192.168.1.1/32"),
            Some((0xc0a8_0101, 32))
        );
        // 表头等非地址目标必须被丢掉
        assert_eq!(parse_netstat_destination("default"), None);
        assert_eq!(parse_netstat_destination("Routing"), None);
        assert_eq!(parse_netstat_destination("Internet:"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn route_exists_finds_a_route_that_is_really_there() {
        use crate::process::run_with_timeout;
        use std::path::Path;
        use std::time::Duration;

        // 真机集成：本机 netstat 里到底有哪些网段不由测试说了算，所以先读一遍真实路由表，
        // 挑一条能解析出来的网段，再交给 route_exists 走完整链路（起 netstat -> 解析缩写
        // -> 比对）确认它认得出来。
        //
        // 之前写死 127.0.0.0/8 是错的：本机 netstat 只有 `127.0.0.1` 这条 /32 主机路由，
        // 并没有 `127`（127.0.0.0/8）网段路由，断言「一定存在」会在真机上误报。
        let (text, _) = run_with_timeout(
            Path::new("/usr/sbin/netstat"),
            &["-rn", "-f", "inet"],
            Duration::from_secs(15),
        );
        let sample = text
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|token| *token != "default")
            .filter_map(parse_netstat_destination)
            .find(|(_, prefix)| *prefix != 32)
            .expect("本机 netstat 至少应有一条可解析的网段路由");
        let (network, prefix) = sample;
        let cidr = format!("{}/{}", format_ipv4(network), prefix);
        let target = RouteTarget::new(0, "", "0.0.0.0");
        assert_eq!(
            route_exists(&cidr, &target),
            Ok(true),
            "从本机路由表里挑出来的 {} 必须能被 route_exists 认出来",
            cidr
        );
    }

    // ===== 真机集成测试（macOS）=====
    //
    // 现场教训：`route` 的绝对路径写成 `/usr/sbin/route`（macOS 上该文件并不存在）
    // 时进程直接起不来（ENOENT），默认路由被读成空，分流路由也全部下发失败，
    // 但两条症状都不会提示「路径写错了」——只有真实跑一次才能暴露。
    // 这里在真机上验证：路径确实存在，且能读出一条带接口名的物理默认路由。

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_route_binary_path_exists_and_query_works() {
        assert!(
            std::path::Path::new(MACOS_ROUTE_BIN).exists(),
            "macOS 的 route 命令路径必须是真实存在的文件: {}",
            MACOS_ROUTE_BIN
        );
        let routes = list_default_routes().expect("读取默认路由失败");
        assert!(
            !routes.is_empty(),
            "读不到默认路由，分流规则就拿不到物理出口（现场表现为「未找到可用的物理默认路由」）"
        );
        let physical = pick_physical_default(&routes, &[]).expect("应该能挑出物理默认路由");
        assert!(
            !physical.alias.is_empty(),
            "物理默认路由必须带接口名，否则 route add 的 -interface 会是空值"
        );
    }
}
