//! Clash 式规则引擎（纯逻辑）
//!
//! 语义与 Clash 一致：规则是**有序列表**，**自上而下首个命中生效**，末尾由
//! `default_policy`（对应 Clash 的 `MATCH`）兜底。**不是**最长前缀匹配。
//!
//! 本模块不碰系统调用与文件，输入是「已采集好的连接信息」，输出是出口策略，
//! 因此可完全离线单测。运行期采集与配置落地由其它模块负责。

use std::collections::HashMap;

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
    /// 展开后的具体规则列表（`RULE-SET` 已在 `compile` 阶段就地展开）。
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// 自上而下首个命中生效；全不命中则用 `default_policy`。
    pub fn evaluate(&self, input: &MatchInput) -> Policy {
        self.evaluate_hit(input).0
    }

    /// 与 `evaluate` 相同，但同时给出命中的那条规则。
    ///
    /// 日志要能说清「这条连接为什么走了这个出口」，只给最终出口不够；
    /// 返回的引用直接指向规则表里的规则，调用方只读。
    pub fn evaluate_hit<'a>(&'a self, input: &MatchInput) -> (Policy, Option<&'a Rule>) {
        for rule in &self.rules {
            if rule_matches(rule, input) {
                return (rule.policy, Some(rule));
            }
        }
        (self.default_policy, None)
    }
}

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
                let expanded = Rule::new(entry.kind, &entry.value, rule.policy)
                    .with_no_resolve(entry.no_resolve);
                validate_rule(&expanded)?;
                out.push(expanded);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_rule_matches_anywhere_in_host() {
        let set = compile(
            &[Rule::new(RuleKind::DomainKeyword, "risen", Policy::Vpn)],
            &HashMap::new(),
            Policy::Direct,
        )
        .unwrap();
        for host in ["oa.risencn.com", "risencn.com", "vpn.risencn.com", "my-risen-tool.io"] {
            assert_eq!(
                set.evaluate(&MatchInput { domain: Some(host), ..Default::default() }),
                Policy::Vpn,
                "{host} 应该命中关键词 risen"
            );
            assert_eq!(set.evaluate_hit(&MatchInput { domain: Some(host), ..Default::default() }).1.map(|r| r.kind), Some(RuleKind::DomainKeyword));
        }
        assert_eq!(
            set.evaluate(&MatchInput { domain: Some("example.com"), ..Default::default() }),
            Policy::Direct
        );
        // 拿不到域名时（例如应用自己缓存了 IP、没走本机 DNS），域名类规则一律不命中
        assert_eq!(set.evaluate(&MatchInput::default()), Policy::Direct);
    }

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
    fn invalid_rule_set_entry_is_rejected() {
        let mut sets = HashMap::new();
        sets.insert(
            "corp".to_string(),
            vec![Rule::new(RuleKind::IpCidr, "10.0.0.0", Policy::Direct)],
        );
        let rules = vec![Rule::new(RuleKind::RuleSet, "corp", Policy::Vpn)];
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
}
