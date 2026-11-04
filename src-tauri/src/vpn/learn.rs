//! 学习模式：把「实际在用的远端」变成一份可以确认的候选网段清单
//!
//! 分流规则表最难的一步不是填，而是不知道该填什么。用户手里往往只有域名
//! （`git.example.com` 这类），拿不到网段；而 IP 层转发又做不了域名匹配。
//! 换个方向就有了：连上隧道之后，让系统自己告诉我们当前正在跟哪些远端通信。
//!
//! 每轮采样做四件事：
//!
//! 1. 拉一次 `Get-NetTCPConnection -State Established`，拿到所有已建立的连接；
//! 2. 过滤掉本机、回环、链路本地、组播，以及物理网卡直连网段里的对端；
//! 3. 按 /24 归并远端地址；同一个上层里出现多个 /24 时再向上汇聚成 /16、/8；
//! 4. 跟现有规则表比对，把这个候选标成「已覆盖」还是「还没进规则表」。
//!
//! 只观察、只建议，**绝不自动改规则表**——自动往里塞网段，出事时用户根本
//! 不知道是哪一步把流量引到了公司网络。
//!
//! 观察结果里一定混着公网地址，这不是缺陷：报告会同时标出 `via_vpn`
//! （这条连接是否真的走了隧道）和 `covered`（现有规则是否已经照顾到）。
//! 在「以客户网络为主」模式下，用户要找的正是「没被规则覆盖、但本该走隧道」
//! 的那几条。

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::vpn::models::{LearnedHost, LearnedSubnet, LearnReport, LearnSummary};
use crate::vpn::route;

/// 单份报告最多保留多少台远端主机，防止长时间挂着把文件撑爆
const MAX_HOSTS: usize = 2000;
/// 每个候选网段最多留几条采样地址（只是给人看的线索，不是全量）
const MAX_SAMPLES: usize = 8;
/// 每台主机最多记几个远端端口
const MAX_PORTS: usize = 8;
/// 每台主机最多记几个发起进程
const MAX_PROCESSES: usize = 4;
/// 汇聚成 /16 至少要在同一个 /16 里观察到这么多个 /24
const BROAD_16_MIN_BLOCKS: usize = 4;
/// 汇聚成 /8 至少要在同一个 /8 里观察到这么多个 /16
const BROAD_8_MIN_BLOCKS: usize = 4;
/// 采样连续失败这么多次就停用学习模式，避免每 15 秒刷一条失败日志
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// 一条观察到的已建立连接（已归一化）
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// 远端 IPv4
    pub ip: String,
    pub remote_port: u16,
    /// 这条连接的本地地址就是隧道地址 -> 确实走了隧道
    pub via_vpn: bool,
    /// 发起连接的进程名
    pub process: String,
}

/// `Get-NetTCPConnection` 的原始行
#[derive(Debug, Clone, Default, Deserialize)]
struct RawConnection {
    #[serde(
        rename = "RemoteAddress",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    remote_address: String,
    #[serde(
        rename = "RemotePort",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    remote_port: u16,
    #[serde(
        rename = "LocalAddress",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    local_address: String,
    #[serde(
        rename = "ProcessName",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    process_name: String,
}

/// 采样脚本。
///
/// - 先在可用性上做一次判断：`Get-NetTCPConnection` 从 Windows 8 才有，
///   缺失时输出 `UNSUPPORTED` 而不是抛异常 —— `winps::wrap` 里设了
///   `$ErrorActionPreference='Stop'`，命令找不到会变成终止性错误，
///   那样每 15 秒就要在日志里刷一条堆栈。
/// - 进程名一次性建表查，不为每条连接单独 `Get-Process`（几百条连接能差出几秒）。
#[cfg(target_os = "windows")]
const ESTABLISHED_QUERY: &str = "$p=@{}; \
     Get-Process -ErrorAction SilentlyContinue | ForEach-Object { $p[[int]$_.Id]=$_.ProcessName }; \
     if ($null -eq (Get-Command Get-NetTCPConnection -ErrorAction SilentlyContinue)) { 'UNSUPPORTED' } else { \
       Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue | ForEach-Object { \
         [pscustomobject]@{ RemoteAddress=$_.RemoteAddress; RemotePort=$_.RemotePort; \
           LocalAddress=$_.LocalAddress; ProcessName=$p[[int]$_.OwningProcess] } } | \
       ConvertTo-Json -Compress }";

