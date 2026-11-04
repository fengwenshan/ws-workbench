# Clash 式规则分流（TUN 引擎）一期：规则引擎与 fake-ip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 Clash 式分流引擎的**纯逻辑内核**——有序列规则的解析与「首个命中生效」匹配，以及 fake-ip 假地址池的分配/反查/过滤/回收，全部以 `cargo test` 覆盖。

**Architecture:** 本期只做零平台依赖的纯逻辑，新增两个独立模块：`vpn/rule_engine.rs`（规则模型 + 规则行解析 + 匹配 + `RULE-SET` 内存展开）与 `vpn/fakedns.rs`（假 IP 池 + `fake_ip_filter`）。两者都不碰系统调用、不碰文件、不依赖 `models.rs` 的配置类型，因而可独立编译与单测。运行期（TUN 捕获、把连接喂给引擎）与配置/UI 落地分别在二～四期承接。

**Tech Stack:** Rust 2021（仅标准库 + 复用既有 `vpn/route.rs` 的纯 CIDR 工具），`#[cfg(test)]` 内建单测。无新增第三方依赖。

**关联设计文档:** `docs/superpowers/specs/2026-09-30-clash-like-routing-design.md`（§4 规则模型与匹配语义、§5 fake-ip DNS 子系统、§9 错误处理、§10 测试策略、§11 分期第 1 期）

**约定：**
- 所有命令均在 `src-tauri/` 目录下执行；测试命令为 `cargo test`。
- 每个任务末尾的 `git commit` 步骤**仅在用户明确同意提交后执行**；未获同意时跳过提交、保留工作区改动。
- 每期（本计划全部任务）完成后按项目规则重启应用（见 Task 6）。

**本期范围（只做这些）：**
- `rule_engine`：`Policy` / `RuleKind` / `Rule` 类型；规则行解析（含注释、缺省出口、`no-resolve`）；六类匹配（`DOMAIN` / `DOMAIN-SUFFIX` / `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `IP-CIDR`）；有序首命中 + 默认出口；`RULE-SET` 的**内存就地展开**。
- `fakedns`：假 IP 池构造与校验、分配、双向反查、`fake_ip_filter`、TTL 回收、LRU 淘汰、清空。

**本期不做（留给后续期）：**
- 读取 `.yaml` / `.list` 规则集**文件**（磁盘 I/O 与解析）——第 3 期（本期只实现「给定内存条目就地展开」）。
- `tun_device` / `netstack` / `router` / `proc_lookup` / `session`——第 2、3 期。
- `models.rs` 配置类型改造、`VpnMode` 移除、前端类型与 UI——第 4 期。

---

## 文件结构

| 文件 | 动作 | 职责 |
| --- | --- | --- |
| `src-tauri/src/vpn/rule_engine.rs` | 新建 | Clash 式有序列规则的解析、匹配与 `RULE-SET` 内存展开（纯逻辑） |
| `src-tauri/src/vpn/fakedns.rs` | 新建 | fake-ip 假地址池：分配 / 反查 / 过滤 / 回收 / 淘汰（纯逻辑） |
| `src-tauri/src/vpn/mod.rs` | 修改 | 注册 `rule_engine` / `fakedns` 模块，更新模块职责说明 |

---

## Task 1: `rule_engine.rs` 类型与规则行解析

**Files:**
- Create: `src-tauri/src/vpn/rule_engine.rs`
- Modify: `src-tauri/src/vpn/mod.rs`（新增 `pub mod rule_engine;`，否则该文件不参与编译、测试无从运行）

- [ ] **Step 1: 新建 `rule_engine.rs`，写入类型、规则行解析与校验（含失败测试先行）**

新建 `src-tauri/src/vpn/rule_engine.rs`：

```rust
//! Clash 式规则引擎（纯逻辑）
//!
//! 语义与 Clash 一致：规则是**有序列表**，**自上而下首个命中生效**，末尾由
//! `default_policy`（对应 Clash 的 `MATCH`）兜底。**不是**最长前缀匹配。
//!
//! 本模块不碰系统调用与文件，输入是「已采集好的连接信息」，输出是出口策略，
//! 因此可完全离线单测。运行期采集与配置落地由其它模块负责。

use super::route;

/// 出口策略。对应 Clash 的 DIRECT / PROXY，本设计只有直连与 VPN 两种。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Direct,
    Vpn,
}

impl Policy {
    /// 用于日志与界面展示的稳定文本形式。
    pub fn as_str(&self) -> &'static str {
        match self {
            Policy::Direct => "DIRECT",
            Policy::Vpn => "VPN",
        }
    }

    /// 从文本解析出口（大小写不敏感）。
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_uppercase().as_str() {
            "DIRECT" => Ok(Policy::Direct),
            "VPN" => Ok(Policy::Vpn),
            other => Err(format!("未知出口策略：{}（仅支持 DIRECT / VPN）", other)),
        }
    }
}

