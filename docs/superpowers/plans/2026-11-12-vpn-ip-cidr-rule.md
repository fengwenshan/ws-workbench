# VPN IP 规则（恢复 `IP-CIDR`）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让规则模式的 `IP-CIDR` 规则（单 IP / 网段、VPN / DIRECT）真正注入隧道路由，内网网站按 IP 访问也能走 VPN，同时覆盖校验、预览、热更新、断开还原与崩溃清理。

**Architecture:** 规则编译成 `SplitPlan { domains, ip_rules }`；`ip_route_plan()` 纯函数把 IP 规则翻译成「隧道网段 + 直连例外」；`SplitTunnel` 把隧道网段并入现有 `static_routes` 维护，直连例外走原默认网关并记 `ip-routes.txt` 账本；macOS 与系统同前缀路由冲突时用 `route change` 覆盖、断开还原，Windows 用同前缀 + metric 1 新增、只删自己的那条。纯 IP 规则不启动 DNS 转发器/解析器。

**Tech Stack:** Rust（Tauri 2 / `src-tauri`）、Vue 3 + TypeScript、macOS `route`/`netstat`、Windows `Get-NetRoute`/`New-NetRoute`。

---

## 执行约定

- 工作目录固定为仓库根 `/Users/fengwenshan/wenshan/demo/layui-admin-build`。
- Rust 命令统一用 `cargo test --manifest-path src-tauri/Cargo.toml <过滤>`（首次编译要几分钟，后续增量很快）。
- 工作区里有大量**与本计划无关的未提交改动**：每个任务只 `git add` 本任务说到的文件，**禁止 `git add -A` / `git add .`**。
- 每个任务按「先写失败测试 → 跑一遍确认失败 → 最小实现 → 跑通 → 提交」推进；测试名就是计划里给的函数名。
- 真机验收（Task 10）需要连接 VPN 且 worker 有 root，由人执行；Agent 只负责代码与自动化测试。

## 文件结构（本次改动）

| 文件 | 职责 |
| --- | --- |
| `src-tauri/src/vpn/rule_engine.rs` | 规则类型解析：新增 `RuleKind::IpCidr` |
| `src-tauri/src/vpn/split.rs` | IP 规则模型、计划编译、路由编译、`SplitTunnel` 接线、`ip-routes.txt` 账本、清理 |
| `src-tauri/src/vpn/route.rs` | macOS 路由查询/改写/还原的小函数与 netstat 解析 |
| `src-tauri/src/vpn/commands.rs` | 规则校验、`vpn_validate_rule` 归一值、预览判定与参数改名 |
| `src-tauri/src/vpn/worker.rs` | 连接路径传入网络快照、日志与状态文案 |
| `src/types/vpn.ts` | `VpnRuleKind` 增加 `'IP-CIDR'`、退役名单移除、类型标签 |
| `src/components/vpn/VpnRuleDialog.vue` | 类型下拉、占位符、IP 写成域名的提示 |
| `src/composables/useVpn.ts` | 预览接口参数 `domain` → `target` |
| `README.md` | 规则类型与语义说明 |

---

### Task 1: 恢复 `RuleKind::IpCidr`

**Files:**
- Modify: `src-tauri/src/vpn/rule_engine.rs:1-50`、`:83-104`
- Test: `src-tauri/src/vpn/rule_engine.rs`（文件内 `mod tests`）

- [ ] **Step 1: 写失败测试**

把 `parses_the_two_live_kinds` 换成：

```rust
    #[test]
    fn parses_the_live_kinds() {
        assert_eq!(RuleKind::parse("DOMAIN").unwrap(), RuleKind::Domain);
        assert_eq!(
            RuleKind::parse("domain-suffix").unwrap(),
            RuleKind::DomainSuffix
        );
        assert_eq!(RuleKind::parse("ip-cidr").unwrap(), RuleKind::IpCidr);
        assert_eq!(RuleKind::IpCidr.as_str(), "IP-CIDR");
    }
```

并把 `retired_kinds_fail_with_actionable_message` 里的 `("IP-CIDR", "域名"),` 一行删掉。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::rule_engine`
Expected: 编译失败，报 `no variant or associated item named 'IpCidr'`

- [ ] **Step 3: 实现**

文件头注释改成：

```rust
//! 规则类型的解析与规范化（纯逻辑，好测）。
//!
//! 这里保留**真正能生效**的三类：域名精确、域名后缀、IPv4 网段。
//! 早期的 Clash 式规则（`DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `RULE-SET`）
//! 需要「按连接抓包 + 按进程/端口匹配」的数据面，macOS 侧已经改成路由注入
//! （见 `split.rs` / `fulltunnel.rs`），那套匹配器没有落点了，一并删掉。
//!
//! `IP-CIDR` 于 2026-11 重新上线：路由注入现在把命中的网段直接装成隧道路由，
//! 直连例外用更具体的直连路由表达（见 `split::ip_route_plan`）。
```

枚举加一个变体：

```rust
pub enum RuleKind {
    /// 精确域名：`oa.example.com`
    Domain,
    /// 域名后缀：`example.com`（DNS 上含子域；路由注入只解析字面域名本身）
    DomainSuffix,
    /// IPv4 单个地址或网段：`192.168.6.115` / `192.168.6.0/24`
    IpCidr,
}
```

`parse` 里把旧的报错分支换成成功分支：

```rust
            "IP-CIDR" => Ok(RuleKind::IpCidr),
```

`as_str` 加：

```rust
            RuleKind::IpCidr => "IP-CIDR",