/// 拉一轮已建立的连接并归一化。
///
/// `vpn_ips` 是本机隧道地址，用来判断某条连接是不是走的隧道；
/// `exclude_subnets` 是物理网卡直连网段，同一个局域网里的对端不该被建议
/// 塞进 VPN 规则表。
pub fn sample(
    vpn_ips: &[String],
    exclude_subnets: &[String],
    self_processes: &[String],
) -> Result<Vec<Observation>, String> {
    #[cfg(target_os = "windows")]
    {
        let text = crate::winps::run(ESTABLISHED_QUERY)?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if text == "UNSUPPORTED" {
            return Err(
                "系统里没有 Get-NetTCPConnection（需要 Windows 8 及以上），无法采集远端连接"
                    .to_string(),
            );
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("解析连接列表失败: {}", e))?;
        let raw: Vec<RawConnection> = crate::winps::parse_list(value);
        Ok(classify(
            &raw,
            vpn_ips,
            exclude_subnets,
            self_processes,
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        // macOS 侧先不做学习模式：网络栈与取数方式都不一样，
        // 与其给一份半成品，不如明确地什么都不返回。
        let _ = (vpn_ips, exclude_subnets, self_processes);
        Ok(Vec::new())
    }
}

/// 过滤 + 归类。抽成纯函数是为了能在任意平台上跑单测。
fn classify(
    raw: &[RawConnection],
    vpn_ips: &[String],
    exclude_subnets: &[String],
    self_processes: &[String],
) -> Vec<Observation> {
    let excluded: Vec<(u32, u8)> = exclude_subnets
        .iter()
        .filter_map(|cidr| route::parse_cidr(cidr).ok())
        .collect();

    let mut out = Vec::new();
    for item in raw {
        let ip = match route::parse_ipv4(&item.remote_address.trim()) {
            Some(ip) => ip,
            // 不是 IPv4（含 IPv6）一律跳过：规则表只认 IPv4
            None => continue,
        };
        if is_uninteresting(ip) {
            continue;
        }
        // 同一个局域网里的对端本来就不该走 VPN
        if excluded
            .iter()
            .any(|(net, len)| ip & route::prefix_to_mask(*len) == *net)
        {
            continue;
        }
        // openconnect 自己那条到 VPN 服务器的连接、以及本程序，都不是分流目标
        let process = item.process_name.trim().to_string();
        if self_processes
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&process))
        {
            continue;
        }
        let via_vpn = vpn_ips
            .iter()
            .any(|local| local.trim() == item.local_address.trim());
        out.push(Observation {
            ip: route::format_ipv4(ip),
            remote_port: item.remote_port,
            via_vpn,
            process,
        });
    }
    out
}

/// 本机自身、回环、链路本地、组播、广播：都不是分流目标
fn is_uninteresting(ip: u32) -> bool {
    ip == 0
        || ip == u32::MAX
        || ip >> 24 == 127
        || ip & 0xffff_0000 == 0xa9fe_0000
        || ip >= 0xe000_0000
}

// ============================== 归并与反推 ==============================

/// 把一轮观察并进报告，并重算候选网段
pub fn merge(
    report: &mut LearnReport,
    observations: &[Observation],
    existing_rules: &[String],
    now: &str,
) {
    report.scans = report.scans.saturating_add(1);
    report.updated_at = now.to_string();

    let mut index: BTreeMap<String, usize> = report
        .hosts
        .iter()
        .enumerate()
        .map(|(i, host)| (host.ip.clone(), i))
        .collect();

    for obs in observations {
        match index.get(&obs.ip).copied() {
            Some(i) => {
                let host = &mut report.hosts[i];
                host.hits = host.hits.saturating_add(1);
                host.last_seen = now.to_string();
                host.via_vpn |= obs.via_vpn;
                push_unique_u16(&mut host.ports, obs.remote_port, MAX_PORTS);
                if !obs.process.is_empty() {
                    push_unique_string(&mut host.processes, &obs.process, MAX_PROCESSES);
                }
            }
            None => {
                let mut host = LearnedHost {
                    ip: obs.ip.clone(),
                    ports: Vec::new(),
                    hits: 1,
                    via_vpn: obs.via_vpn,
                    processes: Vec::new(),
                    first_seen: now.to_string(),
                    last_seen: now.to_string(),
                };
                push_unique_u16(&mut host.ports, obs.remote_port, MAX_PORTS);
                if !obs.process.is_empty() {
                    push_unique_string(&mut host.processes, &obs.process, MAX_PROCESSES);
                }
                index.insert(obs.ip.clone(), report.hosts.len());
                report.hosts.push(host);
            }
        }
    }

    trim_hosts(&mut report.hosts);
    report.candidates = infer(&report.hosts, existing_rules);
}