/// 规则类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Domain,
    DomainSuffix,
    DomainKeyword,
    ProcessName,
    DstPort,
    IpCidr,
    RuleSet,
}

impl RuleKind {
    /// 用于日志与界面展示的稳定文本形式。
    pub fn as_str(&self) -> &'static str {
        match self {
            RuleKind::Domain => "DOMAIN",
            RuleKind::DomainSuffix => "DOMAIN-SUFFIX",
            RuleKind::DomainKeyword => "DOMAIN-KEYWORD",
            RuleKind::ProcessName => "PROCESS-NAME",
            RuleKind::DstPort => "DST-PORT",
            RuleKind::IpCidr => "IP-CIDR",
            RuleKind::RuleSet => "RULE-SET",
        }
    }

    /// 从文本解析规则类型（大小写不敏感）。
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_uppercase().as_str() {
            "DOMAIN" => Ok(RuleKind::Domain),
            "DOMAIN-SUFFIX" => Ok(RuleKind::DomainSuffix),
            "DOMAIN-KEYWORD" => Ok(RuleKind::DomainKeyword),
            "PROCESS-NAME" => Ok(RuleKind::ProcessName),
            "DST-PORT" => Ok(RuleKind::DstPort),
            "IP-CIDR" => Ok(RuleKind::IpCidr),
            "RULE-SET" => Ok(RuleKind::RuleSet),
            other => Err(format!("未知规则类型：{}", other)),
        }
    }

    /// `no-resolve` 仅对 `IP-CIDR` 有意义；其余类型该字段恒为 `true` 且不生效。
    pub fn honors_no_resolve(&self) -> bool {
        matches!(self, RuleKind::IpCidr)
    }
}

/// 一条规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub kind: RuleKind,
    /// 匹配值：域名 / 进程名 / 端口或端口区间 / CIDR / 规则集 ID。
    pub value: String,
    pub policy: Policy,
    /// 仅 `IP-CIDR` 有意义，默认 `true`（不触发 DNS 解析）；其余类型恒为 `true`。
    pub no_resolve: bool,
}

impl Rule {
    /// 构造规则；`no_resolve` 取默认值 `true`。
    pub fn new(kind: RuleKind, value: &str, policy: Policy) -> Self {
        Rule {
            kind,
            value: value.trim().to_string(),
            policy,
            no_resolve: true,
        }
    }

    /// 显式设置 `no_resolve`。非 `IP-CIDR` 类型会被强制回 `true`。
    pub fn with_no_resolve(mut self, no_resolve: bool) -> Self {
        self.no_resolve = if self.kind.honors_no_resolve() {
            no_resolve
        } else {
            true
        };
        self
    }
}

/// 解析单个端口或 `start-end` 区间，返回闭区间 `(start, end)`。
pub fn parse_port_range(text: &str) -> Result<(u16, u16), String> {
    let t = text.trim();
    if let Some((a, b)) = t.split_once('-') {
        let start: u16 = a
            .trim()
            .parse()
            .map_err(|_| format!("端口区间起点不合法：{}", text))?;
        let end: u16 = b
            .trim()
            .parse()
            .map_err(|_| format!("端口区间终点不合法：{}", text))?;
        if start == 0 || end == 0 || start > end {
            return Err(format!("端口区间不合法：{}", text));
        }
        Ok((start, end))
    } else {
        let port: u16 = t.parse().map_err(|_| format!("端口不合法：{}", text))?;
        if port == 0 {
            return Err(format!("端口不合法：{}", text));
        }
        Ok((port, port))
    }
}

/// 校验规则的匹配值是否合法（`IP-CIDR` 与 `DST-PORT` 需要额外校验）。
fn validate_rule(rule: &Rule) -> Result<(), String> {
    match rule.kind {
        RuleKind::IpCidr => {
            route::parse_cidr(&rule.value)?;
        }
        RuleKind::DstPort => {
            parse_port_range(&rule.value)?;
        }
        _ => {}
    }
    Ok(())
}