```

- [ ] **Step 4: 确认 RED 证据（本任务单独无法变绿）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::rule_engine`
Expected: 编译失败 `error[E0004]: non-exhaustive patterns: RuleKind::IpCidr not covered`
（`split.rs:plan` 与 `commands.rs:validate_rule_input` 各一处）。这是预期的：
Task 2 会同时补上这两个 match 分支，代码在 Task 2 结束时恢复绿色。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/rule_engine.rs
git commit -m "feat(vpn): 恢复 IP-CIDR 规则类型解析"
```

---

### Task 2: 计划编译（`IpRule` / `normalize_ip_rule` / `ip_tunnels`）

**Files:**
- Modify: `src-tauri/src/vpn/split.rs`（`DomainRule` 之后、`plan()` 与 `SplitPlan`）
- Modify: `src-tauri/src/vpn/commands.rs:289-303`（`validate_rule_input` 补 `IpCidr` 分支，
  否则 Task 1 引入的 `E0004` 无法消除）
- Test: `src-tauri/src/vpn/split.rs` 的 `mod tests`

- [ ] **Step 1: 写失败测试**

在 `split.rs` 的 `mod tests` 里加：

```rust
    #[test]
    fn normalize_ip_rule_accepts_hosts_and_cidrs() {
        assert_eq!(
            normalize_ip_rule(" 192.168.6.115 ").unwrap(),
            "192.168.6.115/32"
        );
        assert_eq!(normalize_ip_rule("192.168.6.77/24").unwrap(), "192.168.6.0/24");
        assert!(normalize_ip_rule("0.0.0.0/0")
            .unwrap_err()
            .contains("全局模式"));
        assert!(normalize_ip_rule("192.168.6.0/33").is_err());
        assert!(normalize_ip_rule("not-an-ip").is_err());
        assert!(normalize_ip_rule("").is_err());
    }

    #[test]
    fn plan_compiles_ip_rules() {
        let rules = vec![
            rule("IP-CIDR", "192.168.6.115", "VPN"),
            rule("IP-CIDR", "192.168.6.77/24", "VPN"),
            rule("IP-CIDR", "192.168.6.9/32", "DIRECT"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert!(plan.domains.is_empty());
        let cidrs: Vec<String> = plan.ip_rules.iter().map(|item| item.cidr()).collect();
        assert_eq!(
            cidrs,
            vec!["192.168.6.115/32", "192.168.6.0/24", "192.168.6.9/32"]
        );
        assert!(plan.ip_tunnels("192.168.6.115".parse().unwrap()));
        assert!(
            !plan.ip_tunnels("192.168.6.9".parse().unwrap()),
            "更具体的 DIRECT 例外要盖住 /24"
        );
        assert!(plan.ip_tunnels("192.168.6.10".parse().unwrap()));
        assert!(!plan.ip_tunnels("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn invalid_ip_rules_are_reported_not_fatal() {
        let rules = vec![
            rule("IP-CIDR", "0.0.0.0/0", "VPN"),
            rule("IP-CIDR", "192.168.6.0/33", "VPN"),
            rule("IP-CIDR", "hello", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert!(plan.ip_rules.is_empty());
        assert_eq!(plan.notes.len(), 3, "{:?}", plan.notes);
        assert!(plan.notes.iter().any(|note| note.contains("全局模式")));
    }

    #[test]
    fn ip_rule_ties_keep_the_first_one() {
        let rules = vec![
            rule("IP-CIDR", "10.0.0.0/24", "VPN"),
            rule("IP-CIDR", "10.0.0.0/24", "DIRECT"),
            rule("IP-CIDR", "10.0.0.0/16", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        assert!(
            plan.ip_tunnels("10.0.0.5".parse().unwrap()),
            "同前缀先出现的 VPN 胜出"
        );
        assert!(
            plan.ip_tunnels("10.0.5.5".parse().unwrap()),
            "/16 仍在，按最长前缀判定"
        );
    }
```

同时删掉两个旧测试（`IP-CIDR` 已不再是「已下线」）：

- `retired_ip_cidr_is_reported` 整个函数删除；
- `plan_explains_unsupported_kinds` 里删掉 `rule("IP-CIDR", "10.10.3.0/24", "VPN"),` 这一行，并把 `assert_eq!(plan.notes.len(), 4, ...)` 改成 3。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::split::tests`
Expected: 编译失败，`cannot find function normalize_ip_rule` / `no field ip_rules`

- [ ] **Step 3: 实现模型与编译**

在 `DomainRule` 定义之后加：

```rust
/// 一条 IP 规则（IPv4）的数据面语义：网络地址 + 前缀长度 + 走向。
///
/// `network` 已经按前缀抹掉了主机位（`192.168.6.77/24` → `192.168.6.0/24`），
/// 判定时按「最长前缀优先、等长先出现者胜」比较即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRule {
    pub network: u32,
    pub prefix: u8,
    pub policy: Policy,
}

impl IpRule {
    /// 归一后的 CIDR 文本，例如 `192.168.6.0/24`。
    pub fn cidr(&self) -> String {
        format!("{}/{}", route::format_ipv4(self.network), self.prefix)
    }

    fn contains(&self, ip: u32) -> bool {
        ip & route::prefix_to_mask(self.prefix) == self.network
    }
}
```

`SplitPlan` 加字段：

```rust
    /// IP 规则全集（走 VPN 的 + 直连例外），已归一、按用户规则顺序保留
    pub ip_rules: Vec<IpRule>,
```

`SplitPlan` 的 `impl` 里加：

```rust
    /// 一个 IPv4 地址最终走不走隧道。
    ///
    /// 与域名规则同一套语义：最长前缀优先；前缀长度相同时先出现的规则胜出；
    /// 没有任何 IP 规则命中时走直连（兜底策略不参与路由注入）。
    pub fn ip_tunnels(&self, ip: std::net::Ipv4Addr) -> bool {
        let target = u32::from(ip);
        let mut best: Option<(u8, Policy)> = None;
        for rule in &self.ip_rules {
            if !rule.contains(target) {
                continue;
            }
            match best {
                // `>` 才替换：等长时先出现的已记录，保持先到先得
                Some((best_prefix, _)) if rule.prefix <= best_prefix => {}
                _ => best = Some((rule.prefix, rule.policy)),
            }
        }
        matches!(best, Some((_, Policy::Vpn)))
    }
```

在 `normalize_domain` 函数**之前**加：

```rust
/// IP 规则取值归一：单个地址补 `/32`，CIDR 抹掉主机位，拒绝 `/0` 与非法值。
///
/// 校验（`commands::validate_rule_input`）与计划编译（[`plan`]）共用它，
/// 避免「界面校验通过、连接时被跳过」两套逻辑。
pub fn normalize_ip_rule(value: &str) -> Result<String, String> {
    let text = value.trim();
    if text.is_empty() {
        return Err("匹配值不能为空".to_string());
    }
    if !text.contains('/') {
        let ip = route::parse_ipv4(text)
            .ok_or_else(|| format!("不是合法的 IP 或网段：{}", value))?;
        return Ok(format!("{}/32", route::format_ipv4(ip)));
    }
    let (network, prefix) =
        route::parse_cidr(text).map_err(|e| format!("不是合法的 IP 或网段：{}", e))?;
    if prefix == 0 {
        return Err(
            "IP 规则不支持 0.0.0.0/0，需要全部流量走 VPN 请用全局模式".to_string(),
        );
    }
    Ok(format!("{}/{}", route::format_ipv4(network), prefix))
}
```

`plan()` 的 `match kind { ... }` 里加分支（放在 Domain 分支之后）：

```rust
            RuleKind::IpCidr => match normalize_ip_rule(&rule.value) {
                Ok(cidr) => {
                    let (network, prefix) =
                        route::parse_cidr(&cidr).expect("归一后的 CIDR 一定合法");
                    let duplicate = out.ip_rules.iter().any(|item| {
                        item.network == network
                            && item.prefix == prefix
                            && item.policy == policy
                    });
                    if !duplicate {
                        out.ip_rules.push(IpRule {
                            network,
                            prefix,
                            policy,
                        });
                    }
                }
                Err(e) => push_unique(
                    &mut out.notes,
                    format!("IP-CIDR {}：{}", rule.value, e),
                ),
            },
```

并把 `plan()` 上方文档里「`IP-CIDR` 规则已下线……」那段改成：

```rust
/// IP 规则（`IP-CIDR`）编译成 [`IpRule`]：命中网段由 `SplitTunnel` 装成隧道路由，
/// 「直连」规则作为例外交给 [`ip_route_plan`] 决定是否需要装更具体的直连路由。
```

同时在 `src-tauri/src/vpn/commands.rs` 的 `validate_rule_input` 里补上分支
（消除 Task 1 引入的 `E0004`；Task 6 只在此基础上改预览与归一返回）：

```rust
        RuleKind::IpCidr => {
            crate::vpn::split::normalize_ip_rule(value)?;
        }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::split::tests`
Expected: `test result: ok`

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/split.rs src-tauri/src/vpn/commands.rs
git commit -m "feat(vpn): IP 规则编译成 SplitPlan.ip_rules"
```

---

### Task 3: `ip_route_plan` 纯函数（隧道网段 + 直连例外 + 说明）

**Files:**
- Modify: `src-tauri/src/vpn/split.rs`（`plan()` 之后新增结构体与函数）
- Test: `src-tauri/src/vpn/split.rs` 的 `mod tests`

- [ ] **Step 1: 写失败测试**

```rust
    fn snapshot_with_gateway() -> crate::vpn::models::NetworkSnapshot {
        crate::vpn::models::NetworkSnapshot {
            default_gateway: "192.168.1.1".into(),
            default_if_index: 0,
            default_if_alias: "en0".into(),
            local_subnets: vec!["192.168.1.0/24".into()],
        }
    }

    #[test]
    fn ip_route_plan_splits_tunnel_and_direct() {
        let rules = vec![
            rule("IP-CIDR", "192.168.6.0/24", "VPN"),
            rule("IP-CIDR", "192.168.6.9/32", "DIRECT"),
            rule("IP-CIDR", "10.0.0.0/8", "VPN"),
            rule("IP-CIDR", "10.1.0.0/16", "DIRECT"),
            rule("IP-CIDR", "172.16.0.0/12", "DIRECT"),
        ];
        let plan = plan(&rules, "DIRECT");
        let routes = ip_route_plan(&plan, &snapshot_with_gateway(), &[], &[]);
        assert_eq!(routes.tunnel, vec!["192.168.6.0/24", "10.0.0.0/8"]);
        assert_eq!(
            routes.direct,
            vec!["192.168.6.9/32", "10.1.0.0/16"],
            "更宽的 DIRECT 不需要表项"
        );
    }

    #[test]
    fn ip_route_plan_skips_server_and_vpn_dns_hosts() {
        let server: Ipv4Addr = "203.0.113.10".parse().unwrap();
        let dns: Ipv4Addr = "223.5.5.5".parse().unwrap();
        let rules = vec![
            rule("IP-CIDR", "203.0.113.10/32", "VPN"),
            rule("IP-CIDR", "223.5.5.5/32", "VPN"),
            rule("IP-CIDR", "192.168.6.0/24", "VPN"),
        ];
        let plan = plan(&rules, "DIRECT");
        let routes = ip_route_plan(&plan, &snapshot_with_gateway(), &[server], &[dns]);
        assert_eq!(routes.tunnel, vec!["192.168.6.0/24"]);
        assert_eq!(routes.notes.len(), 2, "{:?}", routes.notes);
    }

    #[test]
    fn ip_route_plan_keeps_explicit_local_subnet_rules() {
        let rules = vec![
            rule("IP-CIDR", "192.168.1.0/24", "VPN"),
            rule("IP-CIDR", "192.168.1.9/32", "DIRECT"),
        ];
        let plan = plan(&rules, "DIRECT");
        let routes = ip_route_plan(&plan, &snapshot_with_gateway(), &[], &[]);
        assert_eq!(
            routes.tunnel,
            vec!["192.168.1.0/24"],
            "显式配置不因本机网段被跳过"
        );
        assert_eq!(routes.direct, vec!["192.168.1.9/32"]);
    }

    #[test]
    fn ip_route_plan_reports_missing_gateway() {
        let rules = vec![
            rule("IP-CIDR", "192.168.6.0/24", "VPN"),
            rule("IP-CIDR", "192.168.6.9/32", "DIRECT"),
        ];
        let plan = plan(&rules, "DIRECT");
        let mut snapshot = snapshot_with_gateway();
        snapshot.default_gateway = String::new();
        let routes = ip_route_plan(&plan, &snapshot, &[], &[]);
        assert_eq!(routes.tunnel, vec!["192.168.6.0/24"]);
        assert!(routes.direct.is_empty());
        assert!(
            routes.notes.iter().any(|note| note.contains("默认网关")),
            "{:?}",
            routes.notes
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::split::tests::ip_route_plan`
Expected: 编译失败，`cannot find function ip_route_plan`

- [ ] **Step 3: 实现**

在 `plan()` 函数之后加：

```rust
/// `ip_route_plan` 的产物：装到隧道上的网段、装到原网关上的直连例外、说明。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IpRoutePlan {
    /// 要装到隧道接口上的网段（含 `/32`）
    pub tunnel: Vec<String>,
    /// 要装到原默认网关上的直连例外网段（比覆盖它的 VPN 网段更具体）
    pub direct: Vec<String>,
    pub notes: Vec<String>,
}

/// 把 IP 规则编译成「要装哪些路由」。
///
/// 纯函数：不碰系统路由表、不需要 root，边界可以直接单测。
///
/// - 同 `network` + `prefix` 的规则先出现者胜出（与 [`SplitPlan::ip_tunnels`] 一致），
///   走向冲突时写一条 `notes`；
/// - 只跳过两类硬冲突：规则精确命中 VPN 服务器 IP 的 `/32`（会把隧道自己的
///   TLS/DTLS 流量送回隧道，立即打环）与 VPN DNS 的 `/32`（同目标重复）；
/// - **不判断「是不是本机局域网」**：显式配置一律保留；
/// - `DIRECT` 只有比覆盖它的 VPN 网段更具体时才需要一条直连路由，否则结论本来
///   就是直连，不产生无意义的表项。
pub fn ip_route_plan(
    plan: &SplitPlan,
    snapshot: &super::models::NetworkSnapshot,
    server_ips: &[Ipv4Addr],
    vpn_dns: &[Ipv4Addr],
) -> IpRoutePlan {
    let mut out = IpRoutePlan::default();

    // 同前缀先到先得；后出现的只提示一次走向冲突
    let mut effective: Vec<IpRule> = Vec::new();
    let mut seen: BTreeMap<(u32, u8), Policy> = BTreeMap::new();
    for rule in &plan.ip_rules {
        let key = (rule.network, rule.prefix);
        if let Some(first) = seen.get(&key) {
            if *first != rule.policy {
                push_unique(
                    &mut out.notes,
                    format!(
                        "{} 同时配置了 VPN 与 DIRECT，先出现的 {} 生效",
                        rule.cidr(),
                        first.as_str()
                    ),
                );
            }
            continue;
        }
        seen.insert(key, rule.policy);
        effective.push(*rule);
    }

    let mut usable: Vec<IpRule> = Vec::new();
    for rule in effective {
        if rule.prefix == 32 {
            let ip = Ipv4Addr::from(rule.network);
            if server_ips.contains(&ip) {
                push_unique(
                    &mut out.notes,
                    format!(
                        "{} 是 VPN 服务器地址，已跳过（否则隧道会自己抓自己）",
                        rule.cidr()
                    ),
                );
                continue;
            }
            if vpn_dns.contains(&ip) {
                push_unique(
                    &mut out.notes,
                    format!("{} 已经是 VPN DNS 的固定 /32 路由，不重复安装", rule.cidr()),
                );
                continue;
            }
        }
        usable.push(rule);
    }

    for rule in &usable {
        if matches!(rule.policy, Policy::Vpn) {
            push_unique(&mut out.tunnel, rule.cidr());
        }
    }

    let vpn_rules: Vec<&IpRule> = usable
        .iter()
        .filter(|rule| matches!(rule.policy, Policy::Vpn))
        .collect();
    let mut direct: Vec<String> = Vec::new();
    for rule in usable
        .iter()
        .filter(|rule| matches!(rule.policy, Policy::Direct))
    {
        // 只有「更具体的直连」才能压过隧道；更宽的直连写出去也只是回到默认路由
        let needed = vpn_rules.iter().any(|vpn| {
            vpn.prefix < rule.prefix
                && (rule.network & route::prefix_to_mask(vpn.prefix)) == vpn.network
        });
        if needed {
            push_unique(&mut direct, rule.cidr());
        }
    }

    if direct.is_empty() {
        return out;
    }
    if snapshot.default_gateway.trim().is_empty() {
        for cidr in direct {
            push_unique(
                &mut out.notes,
                format!("{} 需要直连路由，但本机没有可用的默认网关，已跳过", cidr),
            );
        }
        return out;
    }
    out.direct = direct;
    out
}
```

顺手修掉 `split.rs` 模块文档里已经过时的「代价」条目（第 31 行）：

```rust
//! - 网段（`IP-CIDR`）规则按最长前缀注入：只能整段进隧道，需要排除就用更具体的
//!   DIRECT 规则表达（例如 `192.168.6.0/24 → VPN` + `192.168.6.9/32 → DIRECT`）；
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::split::tests`
Expected: `test result: ok`

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/split.rs
git commit -m "feat(vpn): ip_route_plan 编译 IP 规则为隧道路由与直连例外"
```

---

### Task 4: macOS 路由查询 / 覆盖 / 还原

**Files:**
- Modify: `src-tauri/src/vpn/route.rs`（`RouteTarget` 之后、`route_exists` 附近）
- Test: `src-tauri/src/vpn/route.rs` 的 `mod tests`

- [ ] **Step 1: 写失败测试**

在 `route.rs` 的 `mod tests` 里加（纯解析，任何平台都能跑）：

```rust
    #[cfg(target_os = "macos")]
    #[test]
    fn parses_netstat_route_entries() {
        let text = "Destination        Gateway            Flags        Netif Expire
default            192.168.1.1        UGScg          en0
127                127.0.0.1          UCS            lo0
192.168.1          link#12            UCSIg          en0
192.168.6.115      192.168.137.1      UGSc           utun4
";
        let entries = parse_netstat_route_entries(text);
        let local = entries
            .iter()
            .find(|entry| entry.cidr() == "192.168.1.0/24")
            .expect("应解析出本机网段");
        assert_eq!(local.if_name, "en0");
        assert!(!local.via_gateway(), "link# 不是网关地址");

        let host = entries
            .iter()
            .find(|entry| entry.cidr() == "192.168.6.115/32")
            .expect("应解析出 /32 主机路由");
        assert_eq!(host.if_name, "utun4");
        assert!(host.via_gateway());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::route::tests::parses_netstat_route_entries`
Expected: 编译失败，`cannot find function parse_netstat_route_entries`

- [ ] **Step 3: 实现**

在 `RouteTarget` 的 `impl` 之后加：

```rust
/// `netstat -rn -f inet` 里的一条路由（macOS 用；Windows 不需要它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    /// 网络地址（已按前缀抹掉主机位）
    pub network: u32,
    pub prefix: u8,
    /// 原始网关列：`link#12`、`192.168.1.1` 之类；直连（on-link）时不是 IP
    pub gateway: String,
    /// 出口接口名（netstat 的 Netif 列）
    pub if_name: String,
}

impl RouteEntry {
    pub fn cidr(&self) -> String {
        format!("{}/{}", format_ipv4(self.network), self.prefix)
    }

    /// 原路由是否经网关发出（false = on-link，直接走接口）
    pub fn via_gateway(&self) -> bool {
        parse_ipv4(&self.gateway).is_some()
    }
}

/// 解析 `netstat -rn -f inet` 正文。
///
/// 只能 macOS 编译：它复用了 mac 专用的 [`parse_netstat_destination`]（缩写反推）。
/// Windows 按前缀新增路由、不需要改写/还原，也就用不到这个解析。
#[cfg(target_os = "macos")]
pub fn parse_netstat_route_entries(text: &str) -> Vec<RouteEntry> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let destination = parts.next()?;
            let gateway = parts.next()?;
            let _flags = parts.next()?;
            let if_name = parts.next().unwrap_or("");
            let (network, prefix) = parse_netstat_destination(destination)?;
            Some(RouteEntry {
                network,
                prefix,
                gateway: gateway.to_string(),
                if_name: if_name.to_string(),
            })
        })
        .collect()
}

/// 查一条已经存在的路由（按目标网段精确匹配，不管出口在哪）。
#[cfg(target_os = "macos")]
pub fn lookup_route(cidr: &str) -> Result<Option<RouteEntry>, String> {
    use crate::process::run_with_timeout;
    use std::path::Path;
    use std::time::Duration;

    let (network, prefix) = parse_cidr(cidr)?;
    let (text, _) = run_with_timeout(
        Path::new("/usr/sbin/netstat"),
        &["-rn", "-f", "inet"],
        Duration::from_secs(15),
    );
    Ok(parse_netstat_route_entries(&text)
        .into_iter()
        .find(|entry| entry.network == network && entry.prefix == prefix))
}

/// 把一条已存在的路由就地改到指定接口（显式规则覆盖本机网段时用）。
#[cfg(target_os = "macos")]
pub fn change_route_to_interface(cidr: &str, if_name: &str) -> Result<(), String> {
    let cidr = normalize_cidr(cidr)?;
    let args: Vec<String> = vec![
        "-n".into(),
        "change".into(),
        "-net".into(),
        cidr,
        "-interface".into(),
        if_name.to_string(),
    ];
    run_route_macos(&args, &["not in table", "No such process", "no such process"])
}

/// 还原一条被我们改写过的路由（macOS）。
///
/// 先 `route change` 改回原出口；路由已经不存在（接口消失、被别的程序删掉）就
/// 按原出口重新加一条。
#[cfg(target_os = "macos")]
pub fn restore_route(entry: &RouteEntry) -> Result<(), String> {
    let cidr = entry.cidr();
    let mut args: Vec<String> = vec!["-n".into(), "change".into(), "-net".into(), cidr.clone()];
    if entry.via_gateway() {
        args.push(entry.gateway.clone());
    } else if !entry.if_name.is_empty() {
        args.push("-interface".into());
        args.push(entry.if_name.clone());
    } else {
        return Ok(());
    }
    if run_route_macos(&args, &["not in table", "No such process"]).is_ok() {
        return Ok(());
    }
    // on-link 路由的 gateway 列是 `link#12` 这种接口标记，不能当下一跳地址用；
    // 传空 next_hop 让 add_route 走 `-interface` 形式。
    let next_hop = if entry.via_gateway() {
        entry.gateway.clone()
    } else {
        String::new()
    };
    let target = RouteTarget::new(0, &entry.if_name, &next_hop);
    add_route(&cidr, &target, 0)
}

#[cfg(target_os = "windows")]
pub fn restore_route(_entry: &RouteEntry) -> Result<(), String> {
    // Windows 的覆盖是「同前缀新增一条」，断开时只删我们自己那条，
    // 系统原路由自然恢复，不需要改写/还原。
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn restore_route(_entry: &RouteEntry) -> Result<(), String> {
    Ok(())
}
```

注意：`fallback` 里 `add_route(&cidr, &target, 0)` 在 macOS 上 `next_hop` 为空会走
`-interface` 形式，正好还原 on-link 路由。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::route::tests`
Expected: `test result: ok`

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/route.rs
git commit -m "feat(vpn): macOS 路由查询/覆盖/还原与 netstat 解析"
```

---

### Task 5: `SplitTunnel` 接线与 `ip-routes.txt` 账本

**Files:**
- Modify: `src-tauri/src/vpn/split.rs`（`SplitTunnel` 结构体、`apply`、`install_routes`、`guard`、`teardown`、`routes_for_status`、`cleanup_stale`）
- Modify: `src-tauri/src/vpn/worker.rs`（只改 `SplitTunnel::apply` 调用参数，日志文案留到 Task 7）
- Test: `src-tauri/src/vpn/split.rs` 的 `mod tests`

- [ ] **Step 1: 写失败测试（账本编解码）**

```rust
    #[test]
    fn route_ledger_round_trips() {
        let entries = vec![
            RouteLedgerEntry::Direct {
                cidr: "192.168.6.9/32".into(),
                gateway: "192.168.1.1".into(),
                if_index: 0,
                if_name: "en0".into(),
            },
            RouteLedgerEntry::Override {
                cidr: "192.168.1.0/24".into(),
                orig: route::RouteEntry {
                    network: route::parse_ipv4("192.168.1.0").unwrap(),
                    prefix: 24,
                    gateway: "link#12".into(),
                    if_name: "en0".into(),
                },
            },
        ];
        let text = encode_ledger(&entries);
        assert_eq!(parse_ledger(&text), entries);
        assert!(parse_ledger("garbage\n\n").is_empty());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::split::tests::route_ledger`
Expected: 编译失败，`cannot find type RouteLedgerEntry`

- [ ] **Step 3: 实现账本类型**

在 `ip_route_plan` 之后加：

```rust
/// 本次会话动过的路由账本（会话目录下的文件名）。
pub const IP_ROUTES_LEDGER: &str = "ip-routes.txt";

/// 路由账本条目：断开时按它删除或还原。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteLedgerEntry {
    /// 我们新建的直连例外（走原默认网关），断开删掉即可
    Direct {
        cidr: String,
        gateway: String,
        if_index: u32,
        if_name: String,
    },
    /// 我们改写过的系统路由（macOS），断开要还原原出口
    Override { cidr: String, orig: route::RouteEntry },
}

impl RouteLedgerEntry {
    fn encode(&self) -> String {
        match self {
            RouteLedgerEntry::Direct {
                cidr,
                gateway,
                if_index,
                if_name,
            } => format!("direct|{}|{}|{}|{}", cidr, gateway, if_index, if_name),
            RouteLedgerEntry::Override { cidr, orig } => format!(
                "override|{}|{}|{}|{}|{}",
                cidr,
                route::format_ipv4(orig.network),
                orig.prefix,
                orig.gateway,
                orig.if_name
            ),
        }
    }
}

pub fn encode_ledger(entries: &[RouteLedgerEntry]) -> String {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&entry.encode());
        text.push('\n');
    }
    text
}

/// 解析账本；不认识的行直接跳过（旧版本写的账本、手改坏的账本都不能让连接失败）。
pub fn parse_ledger(text: &str) -> Vec<RouteLedgerEntry> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim().split('|').collect();
            match parts.as_slice() {
                ["direct", cidr, gateway, if_index, if_name] => {
                    Some(RouteLedgerEntry::Direct {
                        cidr: (*cidr).to_string(),
                        gateway: (*gateway).to_string(),
                        if_index: if_index.parse().unwrap_or(0),
                        if_name: (*if_name).to_string(),
                    })
                }
                ["override", cidr, network, prefix, gateway, if_name] => {
                    Some(RouteLedgerEntry::Override {
                        cidr: (*cidr).to_string(),
                        orig: route::RouteEntry {
                            network: route::parse_ipv4(network)?,
                            prefix: prefix.parse().ok()?,
                            gateway: (*gateway).to_string(),
                            if_name: (*if_name).to_string(),
                        },
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// 按账本清理上一次会话留下的路由：直连例外删掉，覆盖的系统路由还原。
fn cleanup_ledger(session: &VpnSession) {
    let path = session.dir.join(IP_ROUTES_LEDGER);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    for entry in parse_ledger(&text) {
        match entry {
            RouteLedgerEntry::Direct {
                cidr,
                gateway,
                if_index,
                if_name,
            } => {
                let target = route::RouteTarget::new(if_index, &if_name, &gateway);
                let _ = route::delete_route(&cidr, &target);
            }
            RouteLedgerEntry::Override { orig, .. } => {
                let _ = route::restore_route(&orig);
            }
        }
    }
    let _ = std::fs::remove_file(&path);
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::split::tests::route_ledger`
Expected: PASS

- [ ] **Step 5: 结构体加字段并初始化**

`SplitTunnel` 在 `static_routes` 字段之后加：

```rust
    /// 来自 IP 规则的隧道路由（与 VPN DNS 的 /32 一起放在 `static_routes` 里维护）
    ip_routes: Vec<String>,
    /// 我们新建的直连例外：(cidr, 出口)
    direct_routes: Vec<(String, route::RouteTarget)>,
    /// 我们改写过的系统路由（macOS）
    overrides: Vec<route::RouteEntry>,
    /// 会话目录里的路由账本
    ledger: std::path::PathBuf,
```

`apply` 的 `let mut me = SplitTunnel { ... }` 里加：

```rust
            ip_routes: Vec::new(),
            direct_routes: Vec::new(),
            overrides: Vec::new(),
            ledger: session.dir.join(IP_ROUTES_LEDGER),
```

- [ ] **Step 6: 改 `apply` 的签名与前置判断**

签名加 `snapshot: &super::models::NetworkSnapshot`（放在 `server_ips` 之后、`memo` 之前）：

```rust
    pub fn apply(
        session: &VpnSession,
        tun_dev: &str,
        tun_index: u32,
        vpn_dns: &[Ipv4Addr],
        server_ips: &[Ipv4Addr],
        snapshot: &super::models::NetworkSnapshot,
        memo: DnsMemo,
        plan: SplitPlan,
    ) -> Result<Self, String> {
```

原早退条件：

```rust
        if plan.domains.is_empty() {
            return Err("规则里没有可用的分流动作：请加一条「域名后缀 → 走 VPN」".to_string());
        }
```

换成：

```rust
        if plan.domains.is_empty() && plan.ip_rules.is_empty() {
            return Err(
                "规则里没有可用的分流动作：请加一条「域名后缀」或「IP / 网段」规则"
                    .to_string(),
            );
        }
```

在 `if vpn_dns.is_empty()` 之前加 `let needs_dns = !plan.domains.is_empty();`，
把该判断改成：

```rust
        if needs_dns && vpn_dns.is_empty() {
            return Err("服务端没有下发 DNS 地址，内网域名无法解析".to_string());
        }
```

在构造 `me` **之前**编译路由计划并记录日志：

```rust
        let ip_plan = ip_route_plan(&plan, snapshot, server_ips, vpn_dns);
        for note in &ip_plan.notes {
            session.append_log(&format!("[分流] {}", note));
        }
```

把 `if let Err(e) = me.install_routes() { ... }` 换成：

```rust
        if let Err(e) = me.install_routes(&ip_plan.tunnel, &ip_plan.direct, session, snapshot) {
            me.teardown();
            return Err(e);
        }
```

把 `install_resolver` + `start_resolver` 两段包进 `if needs_dns { ... }`（保持原来的
`me.teardown(); return Err(e);` 逻辑）。

最后 `flush_dns_cache(); Ok(me)` 不变。

- [ ] **Step 7: 重写 `install_routes` 并新增单条安装/写账本**

整个 `install_routes` 函数替换为：

```rust
    /// 装固定路由：VPN DNS 的 `/32`（失败致命）+ IP 规则的隧道网段（单条失败继续）。
    fn install_routes(
        &mut self,
        ip_tunnel: &[String],
        ip_direct: &[String],
        session: &VpnSession,
        snapshot: &super::models::NetworkSnapshot,
    ) -> Result<(), String> {
        for ip in &self.vpn_dns {
            let cidr = format!("{}/32", ip);
            add_tunnel_route(&cidr, &self.tun_dev, self.tun_index())?;
            if !self.static_routes.contains(&cidr) {
                self.static_routes.push(cidr);
            }
        }

        for cidr in ip_tunnel {
            match self.install_ip_tunnel_route(cidr, session) {
                Ok(()) => {
                    if !self.static_routes.contains(cidr) {
                        self.static_routes.push(cidr.clone());
                    }
                    if !self.ip_routes.contains(cidr) {
                        self.ip_routes.push(cidr.clone());
                    }
                    session.append_log(&format!(
                        "[分流] IP 规则 {} → 隧道 {}",
                        cidr, self.tun_dev
                    ));
                }
                Err(e) => session.append_log(&format!(
                    "[分流] IP 规则 {} 未注入：{}（其它规则不受影响）",
                    cidr, e
                )),
            }
        }

        if !ip_direct.is_empty() {
            let target = route::RouteTarget::new(
                snapshot.default_if_index,
                &snapshot.default_if_alias,
                &snapshot.default_gateway,
            );
            // `ensure_route` 返回 `RouteOutcome::{Created,Updated}`，两种都算成功
            for cidr in ip_direct {
                match route::ensure_route(cidr, &target, 1) {
                    Ok(_) => {
                        session.append_log(&format!(
                            "[分流] 直连例外 {} → {}（{}）",
                            cidr, snapshot.default_gateway, snapshot.default_if_alias
                        ));
                        if !self.direct_routes.iter().any(|(item, _)| item == cidr) {
                            self.direct_routes.push((cidr.clone(), target.clone()));
                        }
                    }
                    Err(e) => session.append_log(&format!(
                        "[分流] 直连例外 {} 未安装：{}（其它规则不受影响）",
                        cidr, e
                    )),
                }
            }
        }

        self.write_ledger();
        Ok(())
    }

    /// 装一条 IP 规则的隧道路由。
    ///
    /// macOS 上如果同前缀已经有一条系统路由（本机局域网 on-link、别的 VPN 客户端
    /// 装的路由），显式配置优先：记下原出口后 `route change` 到隧道，断开时还原。
    /// Windows 直接同前缀 + metric 1 新增，系统按 metric 择优，断开只删自己那条。
    fn install_ip_tunnel_route(
        &mut self,
        cidr: &str,
        session: &VpnSession,
    ) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if let Ok(Some(existing)) = route::lookup_route(cidr) {
                if existing.if_name != self.tun_dev {
                    route::change_route_to_interface(cidr, &self.tun_dev)?;
                    session.append_log(&format!(
                        "[分流] 覆盖系统已有路由：{}（原出口 {}，断开时还原）",
                        cidr, existing.if_name
                    ));
                    self.overrides.push(existing);
                    return Ok(());
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = session;
        add_tunnel_route(cidr, &self.tun_dev, self.tun_index())
    }

    /// 把「新建的直连例外 + 改写的系统路由」落盘，崩溃后下次连接前能清理/还原。
    fn write_ledger(&self) {
        let mut entries: Vec<RouteLedgerEntry> = Vec::new();
        for (cidr, target) in &self.direct_routes {
            entries.push(RouteLedgerEntry::Direct {
                cidr: cidr.clone(),
                gateway: target.next_hop.clone(),
                if_index: target.if_index,
                if_name: target.if_name.clone(),
            });
        }
        for orig in &self.overrides {
            entries.push(RouteLedgerEntry::Override {
                cidr: orig.cidr(),
                orig: orig.clone(),
            });
        }
        let _ = std::fs::write(&self.ledger, encode_ledger(&entries));
    }
```

- [ ] **Step 8: 守护、断开、状态、清理**

`guard()` 的 macOS 分支末尾（`static_routes` 循环之后）加：

```rust
            for (cidr, target) in &self.direct_routes {
                if !route::route_exists(cidr, target).unwrap_or(true) {
                    let _ = route::ensure_route(cidr, target, 1);
                }
            }
            for orig in &self.overrides {
                match route::lookup_route(&orig.cidr()) {
                    Ok(Some(current)) if current.if_name == self.tun_dev => {}
                    _ => {
                        let _ = add_tunnel_route(&orig.cidr(), &self.tun_dev, self.tun_index());
                    }
                }
            }
```

`guard()` 的 Windows 分支 `ensure_onlink_routes` 之后加：

```rust
            for (cidr, target) in &self.direct_routes {
                let _ = route::ensure_route(cidr, target, 1);
            }
```

`teardown()` 在删完 `static_routes` 之后、清理 resolver 之前加：

```rust
        for (cidr, target) in &self.direct_routes {
            let _ = route::delete_route(cidr, target);
        }
        for entry in &self.overrides {
            let _ = route::restore_route(entry);
        }
        self.direct_routes.clear();
        self.overrides.clear();
        self.ip_routes.clear();
        let _ = std::fs::remove_file(&self.ledger);
```

`cleanup_stale()` 里改两处：

1. macOS 分支原本在「没有 `resolver-manifest.txt`」时 `return`，会跳过账本清理。
   把 `let Ok(text) = std::fs::read_to_string(&manifest) else { return; };`
   改为 `if let Ok(text) = std::fs::read_to_string(&manifest) {`，原来的循环体
   整体缩进放进这个块，块结束再加 `}`——**纯 IP 规则会话不会写这个 manifest，
   不修的话崩溃后的路由账本永远没人清理**。
2. 函数末尾（两个 `#[cfg]` 块之后）加：

```rust
    cleanup_ledger(session);
```

`routes_for_status()` 换成：

```rust
    pub fn routes_for_status(&self) -> Vec<super::models::AppliedRoute> {
        let mut out: Vec<super::models::AppliedRoute> = self
            .static_routes
            .iter()
            .map(|cidr| super::models::AppliedRoute {
                cidr: cidr.clone(),
                via: "vpn".to_string(),
                next_hop: String::new(),
                iface: self.tun_dev.clone(),
                owned: true,
            })
            .collect();
        out.extend(self.direct_routes.iter().map(|(cidr, target)| {
            super::models::AppliedRoute {
                cidr: cidr.clone(),
                via: "local".to_string(),
                next_hop: target.next_hop.clone(),
                iface: if target.if_name.is_empty() {
                    format!("if#{}", target.if_index)
                } else {
                    target.if_name.clone()
                },
                owned: true,
            }
        }));
        out
    }
```

再加计数方法（放在 `static_route_count` 之后）：

```rust
    /// 本次由 IP 规则装进隧道的网段数（状态/日志用，不含 VPN DNS 的 /32）。
    pub fn ip_route_count(&self) -> usize {
        self.ip_routes.len()
    }
```

- [ ] **Step 9: 更新唯一调用方（worker）**

`src-tauri/src/vpn/worker.rs` 的 `start_split_rules` 里，`SplitTunnel::apply(...)` 参数
在 `&server_ips` 之后插入 `&self.snapshot,`。**注意**是 worker 自己的
`self.snapshot`（`NetworkSnapshot`）；`self.status.snapshot` 是
`Option<NetworkSnapshot>`，别传错：

```rust
        match crate::vpn::split::SplitTunnel::apply(
            &self.session,
            &tun_dev,
            self.status.tun_index,
            &vpn_dns,
            &server_ips,
            &self.status.snapshot,
            self.memo.clone(),
            plan,
        ) {
```

- [ ] **Step 10: 编译并跑测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::split::tests`
Expected: `test result: ok`（账本测试通过；其它既有测试不受影响）

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: 编译通过，无 `unused` 警告涉及本次新增字段

- [ ] **Step 11: 提交**

```bash
git add src-tauri/src/vpn/split.rs src-tauri/src/vpn/worker.rs src-tauri/src/vpn/route.rs
git commit -m "feat(vpn): SplitTunnel 注入 IP 规则路由并落 ip-routes.txt 账本"
```

---

### Task 6: 校验与预览（`commands.rs`）

**Files:**
- Modify: `src-tauri/src/vpn/commands.rs:289-350`
- Test: `src-tauri/src/vpn/commands.rs` 的 `mod tests`（`:1366`、`:1533` 附近）

- [ ] **Step 1: 写失败测试**

把 `validates_the_three_live_kinds` 整段换成：

```rust
    #[test]
    fn validates_the_live_kinds() {
        assert!(validate_rule_input("DOMAIN", "oa.example.com").is_ok());
        assert!(validate_rule_input("DOMAIN-SUFFIX", "example.com").is_ok());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/8").is_ok());
        assert!(validate_rule_input("IP-CIDR", "192.168.6.115").is_ok());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/33").is_err());
        assert!(validate_rule_input("IP-CIDR", "0.0.0.0/0").is_err());
        assert!(validate_rule_input("IP-CIDR", "").is_err());
        assert!(validate_rule_input("10.0.0.0/8", "10.0.0.0/8").is_err());
        assert!(validate_rule_input("DOMAIN", "").is_err());
        assert!(validate_rule_input("bogus", "x").is_err());
        // 退役的四种类型必须明确报错，而不是静默通过
        for kind in ["DOMAIN-KEYWORD", "PROCESS-NAME", "DST-PORT", "RULE-SET"] {
            assert!(
                validate_rule_input(kind, "443").is_err(),
                "{} 应该被拒绝",
                kind
            );
        }
    }

    #[test]
    fn ip_rules_are_normalized_on_validate() {
        assert_eq!(
            vpn_validate_rule("ip-cidr".into(), " 192.168.6.115 ".into()).unwrap(),
            "192.168.6.115/32"
        );
        assert_eq!(
            vpn_validate_rule("ip-cidr".into(), "192.168.6.77/24".into()).unwrap(),
            "192.168.6.0/24"
        );
        assert!(vpn_validate_rule("ip-cidr".into(), "0.0.0.0/0".into()).is_err());
    }
```

把 `retired_ip_cidr_is_rejected_everywhere` 整个函数换成：

```rust
    /// `IP-CIDR` 重新上线：校验放行、预览按最长前缀给出走向。
    #[test]
    fn ip_cidr_rules_are_accepted_and_previewed() {
        let rules = vec![
            rule_of("DOMAIN-SUFFIX", "example.com", "VPN"),
            rule_of("IP-CIDR", "10.10.0.0/16", "VPN"),
            rule_of("IP-CIDR", "10.10.3.7/32", "DIRECT"),
        ];
        assert!(validate_rule_input("IP-CIDR", "10.10.0.0/16").is_ok());
        assert!(validate_rule_set(&rules, "DIRECT").is_ok());
        assert_eq!(
            preview_verdict(&rules, "DIRECT", Some("10.10.3.7")).unwrap(),
            "DIRECT"
        );
        assert_eq!(
            preview_verdict(&rules, "DIRECT", Some("10.10.3.8")).unwrap(),
            "VPN"
        );
        assert_eq!(
            preview_verdict(&rules, "DIRECT", Some("oa.example.com")).unwrap(),
            "VPN"
        );
        assert!(preview_verdict(&rules, "DIRECT", None).is_err());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::commands::tests`
Expected: 失败（`IP-CIDR` 被拒、`preview_verdict` 参数名不匹配等）

- [ ] **Step 3: 实现**

`validate_rule_input` 的 `IpCidr` 分支已在 Task 2 加好（调用
`split::normalize_ip_rule`），本任务**不要**重复添加；只改下面的 `vpn_validate_rule`
与预览。

`vpn_validate_rule` 整段换成：

```rust
/// 校验单条规则，返回可以落盘的规范化值。
///
/// 域名规则原样返回；`IP-CIDR` 返回归一后的 CIDR（单 IP 补成 `/32`，
/// 网段抹掉主机位），前端保存时直接用这个值。
#[tauri::command]
pub fn vpn_validate_rule(kind: String, value: String) -> Result<String, String> {
    validate_rule_input(&kind, &value)?;
    match crate::vpn::rule_engine::RuleKind::parse(&kind)? {
        crate::vpn::rule_engine::RuleKind::IpCidr => {
            crate::vpn::split::normalize_ip_rule(&value)
        }
        _ => Ok(value),
    }
}
```

`preview_verdict` 与 `vpn_preview_rules` 整段换成：

```rust
/// 预览一条输入会走哪：和真实数据面共用同一套判定。
///
/// 输入可以是域名，也可以是 IPv4 地址 / 网段（`IP-CIDR` 按最长前缀判定）；
/// 输入是网段时按网络地址判定。域名走 [`SplitPlan::tunnels`]，IP 走
/// [`SplitPlan::ip_tunnels`]。直连例外在预览里必须显示 DIRECT。
pub(crate) fn preview_verdict(
    rules: &[VpnRule],
    default_policy: &str,
    target: Option<&str>,
) -> Result<String, String> {
    let plan = crate::vpn::split::plan(rules, default_policy);
    let Some(target) = target.map(str::trim).filter(|item| !item.is_empty()) else {
        return Err("请提供要预览的域名或 IP".to_string());
    };
    if crate::vpn::split::normalize_ip_rule(target).is_ok() {
        let normalized = crate::vpn::split::normalize_ip_rule(target)?;
        let (network, _) = crate::vpn::route::parse_cidr(&normalized)?;
        let ip = std::net::Ipv4Addr::from(network);
        return Ok(if plan.ip_tunnels(ip) { "VPN" } else { "DIRECT" }.to_string());
    }
    Ok(if plan.tunnels(target) { "VPN" } else { "DIRECT" }.to_string())
}

#[tauri::command]
pub fn vpn_preview_rules(
    rules: Vec<VpnRule>,
    default_policy: String,
    target: Option<String>,
) -> Result<String, String> {
    preview_verdict(&rules, &default_policy, target.as_deref())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::commands::tests`
Expected: `test result: ok`

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/commands.rs
git commit -m "feat(vpn): IP-CIDR 规则校验、归一与预览判定"
```

---

### Task 7: worker 日志与状态文案

**Files:**
- Modify: `src-tauri/src/vpn/worker.rs:685-700`（成功日志）、`:770-778`（切模式回收日志）、`:828-838`（连接说明）

- [ ] **Step 1: 改成功日志（`:685` 附近）**

把：

```rust
                self.log(&format!(
                    "[分流] 路由注入分流已生效：{} 个域名走隧道 {}，{} 条固定隧道路由；\
                     默认路由与系统 DNS 均未改动（系统解析器不受影响）",
                    split.domain_count(),
```

改成：

```rust
                self.log(&format!(
                    "[分流] 路由注入分流已生效：{} 个域名 + {} 条 IP 路由走隧道 {}，\
                     {} 条固定隧道路由；默认路由与系统 DNS 均未改动（系统解析器不受影响）",
                    split.domain_count(),
                    split.ip_route_count(),
```

（后面的 `tun_dev, split.static_route_count()` 保持不变。）

- [ ] **Step 2: 改计划日志（`:660` 附近）**

把 `"[分流] 路由注入模式：域名 {:?}，VPN DNS {:?}，隧道 {}"` 的参数补齐：

```rust
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
```

- [ ] **Step 3: 改切模式回收日志（`:770` 附近）**

```rust
        if let Some(mut split) = self.split.take() {
            split.teardown();
            self.log(&format!(
                "[模式] 已撤掉规则分流的布线（{} 个域名、{} 条 IP 路由与动态注入的内网 IP 一并回收）",
                split.domain_count(),
                split.ip_route_count()
            ));
        }
```

- [ ] **Step 4: 改连接说明（`:828` 附近）**

```rust
        if let Some(split) = self.split.as_ref() {
            return format!(
                "隧道已建立，路由注入分流已生效（{} 个域名 / {} 条 IP 路由 → {}）",
                split.domain_count(),
                split.ip_route_count(),
                split.tun_dev()
            );
        }
```

- [ ] **Step 5: 编译并跑 worker 单测**

Run: `cargo test --manifest-path src-tauri/Cargo.toml vpn::worker`
Expected: `test result: ok`

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/vpn/worker.rs
git commit -m "feat(vpn): worker 日志与连接说明带上 IP 路由计数"
```

---

### Task 8: 前端类型、规则弹窗与预览接口

**Files:**
- Modify: `src/types/vpn.ts:18-35`、`:233` 附近
- Modify: `src/components/vpn/VpnRuleDialog.vue:18-40`、`:225-250`
- Modify: `src/composables/useVpn.ts:22-30`、`:583-600`

- [ ] **Step 1: `types/vpn.ts`**

注释与类型：

```ts
/** 规则匹配类型，取值与后端 rule_engine::RuleKind 一致 */
export type VpnRuleKind = 'DOMAIN' | 'DOMAIN-SUFFIX' | 'IP-CIDR'

/**
 * 历史上还有这几种规则类型，但需要「按连接抓包 + 按进程/端口匹配」的数据面，
 * 路由注入模式没有落点，后端会直接拒绝。留着这份名单只为把老配置标成「不生效」。
 */
export const RETIRED_RULE_KINDS = [
  'DOMAIN-KEYWORD',
  'PROCESS-NAME',
  'DST-PORT',
  'RULE-SET',
] as const
```

在 `POLICY_LABELS` 附近加：

```ts
/** 规则类型在列表里的中文标签（老配置里的退役类型回退显示原值） */
export const RULE_KIND_LABELS: Record<VpnRuleKind, string> = {
  'DOMAIN-SUFFIX': '域名后缀',
  'DOMAIN': '完整域名',
  'IP-CIDR': 'IP / 网段',
}
```

- [ ] **Step 2: `VpnRuleDialog.vue`**

`KIND_OPTIONS` 加一项（放在 `DOMAIN` 之后）：

```ts
  { value: 'IP-CIDR', label: 'IP / 网段 (IP-CIDR)' },
```

import 补 `RULE_KIND_LABELS`：

```ts
import { POLICY_LABELS, RETIRED_RULE_KINDS, RULE_KIND_LABELS } from '@/types/vpn'
```

在 `kind` / `value` 附近加输入提示与「IP 写成域名」检测：

```ts
const IPV4_OR_CIDR = /^\d{1,3}(?:\.\d{1,3}){3}(?:\/\d{1,2})?$/

/** 输入框占位随类型切换：IP 规则提示单 IP 与 CIDR 两种写法 */
const valuePlaceholder = computed(() =>
  kind.value === 'IP-CIDR'
    ? '192.168.6.115 或 192.168.6.0/16'
    : 'example.com（子域用 DOMAIN-SUFFIX）',
)

/** 域名规则里填了 IP：不阻止保存，只提示改用 IP 类型（历史配置兼容） */
const ipWrittenAsDomain = computed(
  () => kind.value !== 'IP-CIDR' && IPV4_OR_CIDR.test(value.value.trim()),
)
```

模板里 `placeholder="请输入"` 改成 `:placeholder="valuePlaceholder"`，并在错误提示
`<p v-if="error" ...>` 之前插入：

```html
          <p v-if="ipWrittenAsDomain" class="field-hint">
            这看起来是 IP：域名规则填 IP 不会生效，请把类型改成「IP / 网段」
          </p>
```

列表里的类型标签：

```html
                <span class="row-kind">{{ RULE_KIND_LABELS[rule.kind] ?? rule.kind }}</span>
```

样式区补一条（与 `.field-error` 同区域）：

```css
.field-hint {
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-muted);
}
```

- [ ] **Step 3: `useVpn.ts`**

`usableRules` 注释更新：

```ts
/**
 * 已下线的规则类型（DOMAIN-KEYWORD / PROCESS-NAME / DST-PORT / RULE-SET）
 * 不参与数据面，留着只会让后端整体校验失败。从配置里读出来时就剔掉；
 * 界面仍能看到它们并手动删除。
 */
```

`previewRule` 参数改名与注释：

```ts
  /**
   * 预览一条输入会走哪。
   *
   * 输入可以是域名，也可以是 IPv4 / CIDR 网段；路由注入模式下没有按连接分流，
   * 端口、进程都不参与判定。
   */
  async function previewRule(
    rules: VpnRule[],
    defaultPolicy: VpnPolicy,
    target: string,
  ): Promise<VpnPolicy> {
    const verdict = await invoke<string>('vpn_preview_rules', {
      rules: usableRules(rules),
      defaultPolicy,
      target: target || null,
    })
    return verdict as VpnPolicy
  }
```

- [ ] **Step 4: 类型检查与构建**

Run: `pnpm build`
Expected: `vue-tsc --noEmit` 通过，`vite build` 成功

- [ ] **Step 5: 提交**

```bash
git add src/types/vpn.ts src/components/vpn/VpnRuleDialog.vue src/composables/useVpn.ts
git commit -m "feat(vpn): 前端支持 IP-CIDR 规则类型与 IP 预览"
```

---

### Task 9: README 与全量回归

**Files:**
- Modify: `README.md:104-112`

- [ ] **Step 1: 更新规则分流说明**

把「规则类型只剩两类…」与「老配置里的 `IP-CIDR` 规则不会再生效…」两条，
替换成：

```markdown
- 规则类型有三类：`DOMAIN`、`DOMAIN-SUFFIX`、`IP-CIDR`；出口只有 `DIRECT`（直连）与 `VPN` 两种。`IP-CIDR` 的值可以是单个 IP（自动按 `/32` 处理）或 CIDR 网段，例如 `192.168.6.115`、`192.168.6.0/24`
- `IP-CIDR` 不需要 DNS：连接完成就立刻把命中网段装成隧道路由，适合「内网网站只给 IP、没有域名」的场景。判定按最长前缀（`/24` 比 `/16` 具体），前缀相同时先出现的规则胜出；`DIRECT` 例外用更具体的直连路由表达（例如 `192.168.6.0/24 → VPN` 再加 `192.168.6.9/32 → DIRECT`，只有 `.9` 直连）
- 显式配置优先：规则里写了的网段一律按规则注入，不会因为「看起来像本机局域网」而自动跳过。如果规则恰好覆盖本机所在网段，那个网段的流量会进隧道（通常就访问不到了），断开连接时路由自动还原——所以别把本机局域网写成 `VPN`
- 历史 `IP-CIDR` 规则从本版本起生效；`DOMAIN` / `DOMAIN-SUFFIX` 里写 IP 的旧写法仍然不生效（弹窗会提示改用 IP 类型）
```

- [ ] **Step 2: 全量后端测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: 全部 `test result: ok`；若出现与本改动无关的既有失败，先记录再排查

- [ ] **Step 3: 前端构建**

Run: `pnpm build`
Expected: 成功

- [ ] **Step 4: 提交**

```bash
git add README.md
git commit -m "docs(vpn): README 补充 IP-CIDR 规则说明"
```

---

### Task 10: 真机验收（需要连接 VPN + root，人工执行）

**Files:** 无代码改动。

- [ ] **Step 1: 升级构建**

Run: `pnpm tauri build`（或 `pnpm package:local`），用新构建替换旧安装。

- [ ] **Step 2: 新增规则**

在 VPN 面板「规则」里新增：类型 `IP / 网段 (IP-CIDR)`，内容 `192.168.6.0/24`，策略 `VPN`。
预期：保存后规则列表显示 `IP / 网段`，连接中会立刻热更新（日志出现
`[规则] 已更新 … 条规则，正在重新下发路由注入分流`）。

- [ ] **Step 3: 验证路由与访问**

Run: `route -n get 192.168.6.115`
Expected: `interface: utun*`（而不是 `en0`）

Run: `curl -k -I --max-time 10 https://192.168.6.115:51006/oa/web`
Expected: 有 HTTP 响应（301/302/200 均可）

- [ ] **Step 4: 验证纯 IP 规则不碰 DNS**

Run: `ls /etc/resolver`
Expected: 没有新增 `192.168…` 文件；连接后系统 DNS 未被改写
（`scutil --dns | head -20` 的 resolver 列表里没有本机转发器）。

- [ ] **Step 5: 验证断开还原**

断开连接后 Run: `route -n get 192.168.6.115`
Expected: 回到 `interface: en0`（或默认网关）；`192.168.6.9` 这类直连例外
（若配置了）也回到原网关。

- [ ] **Step 6: 验证显式覆盖本机网段**

临时加 `192.168.1.0/24 → VPN`（把 `<你本机网段>` 换成 `netstat -rn | grep default`
对应的网段），连接后 Run: `route -n get 192.168.1.1`
Expected: `interface: utun*`，日志有 `[分流] 覆盖系统已有路由：…（原出口 en0，断开时还原）`；
断开后 Run 同命令，Expected: 回到 `en0`。

- [ ] **Step 7: 验证 DIRECT 例外**

配 `192.168.6.0/24 → VPN` + `192.168.6.9/32 → DIRECT`，连接后
Run: `route -n get 192.168.6.9`
Expected: 走默认网关（`gateway: 192.168.1.1` / `interface: en0`），而非 utun。

- [ ] **Step 8: 验证崩溃清理（可选）**

连接后 `kill -9` worker 进程，重启应用再连接一次。
Expected: `worker.log` 出现账本清理；`route -n get <覆盖网段>` 不残留指向已消失 utun 的路由。

---

## 自审记录

- **Spec 覆盖**：§4 规则语义 → Task 1/2/6；§5 编译与模型 → Task 2/3；§6 安全边界 →
  Task 3（`/0`、服务器 IP、VPN DNS、显式优先）+ Task 5（覆盖与还原）；§7 运行期 →
  Task 5, 7；§7.3 账本 → Task 5；§8 UI → Task 8；§9 测试 → 各任务测试 + Task 10；
  §10 兼容 → Task 9；§11 风险 → Task 5/10 的覆盖/还原/清理步骤。
- **占位符**：无 TBD/TODO；所有新函数都给了完整实现。
- **类型一致性**：`SplitPlan.ip_rules`、`IpRule.cidr()`、`ip_route_plan()` 的
  `IpRoutePlan { tunnel, direct, notes }`、`RouteLedgerEntry::{Direct, Override}`、
  `SplitTunnel::{ip_routes, direct_routes, overrides, ledger}`、`route::RouteEntry`
  在各任务间名字一致。