/// 主机明细太多时，只留命中次数最高的一批
fn trim_hosts(hosts: &mut Vec<LearnedHost>) {
    if hosts.len() <= MAX_HOSTS {
        return;
    }
    hosts.sort_by(|a, b| b.hits.cmp(&a.hits).then(a.ip.cmp(&b.ip)));
    hosts.truncate(MAX_HOSTS);
    // 明细表本身按 IP 排序，界面上更好找
    hosts.sort_by(|a, b| {
        route::parse_ipv4(&a.ip)
            .unwrap_or(0)
            .cmp(&route::parse_ipv4(&b.ip).unwrap_or(0))
    });
}

/// 由主机明细反推候选网段
pub fn infer(hosts: &[LearnedHost], existing_rules: &[String]) -> Vec<LearnedSubnet> {
    let rules: Vec<(u32, u8)> = existing_rules
        .iter()
        .filter_map(|cidr| route::parse_cidr(cidr).ok())
        .collect();

    // 先按 /24 归并
    struct Agg {
        hosts: u32,
        hits: u32,
        samples: Vec<String>,
    }
    let mut blocks: BTreeMap<u32, Agg> = BTreeMap::new();
    for host in hosts {
        let ip = match route::parse_ipv4(&host.ip) {
            Some(ip) => ip,
            None => continue,
        };
        let entry = blocks.entry(ip & 0xffff_ff00).or_insert(Agg {
            hosts: 0,
            hits: 0,
            samples: Vec::new(),
        });
        entry.hosts += 1;
        entry.hits = entry.hits.saturating_add(host.hits.max(1));
        entry.samples.push(host.ip.clone());
    }

    let mut out: Vec<LearnedSubnet> = Vec::new();

    // 第一层：每个观察到的 /24 一个候选
    for (key, agg) in &blocks {
        out.push(make_candidate(*key, 24, agg.hosts, agg.hits, &agg.samples, false, &rules));
    }

    // 第二层：同一个 /16 里出现多个 /24，说明整个 /16 大概率都是同一片内网
    let mut by16: BTreeMap<u32, Vec<&Agg>> = BTreeMap::new();
    for (key, agg) in &blocks {
        by16.entry(*key & 0xffff_0000).or_default().push(agg);
    }
    for (key, group) in &by16 {
        if group.len() < BROAD_16_MIN_BLOCKS {
            continue;
        }
        let hosts: u32 = group.iter().map(|a| a.hosts).sum();
        let hits: u32 = group.iter().map(|a| a.hits).sum();
        let samples: Vec<String> = group
            .iter()
            .flat_map(|a| a.samples.iter().cloned())
            .collect();
        out.push(make_candidate(*key, 16, hosts, hits, &samples, true, &rules));
    }

    // 第三层：同一个 /8 里出现多个 /16，才考虑整段交给 VPN。
    // 门槛设得比 /16 更严，因为汇聚出 /8 基本等于「把这一整片都送进公司网络」。
    let mut by8: BTreeMap<u32, usize> = BTreeMap::new();
    for key in by16.keys() {
        *by8.entry(*key & 0xff00_0000).or_insert(0) += 1;
    }
    for (key, count) in &by8 {
        if *count < BROAD_8_MIN_BLOCKS {
            continue;
        }
        let hosts: u32 = blocks
            .iter()
            .filter(|(k, _)| *k & 0xff00_0000 == *key)
            .map(|(_, a)| a.hosts)
            .sum();
        let hits: u32 = blocks
            .iter()
            .filter(|(k, _)| *k & 0xff00_0000 == *key)
            .map(|(_, a)| a.hits)
            .sum();
        let samples: Vec<String> = blocks
            .iter()
            .filter(|(k, _)| *k & 0xff00_0000 == *key)
            .flat_map(|(_, a)| a.samples.iter().cloned())
            .collect();
        out.push(make_candidate(*key, 8, hosts, hits, &samples, true, &rules));
    }

    // 未覆盖的排前面（这些才是用户要看的），其次精确的排在放大网段前面，
    // 再按命中次数从多到少
    out.sort_by(|a, b| {
        a.covered
            .cmp(&b.covered)
            .then(a.broad.cmp(&b.broad))
            .then(b.hits.cmp(&a.hits))
            .then(a.cidr.cmp(&b.cidr))
    });
    out
}