/// 解析一行规则，例如 `DOMAIN-SUFFIX,client.com,VPN` 或 `IP-CIDR,10.0.0.0/8,DIRECT,no-resolve`。
///
/// - 空行与以 `#` / `//` 开头的注释行返回 `Ok(None)`。
/// - 省略出口时用 `default_policy` 补全。
/// - 结尾可带 `no-resolve` / `resolve` 修饰（仅对 `IP-CIDR` 生效）。
/// - `MATCH` 不允许作为规则行出现（它是 `default_policy`，不是规则）。
pub fn parse_rule_line(line: &str, default_policy: Policy) -> Result<Option<Rule>, String> {
    let text = line.trim();
    if text.is_empty() || text.starts_with('#') || text.starts_with("//") {
        return Ok(None);
    }

    let mut parts = text.split(',').map(|s| s.trim());
    let kind_text = parts.next().unwrap_or("");
    if kind_text.eq_ignore_ascii_case("MATCH") {
        return Err("MATCH 不能作为规则行，请改用 default_policy".to_string());
    }
    let kind = RuleKind::parse(kind_text)?;

    let value = parts.next().unwrap_or("");
    if value.is_empty() {
        return Err(format!("规则缺少匹配值：{}", line));
    }

    let policy = match parts.next() {
        Some(p) if !p.is_empty() => Policy::parse(p)?,
        _ => default_policy,
    };

    let mut no_resolve = true;
    for extra in parts {
        if extra.eq_ignore_ascii_case("no-resolve") {
            no_resolve = true;
        } else if extra.eq_ignore_ascii_case("resolve") {
            no_resolve = false;
        }
    }

    let rule = Rule::new(kind, value, policy).with_no_resolve(no_resolve);
    validate_rule(&rule)?;
    Ok(Some(rule))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rule_line_with_explicit_policy() {
        let rule = parse_rule_line("DOMAIN-SUFFIX,client.com,VPN", Policy::Direct)
            .unwrap()
            .unwrap();
        assert_eq!(rule.kind, RuleKind::DomainSuffix);
        assert_eq!(rule.value, "client.com");
        assert_eq!(rule.policy, Policy::Vpn);
        assert!(rule.no_resolve);
    }

    #[test]
    fn falls_back_to_default_policy_when_omitted() {
        let rule = parse_rule_line("DOMAIN,a.b.com", Policy::Vpn).unwrap().unwrap();
        assert_eq!(rule.policy, Policy::Vpn);
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        assert!(parse_rule_line("", Policy::Direct).unwrap().is_none());
        assert!(parse_rule_line("   ", Policy::Direct).unwrap().is_none());
        assert!(parse_rule_line("# comment", Policy::Direct).unwrap().is_none());
        assert!(parse_rule_line("// comment", Policy::Direct).unwrap().is_none());
    }

    #[test]
    fn rejects_match_line() {
        assert!(parse_rule_line("MATCH,DIRECT", Policy::Direct).is_err());
    }

    #[test]
    fn rejects_unknown_kind_and_policy() {
        assert!(parse_rule_line("DOMAIN-FOO,x,DIRECT", Policy::Direct).is_err());
        assert!(parse_rule_line("DOMAIN,x,PROXY", Policy::Direct).is_err());
        assert!(parse_rule_line("DOMAIN,,DIRECT", Policy::Direct).is_err());
    }

    #[test]
    fn validates_ip_cidr_and_port() {
        assert!(parse_rule_line("IP-CIDR,10.0.0.0/8,VPN", Policy::Direct).is_ok());
        assert!(parse_rule_line("IP-CIDR,10.0.0.0/33,VPN", Policy::Direct).is_err());
        assert!(parse_rule_line("DST-PORT,443,VPN", Policy::Direct).is_ok());
        assert!(parse_rule_line("DST-PORT,1000-2000,VPN", Policy::Direct).is_ok());
        assert!(parse_rule_line("DST-PORT,2000-1000,VPN", Policy::Direct).is_err());
        assert!(parse_rule_line("DST-PORT,abc,VPN", Policy::Direct).is_err());
    }

    #[test]
    fn parses_no_resolve_only_for_ip_cidr() {
        let r = parse_rule_line("IP-CIDR,10.0.0.0/8,DIRECT,no-resolve", Policy::Vpn)
            .unwrap()
            .unwrap();
        assert!(r.no_resolve);

        let r = parse_rule_line("IP-CIDR,10.0.0.0/8,DIRECT,resolve", Policy::Vpn)
            .unwrap()
            .unwrap();
        assert!(!r.no_resolve);

        // 非 IP-CIDR：即便写了 resolve 也强制回 true
        let r = parse_rule_line("DOMAIN,a.com,DIRECT,resolve", Policy::Vpn)
            .unwrap()
            .unwrap();
        assert!(r.no_resolve);
    }

    #[test]
    fn parses_port_range_helper() {
        assert_eq!(parse_port_range("443").unwrap(), (443, 443));
        assert_eq!(parse_port_range("1000-2000").unwrap(), (1000, 2000));
        assert!(parse_port_range("0").is_err());
        assert!(parse_port_range("2000-1000").is_err());
        assert!(parse_port_range("abc").is_err());
    }
}
```

- [ ] **Step 2: 在 `mod.rs` 声明 `rule_engine` 模块（否则文件不参与编译）**

在 `src-tauri/src/vpn/mod.rs` 的模块声明列表中，把

```rust
pub mod route;
pub mod script;
```

改为

```rust
pub mod route;
pub mod rule_engine;
pub mod script;
```

- [ ] **Step 3: 运行测试，确认通过**

Run（`src-tauri/`）: `cargo test rule_engine`
Expected: 编译通过，`rule_engine` 下 8 个测试全绿。

- [ ] **Step 4: Commit（需用户同意）**

```bash
git add src-tauri/src/vpn/rule_engine.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): add clash-style rule model and rule-line parser"
```

---

## Task 2: `rule_engine.rs` 匹配执行

**Files:**
- Modify: `src-tauri/src/vpn/rule_engine.rs`（在 `#[cfg(test)]` 模块前插入实现；在测试模块内追加测试）

