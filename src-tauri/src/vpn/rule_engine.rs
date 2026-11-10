//! 规则类型的解析与规范化（纯逻辑，好测）。
//!
//! 这里只保留**真正能生效**的两类：域名精确、域名后缀。
//! 早期的 Clash 式规则（`DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `RULE-SET`）
//! 需要「按连接抓包 + 按进程/端口匹配」的数据面，macOS 侧已经改成路由注入
//! （见 `split.rs` / `fulltunnel.rs`），那套匹配器没有落点了，一并删掉。
//!
//! `IP-CIDR` 也已经下线：网段注入只做「整段进隧道」，做不到按网段排除，留着
//! 只会让用户配了以为生效。老配置里如果还留着这些类型，`RuleKind::parse` 会
//! 明确报错，界面按「不生效」标注。

/// 规则的匹配类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    /// 精确域名：`oa.example.com`
    Domain,
    /// 域名后缀：`example.com`（DNS 上含子域；路由注入只解析字面域名本身）
    DomainSuffix,
}

impl RuleKind {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let text = raw.trim().to_ascii_uppercase();
        match text.as_str() {
            "DOMAIN" => Ok(RuleKind::Domain),
            "DOMAIN-SUFFIX" => Ok(RuleKind::DomainSuffix),
            "IP-CIDR" => Err(
                "网段规则（IP-CIDR）已下线：路由注入模式只支持 DOMAIN / DOMAIN-SUFFIX，\
                 请改成域名规则"
                    .to_string(),
            ),
            "" => Err("规则类型不能为空".to_string()),
            "DOMAIN-KEYWORD" => Err(
                "DOMAIN-KEYWORD 需要按域名做数据面匹配，路由注入模式不支持；请改成 DOMAIN-SUFFIX"
                    .to_string(),
            ),
            "PROCESS-NAME" => Err("PROCESS-NAME 需要接管进程流量，路由注入模式不支持".to_string()),
            "DST-PORT" => Err("DST-PORT 需要按连接分流，路由注入模式不支持".to_string()),
            "RULE-SET" => Err("RULE-SET 规则集已不再支持，请把规则展开成具体条目".to_string()),
            other => Err(format!("不认识的规则类型：{}", other)),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RuleKind::Domain => "DOMAIN",
            RuleKind::DomainSuffix => "DOMAIN-SUFFIX",
        }
    }
}

/// 命中的规则把流量送去哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// 走 VPN
    Vpn,
    /// 直连
    Direct,
}

impl Policy {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let text = raw.trim().to_ascii_uppercase();
        match text.as_str() {
            "" | "VPN" | "PROXY" => Ok(Policy::Vpn),
            "DIRECT" => Ok(Policy::Direct),
            other => Err(format!("不认识的走向：{}（只支持 VPN / DIRECT）", other)),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Vpn => "VPN",
            Policy::Direct => "DIRECT",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_two_live_kinds() {
        assert_eq!(RuleKind::parse("DOMAIN").unwrap(), RuleKind::Domain);
        assert_eq!(
            RuleKind::parse("domain-suffix").unwrap(),
            RuleKind::DomainSuffix
        );
    }

    #[test]
    fn retired_kinds_fail_with_actionable_message() {
        for (kind, hint) in [
            ("DOMAIN-KEYWORD", "DOMAIN-SUFFIX"),
            ("PROCESS-NAME", "进程"),
            ("DST-PORT", "连接"),
            ("RULE-SET", "规则集"),
            ("IP-CIDR", "域名"),
        ] {
            let err = RuleKind::parse(kind).unwrap_err();
            assert!(err.contains(hint), "{} 的报错要说清楚怎么办：{}", kind, err);
        }
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!(RuleKind::parse("BOGUS").is_err());
        assert!(RuleKind::parse("").is_err());
    }

    #[test]
    fn policies_round_trip() {
        assert_eq!(Policy::parse("VPN").unwrap(), Policy::Vpn);
        assert_eq!(Policy::parse("direct").unwrap(), Policy::Direct);
        assert_eq!(Policy::parse("").unwrap(), Policy::Vpn, "空值默认走 VPN");
        assert!(Policy::parse("REJECT").is_err());
        assert_eq!(Policy::Vpn.as_str(), "VPN");
    }
}