fn make_candidate(
    network: u32,
    prefix: u8,
    hosts: u32,
    hits: u32,
    samples: &[String],
    broad: bool,
    rules: &[(u32, u8)],
) -> LearnedSubnet {
    let cidr = format!("{}/{}", route::format_ipv4(network), prefix);
    let mut uniq: Vec<String> = samples.to_vec();
    uniq.sort();
    uniq.dedup();
    uniq.truncate(MAX_SAMPLES);
    LearnedSubnet {
        cidr,
        hosts,
        hits,
        broad,
        // 现有规则里有一条能把它整个包进去，才算已经照顾到
        covered: rules
            .iter()
            .any(|(net, len)| *len <= prefix && network & route::prefix_to_mask(*len) == *net),
        adopted: rules.iter().any(|(net, len)| *net == network && *len == prefix),
        samples: uniq,
    }
}

/// 给状态文件用的摘要
pub fn summarize(report: &LearnReport) -> LearnSummary {
    LearnSummary {
        enabled: report.enabled,
        scans: report.scans,
        hosts: report.hosts.len() as u32,
        candidates: report.candidates.len() as u32,
        uncovered: report
            .candidates
            .iter()
            .filter(|c| !c.covered && !c.adopted)
            .count() as u32,
        error: String::new(),
        updated_at: report.updated_at.clone(),
    }
}

fn push_unique_u16(list: &mut Vec<u16>, value: u16, cap: usize) {
    if value == 0 || list.contains(&value) || list.len() >= cap {
        return;
    }
    list.push(value);
    list.sort_unstable();
}