- [ ] **Step 1: 插入匹配输入、单条匹配与规则集求值**

在 `src-tauri/src/vpn/rule_engine.rs` 中，`#[cfg(test)]` 之前插入：

```rust
/// 匹配输入：运行期从一条连接上采集到的信息，缺省即「不参与该维度匹配」。
#[derive(Debug, Clone, Default)]
pub struct MatchInput<'a> {
    /// 目标域名（由 fake-ip 反查得到；拿不到则为 `None`）。
    pub domain: Option<&'a str>,
    /// 目标真实 IP 的点分十进制形式。
    pub ip: Option<&'a str>,
    /// 目标端口。
    pub port: Option<u16>,
    /// 源进程名。
    pub process: Option<&'a str>,
}

/// `client.com` 命中 `client.com` 与 `a.client.com`，但**不**命中 `notclient.com`。
fn domain_has_suffix(domain: &str, suffix: &str) -> bool {
    let domain = domain.to_ascii_lowercase();
    let suffix = suffix.to_ascii_lowercase();
    domain == suffix || domain.ends_with(&format!(".{}", suffix))
}

/// 单条规则是否命中。
fn rule_matches(rule: &Rule, input: &MatchInput) -> bool {
    match rule.kind {
        RuleKind::Domain => input
            .domain
            .map(|d| d.eq_ignore_ascii_case(&rule.value))
            .unwrap_or(false),
        RuleKind::DomainSuffix => input
            .domain
            .map(|d| domain_has_suffix(d, &rule.value))
            .unwrap_or(false),
        RuleKind::DomainKeyword => {
            let needle = rule.value.to_ascii_lowercase();
            input
                .domain
                .map(|d| d.to_ascii_lowercase().contains(&needle))
                .unwrap_or(false)
        }
        RuleKind::ProcessName => input
            .process
            .map(|p| p.eq_ignore_ascii_case(&rule.value))
            .unwrap_or(false),
        RuleKind::DstPort => match (input.port, parse_port_range(&rule.value)) {
            (Some(port), Ok((start, end))) => port >= start && port <= end,
            _ => false,
        },
        RuleKind::IpCidr => match (input.ip, route::parse_cidr(&rule.value)) {
            (Some(ip), Ok((network, prefix))) => match route::parse_ipv4(ip) {
                Some(value) => (value & route::prefix_to_mask(prefix)) == network,
                None => false,
            },
            _ => false,
        },
        // `RULE-SET` 应在 `compile` 阶段被就地展开，运行期不应残留。
        RuleKind::RuleSet => false,
    }
}

/// 编译后的规则集：有序具体规则 + 兜底出口。
#[derive(Debug, Clone)]
pub struct CompiledRuleSet {
    pub rules: Vec<Rule>,
    pub default_policy: Policy,
}

impl CompiledRuleSet {
    /// 自上而下首个命中生效；全不命中则用 `default_policy`。
    pub fn evaluate(&self, input: &MatchInput) -> Policy {
        for rule in &self.rules {
            if rule_matches(rule, input) {
                return rule.policy;
            }
        }
        self.default_policy
    }
}
```

- [ ] **Step 2: 在测试模块内追加匹配测试**

在 `src-tauri/src/vpn/rule_engine.rs` 的 `mod tests` 内追加：