fn push_unique_string(list: &mut Vec<String>, value: &str, cap: usize) {
    if list.iter().any(|existing| existing == value) || list.len() >= cap {
        return;
    }
    list.push(value.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(ip: &str, port: u16, via_vpn: bool, process: &str) -> Observation {
        Observation {
            ip: ip.to_string(),
            remote_port: port,
            via_vpn,
            process: process.to_string(),
        }
    }

    fn raw(remote: &str, local: &str, port: u16, process: &str) -> RawConnection {
        RawConnection {
            remote_address: remote.to_string(),
            remote_port: port,
            local_address: local.to_string(),
            process_name: process.to_string(),
        }
    }

    #[test]
    fn classify_drops_noise_and_marks_tunnel_traffic() {
        let raw_rows = vec![
            raw("127.0.0.1", "127.0.0.1", 8080, "chrome"),
            raw("0.0.0.0", "10.0.0.5", 0, "chrome"),
            raw("169.254.10.10", "10.0.0.5", 445, "chrome"),
            raw("224.0.0.251", "10.0.0.5", 5353, "chrome"),
            raw("::1", "::1", 80, "chrome"),
            // 同一个局域网里的对端，不该被建议塞进 VPN
            raw("192.168.1.20", "192.168.1.10", 5432, "navicat"),
            // openconnect 自己到 VPN 服务器的连接
            raw("203.0.113.9", "192.168.1.10", 1218, "openconnect"),
            // 真正要看的：走隧道 / 走本地
            raw("10.10.1.7", "10.10.1.7", 22, "git"),
            raw("10.10.3.8", "192.168.1.10", 3306, "mysql"),
        ];
        let got = classify(
            &raw_rows,
            &["10.10.1.7".to_string()],
            &["192.168.1.0/24".to_string()],
            &["openconnect".to_string(), "ws-workbench.exe".to_string()],
        );
        assert_eq!(
            got,
            vec![
                obs("10.10.1.7", 22, true, "git"),
                obs("10.10.3.8", 3306, false, "mysql"),
            ]
        );
    }

    #[test]
    fn infer_groups_by_24_and_marks_coverage() {
        let hosts = vec![
            LearnedHost {
                ip: "10.10.1.7".into(),
                ports: vec![22],
                hits: 5,
                via_vpn: true,
                processes: vec!["git".into()],
                first_seen: "t0".into(),
                last_seen: "t1".into(),
            },
            // 同 /24 的第二台
            LearnedHost {
                ip: "10.10.1.9".into(),
                ports: vec![],
                hits: 2,
                via_vpn: true,
                processes: vec![],
                first_seen: "t0".into(),
                last_seen: "t1".into(),
            },
            LearnedHost {
                ip: "10.10.3.8".into(),
                ports: vec![],
                hits: 1,
                via_vpn: false,
                processes: vec![],
                first_seen: "t0".into(),
                last_seen: "t1".into(),
            },
        ];
        let got = infer(&hosts, &["10.10.0.0/16".to_string()]);
        let cidrs: Vec<&str> = got.iter().map(|c| c.cidr.as_str()).collect();
        assert!(cidrs.contains(&"10.10.1.0/24"));
        assert!(cidrs.contains(&"10.10.3.0/24"));
        // 两个 /24 已经是 10.10.0.0/16 的成员，规则表里那条能兜住它们
        for cand in &got {
            assert!(cand.covered, "{} 应被 10.10.0.0/16 覆盖", cand.cidr);
        }
        let exact = got.iter().find(|c| c.cidr == "10.10.1.0/24").unwrap();
        assert_eq!(exact.hosts, 2);
        assert_eq!(exact.hits, 7);
        assert!(!exact.broad);
        assert!(!exact.adopted);
    }

    #[test]
    fn infer_marks_adopted_only_on_exact_match() {
        let hosts = vec![LearnedHost {
            ip: "198.51.100.5".into(),
            ports: vec![],
            hits: 1,
            via_vpn: true,
            processes: vec![],
            first_seen: "t0".into(),
            last_seen: "t1".into(),
        }];
        let got = infer(&hosts, &["198.51.100.0/24".to_string()]);
        let cand = got.iter().find(|c| c.cidr == "198.51.100.0/24").unwrap();
        assert!(cand.adopted);
        assert!(cand.covered);

        // 规则表里只有更宽的那条时，不算「已采纳」，但算覆盖
        let got = infer(&hosts, &["198.51.100.0/24".to_string()]);
        let cand = got.iter().find(|c| c.cidr == "198.51.100.0/24").unwrap();
        assert!(!cand.adopted);
        assert!(cand.covered);

        // 完全没有规则时既不覆盖也不采纳 -> 排在最前面
        let got = infer(&hosts, &[]);
        assert_eq!(got[0].cidr, "198.51.100.0/24");
        assert!(!got[0].covered);
        assert!(!got[0].adopted);
    }

    #[test]
    fn infer_only_broadens_when_enough_blocks_seen() {
        let host = |ip: &str| LearnedHost {
            ip: ip.to_string(),
            ports: vec![],
            hits: 1,
            via_vpn: true,
            processes: vec![],
            first_seen: "t0".into(),
            last_seen: "t1".into(),
        };
        // 三个 /24：还不到汇聚 /16 的门槛（4 个）
        let three = vec![host("10.10.1.1"), host("10.10.2.1"), host("10.10.3.1")];
        assert!(!infer(&three, &[]).iter().any(|c| c.broad));

        // 四个 /24 落在同一个 /16 -> 多出一条放大候选
        let four = vec![
            host("10.10.1.1"),
            host("10.10.2.1"),
            host("10.10.3.1"),
            host("10.10.4.1"),
        ];
        let got = infer(&four, &[]);
        let broad: Vec<&LearnedSubnet> = got.iter().filter(|c| c.broad).collect();
        assert_eq!(broad.len(), 1);
        assert_eq!(broad[0].cidr, "10.10.0.0/16");
        assert_eq!(broad[0].hosts, 4);
        // 精确的 /24 排在前，放大网段排在后
        assert_eq!(got[0].cidr, "10.10.1.0/24");
    }

    #[test]
    fn merge_accumulates_hits_and_dedups_ports() {
        let mut report = LearnReport {
            enabled: true,
            ..Default::default()
        };
        merge(
            &mut report,
            &[obs("10.10.1.7", 22, true, "git"), obs("10.10.1.7", 22, true, "git")],
            &[],
            "t1",
        );
        assert_eq!(report.scans, 1);
        assert_eq!(report.hosts.len(), 1);
        let host = &report.hosts[0];
        assert_eq!(host.hits, 2);
        assert_eq!(host.ports, vec![22]);
        assert_eq!(host.processes, vec!["git".to_string()]);
        assert_eq!(host.first_seen, "t1");

        merge(
            &mut report,
            &[obs("10.10.1.7", 443, false, "chrome")],
            &[],
            "t2",
        );
        assert_eq!(report.scans, 2);
        assert_eq!(report.hosts.len(), 1);
        let host = &report.hosts[0];
        assert_eq!(host.hits, 3);
        assert_eq!(host.ports, vec![22, 443]);
        assert!(
            host.via_vpn,
            "有一条走了隧道就应记成走过隧道（或语义：至少一次）"
        );

        let summary = summarize(&report);
        assert_eq!(summary.hosts, 1);
        assert_eq!(summary.candidates, 1);
        assert_eq!(summary.uncovered, 1);
        assert_eq!(summary.scans, 2);
    }
}