```rust
    fn ruleset(rules: Vec<Rule>, default_policy: Policy) -> CompiledRuleSet {
        CompiledRuleSet { rules, default_policy }
    }

    #[test]
    fn domain_exact_matches_only_equal() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::Domain, "a.com", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("a.com"), ..Default::default() }),
            Policy::Vpn
        );
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("b.a.com"), ..Default::default() }),
            Policy::Direct
        );
    }

    #[test]
    fn domain_suffix_respects_boundary() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::DomainSuffix, "client.com", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("a.client.com"), ..Default::default() }),
            Policy::Vpn
        );
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("client.com"), ..Default::default() }),
            Policy::Vpn
        );
        // 边界：notclient.com 不能命中 client.com
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("notclient.com"), ..Default::default() }),
            Policy::Direct
        );
    }

    #[test]
    fn domain_keyword_contains() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::DomainKeyword, "gitlab", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("git.gitlab.io"), ..Default::default() }),
            Policy::Vpn
        );
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("example.com"), ..Default::default() }),
            Policy::Direct
        );
    }

    #[test]
    fn process_name_is_case_insensitive() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::ProcessName, "Idea", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(
            rs.evaluate(&MatchInput { process: Some("idea"), ..Default::default() }),
            Policy::Vpn
        );
        assert_eq!(
            rs.evaluate(&MatchInput { process: Some("Cursor"), ..Default::default() }),
            Policy::Direct
        );
    }

    #[test]
    fn dst_port_supports_range() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::DstPort, "1000-2000", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(
            rs.evaluate(&MatchInput { port: Some(1500), ..Default::default() }),
            Policy::Vpn
        );
        assert_eq!(
            rs.evaluate(&MatchInput { port: Some(80), ..Default::default() }),
            Policy::Direct
        );
    }

    #[test]
    fn ip_cidr_matches_network() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::IpCidr, "10.0.0.0/8", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(
            rs.evaluate(&MatchInput { ip: Some("10.1.2.3"), ..Default::default() }),
            Policy::Vpn
        );
        assert_eq!(
            rs.evaluate(&MatchInput { ip: Some("192.168.1.1"), ..Default::default() }),
            Policy::Direct
        );
    }

    #[test]
    fn first_match_wins_in_order() {
        let rs = ruleset(
            vec![
                Rule::new(RuleKind::DomainSuffix, "client.com", Policy::Vpn),
                Rule::new(RuleKind::DomainKeyword, "client", Policy::Direct),
            ],
            Policy::Direct,
        );
        // 两条都要求命中，顺序在前者胜出
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("a.client.com"), ..Default::default() }),
            Policy::Vpn
        );
    }

    #[test]
    fn falls_back_to_default_policy() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::Domain, "a.com", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(
            rs.evaluate(&MatchInput { domain: Some("zzz.com"), ..Default::default() }),
            Policy::Direct
        );
    }

    #[test]
    fn missing_input_never_matches_domain_rules() {
        let rs = ruleset(
            vec![Rule::new(RuleKind::DomainSuffix, "client.com", Policy::Vpn)],
            Policy::Direct,
        );
        assert_eq!(rs.evaluate(&MatchInput::default()), Policy::Direct);
    }
```

- [ ] **Step 3: 运行测试，确认通过**

Run（`src-tauri/`）: `cargo test rule_engine`
Expected: 编译通过，`rule_engine` 下 17 个测试全绿。

- [ ] **Step 4: Commit（需用户同意）**

```bash
git add src-tauri/src/vpn/rule_engine.rs
git commit -m "feat(vpn): add ordered first-match rule evaluation"
```

---

## Task 3: `rule_engine.rs` 的 `RULE-SET` 内存展开

**Files:**
- Modify: `src-tauri/src/vpn/rule_engine.rs`（顶部追加 `use`；`#[cfg(test)]` 前插入 `compile`；测试模块内追加测试）

- [ ] **Step 1: 追加 `HashMap` 导入**

在 `src-tauri/src/vpn/rule_engine.rs` 顶部，把

```rust
use super::route;
```

改为

```rust
use std::collections::HashMap;

use super::route;
```

- [ ] **Step 2: 插入 `compile`（把 `RULE-SET` 就地展开为具体规则）**

在 `#[cfg(test)]` 之前插入：

```rust
/// 把含 `RULE-SET` 的原始规则就地展开为具体规则。
///
/// `sets` 是「规则集 ID -> 已解析条目」，由上层在启动时加载（第 3 期负责从
/// `.yaml` / `.list` 文件读取）。展开时 `RULE-SET` 的条目**沿用外层规则的出口**
/// （规则集文件内的条目只有类型与值，不携带出口）。
///
/// 引用到不存在的规则集即报错——对应设计文档 §9 的「解析失败即拒绝启动」。
pub fn compile(
    rules: &[Rule],
    sets: &HashMap<String, Vec<Rule>>,
    default_policy: Policy,
) -> Result<CompiledRuleSet, String> {
    let mut out = Vec::new();
    for rule in rules {
        if rule.kind == RuleKind::RuleSet {
            let entries = sets
                .get(&rule.value)
                .ok_or_else(|| format!("RULE-SET 引用了未定义的规则集：{}", rule.value))?;
            for entry in entries {
                if entry.kind == RuleKind::RuleSet {
                    return Err(format!(
                        "规则集 {} 内不允许再嵌套 RULE-SET",
                        rule.value
                    ));
                }
                out.push(Rule {
                    kind: entry.kind,
                    value: entry.value.clone(),
                    policy: rule.policy,
                    no_resolve: entry.no_resolve,
                });
            }
        } else {
            out.push(rule.clone());
        }
    }
    Ok(CompiledRuleSet {
        rules: out,
        default_policy,
    })
}
```

- [ ] **Step 3: 在测试模块内追加展开测试**

在 `mod tests` 内追加：

```rust
    #[test]
    fn expands_rule_set_in_place() {
        let mut sets = HashMap::new();
        sets.insert(
            "corp".to_string(),
            vec![
                Rule::new(RuleKind::DomainSuffix, "client.com", Policy::Direct),
                Rule::new(RuleKind::IpCidr, "10.0.0.0/8", Policy::Direct),
            ],
        );
        let rules = vec![
            Rule::new(RuleKind::Domain, "before.com", Policy::Vpn),
            Rule::new(RuleKind::RuleSet, "corp", Policy::Vpn),
            Rule::new(RuleKind::Domain, "after.com", Policy::Vpn),
        ];
        let compiled = compile(&rules, &sets, Policy::Direct).unwrap();
        assert_eq!(compiled.rules.len(), 4);
        assert_eq!(compiled.rules[0].value, "before.com");
        assert_eq!(compiled.rules[1].value, "client.com");
        assert_eq!(compiled.rules[2].value, "10.0.0.0/8");
        assert_eq!(compiled.rules[3].value, "after.com");
    }

    #[test]
    fn rule_set_entries_inherit_outer_policy() {
        let mut sets = HashMap::new();
        sets.insert(
            "s".to_string(),
            vec![Rule::new(RuleKind::Domain, "x.com", Policy::Direct)],
        );
        let rules = vec![Rule::new(RuleKind::RuleSet, "s", Policy::Vpn)];
        let compiled = compile(&rules, &sets, Policy::Direct).unwrap();
        assert_eq!(compiled.rules[0].policy, Policy::Vpn);
    }

    #[test]
    fn missing_rule_set_is_rejected() {
        let sets = HashMap::new();
        let rules = vec![Rule::new(RuleKind::RuleSet, "nope", Policy::Vpn)];
        assert!(compile(&rules, &sets, Policy::Direct).is_err());
    }

    #[test]
    fn nested_rule_set_is_rejected() {
        let mut sets = HashMap::new();
        sets.insert(
            "inner".to_string(),
            vec![Rule::new(RuleKind::RuleSet, "outer", Policy::Direct)],
        );
        let rules = vec![Rule::new(RuleKind::RuleSet, "inner", Policy::Vpn)];
        assert!(compile(&rules, &sets, Policy::Direct).is_err());
    }

    #[test]
    fn expanded_entries_participate_in_ordering() {
        let mut sets = HashMap::new();
        sets.insert(
            "corp".to_string(),
            vec![Rule::new(RuleKind::DomainSuffix, "client.com", Policy::Vpn)],
        );
        let rules = vec![
            Rule::new(RuleKind::DomainSuffix, "client.com", Policy::Direct),
            Rule::new(RuleKind::RuleSet, "corp", Policy::Vpn),
        ];
        let compiled = compile(&rules, &sets, Policy::Direct).unwrap();
        // 第一条具体规则先命中，展开的条目排在其后
        assert_eq!(
            compiled.evaluate(&MatchInput { domain: Some("a.client.com"), ..Default::default() }),
            Policy::Direct
        );
    }
```

- [ ] **Step 4: 运行测试，确认通过**

Run（`src-tauri/`）: `cargo test rule_engine`
Expected: 编译通过，`rule_engine` 下 22 个测试全绿。

- [ ] **Step 5: Commit（需用户同意）**

```bash
git add src-tauri/src/vpn/rule_engine.rs
git commit -m "feat(vpn): expand RULE-SET entries in place"
```

---

## Task 4: `fakedns.rs` 假地址池、分配与过滤

**Files:**
- Create: `src-tauri/src/vpn/fakedns.rs`
- Modify: `src-tauri/src/vpn/mod.rs`（新增 `pub mod fakedns;`，否则该文件不参与编译）

- [ ] **Step 1: 新建 `fakedns.rs`，写入池、分配与过滤（含失败测试先行）**

新建 `src-tauri/src/vpn/fakedns.rs`：

```rust
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
}
```

- [ ] **Step 2: 在 `mod.rs` 声明 `fakedns` 模块（否则文件不参与编译）**

在 `src-tauri/src/vpn/mod.rs` 的模块声明列表中，把

```rust
pub mod detect;
pub mod elevate;
pub mod guide;
```

改为

```rust
pub mod detect;
pub mod elevate;
pub mod fakedns;
pub mod guide;
```

- [ ] **Step 3: 运行测试，确认通过**

Run（`src-tauri/`）: `cargo test fakedns`
Expected: 编译通过，`fakedns` 下 5 个测试全绿。

- [ ] **Step 4: Commit（需用户同意）**

```bash
git add src-tauri/src/vpn/fakedns.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): add fake-ip pool with allocation and filtering"
```

---

## Task 5: `fakedns.rs` 回收、淘汰与清空

**Files:**
- Modify: `src-tauri/src/vpn/fakedns.rs`（`#[cfg(test)]` 前插入实现；测试模块内追加测试）

- [ ] **Step 1: 插入 TTL 回收与清空**

在 `src-tauri/src/vpn/fakedns.rs` 的 `pick_slot` 之后、`#[cfg(test)]` 之前插入：

```rust
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
```

- [ ] **Step 2: 在 `impl FakeIpPool` 末尾追加 `clear`**

将 `reclaim_expired` 之后（仍是 `impl` 块内）继续追加：

```rust
    /// 断开重连时清空整池。
    pub fn clear(&mut self) {
        self.by_ip.clear();
        self.by_domain.clear();
        self.cursor = 1;
    }
```

- [ ] **Step 3: 在测试模块内追加回收/淘汰/清空测试**

在 `mod tests` 内追加：

```rust
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
```

- [ ] **Step 4: 运行测试，确认通过**

Run（`src-tauri/`）: `cargo test fakedns`
Expected: 编译通过，`fakedns` 下 8 个测试全绿。

- [ ] **Step 5: Commit（需用户同意）**

```bash
git add src-tauri/src/vpn/fakedns.rs
git commit -m "feat(vpn): add fake-ip TTL reclamation and LRU eviction"
```

---

## Task 6: 注册模块、全量测试与重启验收

**Files:**
- Modify: `src-tauri/src/vpn/mod.rs:15-26`（模块职责说明）
- Modify: `src-tauri/src/vpn/mod.rs:32-43`（模块声明列表；通常已由 Task 1 / Task 4 完成，本任务仅核对）

- [ ] **Step 1: 核对 `mod.rs` 已声明两个新模块**

核对 `src-tauri/src/vpn/mod.rs` 的模块声明列表同时包含：

```rust
pub mod fakedns;
pub mod rule_engine;
```

若缺失则补上（应与 Task 1 / Task 4 的声明一致）。

- [ ] **Step 2: 更新 `mod.rs` 的模块职责说明**

在 `src-tauri/src/vpn/mod.rs` 的文档注释模块分工列表中，把

```rust
//! - `learn`       学习模式：采样实际连通的远端 IP，反推候选分流网段
```

改为

```rust
//! - `learn`       学习模式：采样实际连通的远端 IP，反推候选分流网段
//! - `rule_engine` Clash 式有序列规则：解析、匹配与 RULE-SET 内存展开（纯逻辑）
//! - `fakedns`     fake-ip 假地址池：分配 / 反查 / 过滤 / 回收（纯逻辑）
```

- [ ] **Step 3: 运行全量测试**

Run（`src-tauri/`）: `cargo test`
Expected: 编译通过，既有测试与新增 `rule_engine`（22）、`fakedns`（8）测试全部通过，无失败。

- [ ] **Step 4: 确认无编译告警残留**

Run（`src-tauri/`）: `cargo build`
Expected: 构建成功；新增两个模块不产生 `unused` / `dead_code` 告警（对外 `pub` 项视为库 API，不触发 `dead_code`）。

- [ ] **Step 5: 重启应用**

先结束残留的 vite / tauri 进程，再重新拉起（macOS）：

```bash
pkill -f "ws-workbench" ; pkill -f "vite" ; pkill -f "target/debug/ws-workbench"
pnpm exec tauri dev
```

> 注意：不要用 `pnpm tauri dev`（`package.json` 的 `tauri` 脚本会解析成 `tauri dev dev`）。

- [ ] **Step 6: 按本期验收标准逐项核对**

Check:
1. `cargo test rule_engine` 全绿：顺序与首命中、`MATCH`（`default_policy`）兜底、六类匹配、`no-resolve` 行为、`RULE-SET` 内存展开均有对应测试。
2. `cargo test fakedns` 全绿：分配、双向反查、`fake_ip_filter` 命中、TTL 回收、LRU 淘汰、清空均有对应测试。
3. `cargo test` 全量通过，应用可正常启动（本期为纯逻辑，界面无可见变化属预期）。

Expected: 以上 3 项全部满足。

- [ ] **Step 7: Commit（需用户同意）**

```bash
git add src-tauri/src/vpn/mod.rs
git commit -m "chore(vpn): register rule_engine and fakedns modules"
```

---

## 自检（Self-Review）

**1. Spec 覆盖对照**

| Spec 章节 | 对应任务 |
| --- | --- |
| §4 「顺序即优先级 / 首个命中生效，非最长前缀」 | Task 2（`CompiledRuleSet::evaluate`、`first_match_wins_in_order`） |
| §4 「`MATCH` 用 `default_policy` 表示、必须存在」 | Task 2（`default_policy` 字段 + `falls_back_to_default_policy`）；`parse_rule_line` 拒绝把 `MATCH` 当规则行（Task 1） |
| §4 各匹配类型语义（`DOMAIN` / `DOMAIN-SUFFIX` / `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `IP-CIDR`） | Task 2（`rule_matches` + 各类测试） |
| §4 `DOMAIN-SUFFIX` 不误命中 `notclient.com` | Task 2（`domain_has_suffix`、`domain_suffix_respects_boundary`） |
| §4 `DST-PORT` 支持单端口与区间 | Task 1（`parse_port_range`）、Task 2（`dst_port_supports_range`） |
| §4 `no-resolve` 默认 `true`、仅 `IP-CIDR` 生效、其余类型恒 `true` | Task 1（`with_no_resolve`、`parses_no_resolve_only_for_ip_cidr`） |
| §4 `RULE-SET` 就地展开 | Task 3（`compile` + 展开测试） |
| §5 假 IP 池（默认 `198.18.0.0/16`，可配 `fake_ip_range`） | Task 4（`FakeIpPool::new` 接受任意 CIDR） |
| §5 `假IP -> 域名` 反查 | Task 4（`lookup_domain` / `lookup_ip`） |
| §5 生命周期：TTL + LRU 回收、池满淘汰最旧、断开清空 | Task 4（`allocate` 池满淘汰）、Task 5（`reclaim_expired` / `clear`） |
| §5 `fake_ip_filter`（后缀/关键词） | Task 4（`FilterRule`、`is_filtered`、`filtered_domains_are_not_faked`） |
| §9 「规则解析失败即拒绝启动」（fail-closed） | Task 3（`missing_rule_set_is_rejected` / `nested_rule_set_is_rejected` 返回 `Err`） |
| §10 纯逻辑层测试（主力，`cargo test` 全覆盖） | Task 1~5 全部测试 |
| §11 第 1 期「`rule_engine` + `fakedns` + 单测，`cargo test` 绿」 | 本计划整体；Task 6 Step 3 |

> 说明：§4 提到的 `RULE-SET` **文件格式（`.yaml` / `.list`）加载**、§11 第 3 期的「文件读取」、以及 §8 的配置/存储/UI 与 `VpnMode` 移除，均**不在本期**范围，映射到后续期次计划，此处不列为本期任务。

**2. Placeholder 扫描**：无 `TBD` / `TODO` / 「稍后实现」/「类似 Task N」等占位；每个代码步骤均给出可直接粘贴的完整代码。

**3. 类型与命名一致性核对**
- `Policy::{Direct, Vpn}` / `Policy::parse(&str) -> Result<Policy, String>` / `Policy::as_str() -> &'static str`：Task 1 定义，测试中一致使用。
- `RuleKind::{Domain, DomainSuffix, DomainKeyword, ProcessName, DstPort, IpCidr, RuleSet}` / `RuleKind::parse` / `RuleKind::honors_no_resolve`：Task 1 定义，Task 2、Task 3 一致使用。
- `Rule { kind, value, policy, no_resolve }` / `Rule::new(kind, &str, policy)` / `Rule::with_no_resolve(bool)`：Task 1 定义，Task 2、Task 3 一致使用。
- `parse_port_range(&str) -> Result<(u16, u16), String>`：Task 1 定义，Task 2 `rule_matches` 一致调用。
- `MatchInput { domain, ip, port, process }`（`Default`）：Task 2 定义，Task 2、Task 3 测试一致使用。
- `CompiledRuleSet { rules, default_policy }` / `CompiledRuleSet::evaluate(&MatchInput) -> Policy`：Task 2 定义，Task 3 的 `compile` 复用。
- `compile(&[Rule], &HashMap<String, Vec<Rule>>, Policy) -> Result<CompiledRuleSet, String>`：Task 3 定义，与测试调用签名一致。
- `FakeIpPool::new(&str, &[String]) -> Result<FakeIpPool, String>`：Task 4 定义；`allocate(&str, u64, u64) -> Option<u32>`、`lookup_domain(u32) -> Option<&str>`、`lookup_ip(&str) -> Option<u32>`、`contains(u32) -> bool`、`capacity() -> u32`、`len() -> usize`、`is_empty() -> bool`、`is_filtered(&str) -> bool`：Task 4 定义，Task 5 一致使用。
- `reclaim_expired(&mut self, u64) -> usize` / `clear(&mut self)`：Task 5 定义，与测试一致。
- 复用 `vpn/route.rs` 的 `parse_cidr` / `parse_ipv4` / `prefix_to_mask`：签名（`parse_cidr(&str) -> Result<(u32,u8),String>`、`parse_ipv4(&str) -> Option<u32>`、`prefix_to_mask(u8) -> u32`）与调用一致。

**4. 已知取舍**
- 本期 `rule_engine` / `fakedns` 的类型**不带 `serde` 派生**，保持纯逻辑；配置持久化（密文落盘）与「配置类型 ↔ 引擎类型」的映射由第 4 期在 `models.rs` / `config.rs` 落地，届时再按需补派生或映射函数。
- 本期 `FakeIpPool` 未从磁盘读时钟：`allocate` / `reclaim_expired` 显式接收 `now`（秒）。运行期由调用方传入当前时间，便于确定性与可测。
- `FakeIpPool::pick_slot` 池满时用 O(n) 扫描取 LRU 项（`/16` 约 6.5 万项，仅在池满路径触发）；一期以正确性与可读性优先，若后续有性能诉求再换数据结构。
- `RULE-SET` 的**文件**读取与解析（`.yaml` 的 `payload:` 数组、`.list` 纯文本）不在本期；`compile` 接收的是「上层已解析好的内存条目」，第 3 期把文件读入内存后调用同一 `compile`。
- 前端无单测框架，本期为纯 Rust 逻辑，验证手段为 `cargo test` 与 `cargo build`。
