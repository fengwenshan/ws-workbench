use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// 连接器类型。现在只有 openconnect 一条通道，两个取值等价，
/// 保留枚举是为了不改存储格式，将来扩通道时直接加取值即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    Auto,
    Openconnect,
}

impl<'de> Deserialize<'de> for ConnectorKind {
    /// 手写反序列化，而不是靠 derive：老配置档案里可能还写着 cisco
    /// （那条保底通道已经拆掉了），derive 遇到不认识的取值会直接报错。
    /// 而整份 AppConfig 是「一处读失败就整份作废」的，那样用户的服务器、
    /// 密码、规则表会被一起清空，所以这里把不认识的取值一律降级成 auto。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "openconnect" => ConnectorKind::Openconnect,
            _ => ConnectorKind::Auto,
        })
    }
}

impl Default for ConnectorKind {
    fn default() -> Self {
        ConnectorKind::Auto
    }
}

/// 连接阶段，前端据此展示状态灯
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VpnPhase {
    /// 未连接
    Disconnected,
    /// 正在准备（生成脚本、写请求文件）
    Preparing,
    /// 正在等待提权 worker 启动
    Elevating,
    /// 正在建立隧道
    Connecting,
    /// 隧道已建立，正在按分流模式下发路由
    ConfiguringRoutes,
    /// 已连接
    Connected,
    /// 正在断开
    Disconnecting,
    /// 失败
    Failed,
}

impl VpnPhase {
    /// 状态文件 / 前端 `VpnPhase` 里的原始取值（serde 序列化用的就是它）
    pub fn as_str(&self) -> &'static str {
        match self {
            VpnPhase::Disconnected => "disconnected",
            VpnPhase::Preparing => "preparing",
            VpnPhase::Elevating => "elevating",
            VpnPhase::Connecting => "connecting",
            VpnPhase::ConfiguringRoutes => "configuring_routes",
            VpnPhase::Connected => "connected",
            VpnPhase::Disconnecting => "disconnecting",
            VpnPhase::Failed => "failed",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            VpnPhase::Disconnected => "未连接",
            VpnPhase::Preparing => "准备中",
            VpnPhase::Elevating => "等待提权",
            VpnPhase::Connecting => "连接中",
            VpnPhase::ConfiguringRoutes => "配置路由",
            VpnPhase::Connected => "已连接",
            VpnPhase::Disconnecting => "断开中",
            VpnPhase::Failed => "失败",
        }
    }
}

/// 单条分流规则。`kind`/`policy` 取值与 rule_engine::RuleKind/Policy 的 as_str() 一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnRule {
    pub id: String,
    /// 只有三类会生效：DOMAIN / DOMAIN-SUFFIX / IP-CIDR（见 `rule_engine::RuleKind`）
    pub kind: String,
    pub value: String,
    #[serde(default = "default_policy_str")]
    pub policy: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_policy_str() -> String {
    "DIRECT".to_string()
}

/// 规则集：一组有序规则 + 兜底走向，可以绑定到不同的 VPN 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnRuleSet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub rules: Vec<VpnRule>,
    #[serde(default = "default_policy_str")]
    pub default_policy: String,
    #[serde(default)]
    pub created_at: String,
}

/// VPN 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnProfile {
    pub id: String,
    pub name: String,
    /// 服务器地址，可带端口，例如 vpn.example.com:1218
    pub server: String,
    pub username: String,
    /// 密码密文（Windows 为 DPAPI 密文，macOS 为钥匙串引用标记），见 vpn/credential.rs
    #[serde(default)]
    pub password: String,
    /// 密码是否已加密存储
    #[serde(default)]
    pub password_encrypted: bool,
    /// openconnect 协议名：anyconnect / nc / gp / pulse / f5 / fortinet / array
    #[serde(default = "default_protocol")]
    pub protocol: String,
    /// 指定连接器；现在只有 openconnect 一条通道，auto 与 openconnect 等价
    #[serde(default)]
    pub connector: ConnectorKind,
    /// 代理模式：rule / global / direct。老配置没有这个字段时按规则模式处理。
    #[serde(default = "default_proxy_mode")]
    pub proxy_mode: String,
    /// 这份配置自带的分流规则。
    ///
    /// 规则以前放在全局 `vpn_rule_sets` 里用 `rule_set_id` 引用，现在改成内嵌；
    /// 这里必须真的能读写，否则每次落盘都会把用户的规则整段丢掉。
    #[serde(default)]
    pub rules: Vec<VpnRule>,
    /// 所有规则都不命中时的兜底走向（现在界面固定直连，保留字段兼容老配置）
    #[serde(default = "default_policy_str")]
    pub rules_default_policy: String,
    /// 额外的 openconnect 参数，每行一条
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// 手动指定的连接器可执行文件路径（覆盖自动探测）
    #[serde(default)]
    pub connector_path: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// 通道身份：只由「建立隧道所需的连接参数」决定。
///
/// 名称、规则、代理模式都不参与比较——它们可以在同一条隧道上热切换。
/// 输出是稳定哈希，不含明文密码，可以直接写进状态文件与前端。
pub fn conn_identity(profile: &VpnProfile) -> String {
    let mut parts: Vec<String> = vec![
        profile.server.clone(),
        profile.username.clone(),
        profile.password.clone(),
        profile.protocol.clone(),
        format!("{:?}", profile.connector),
        profile.connector_path.clone(),
    ];
    let mut extra = profile.extra_args.clone();
    extra.sort();
    parts.push(extra.join("\u{1e}"));

    let joined = parts.join("\u{1f}");
    // FNV-1a 64：不引入依赖，跨进程/跨重启稳定
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in joined.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn default_protocol() -> String {
    "anyconnect".to_string()
}

/// 代理模式：决定隧道建好之后，这台机器的流量怎么走。
///
/// - `Rule`：按规则注入（`split.rs`），只把命中的域名/网段送进隧道；
/// - `Global`：整机流量走隧道（`fulltunnel.rs`），等价于裸跑 openconnect；
/// - `Direct`：只建隧道，不接管任何流量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    Rule,
    Global,
    Direct,
}

impl ProxyMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "rule" => Ok(ProxyMode::Rule),
            "global" => Ok(ProxyMode::Global),
            "direct" => Ok(ProxyMode::Direct),
            other => Err(format!("不认识的代理模式：{}", other)),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProxyMode::Rule => "rule",
            ProxyMode::Global => "global",
            ProxyMode::Direct => "direct",
        }
    }
}

fn default_proxy_mode() -> String {
    "rule".to_string()
}

/// 探测到的连接器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorInfo {
    /// 通道标识，当前恒为 openconnect
    pub kind: String,
    /// 展示名，例如 OpenConnect (内置)
    pub name: String,
    /// 可执行文件绝对路径
    pub path: String,
    #[serde(default)]
    pub version: String,
    /// 是否可用
    pub available: bool,
    /// 来源：bundled / system / custom / missing
    pub source: String,
    /// 补充说明（不可用原因、安装提示等）
    #[serde(default)]
    pub note: String,
}

/// 已生效的路由（用于界面预览）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedRoute {
    /// 目标网段
    pub cidr: String,
    /// 出口：vpn / local
    pub via: String,
    /// 下一跳
    #[serde(default)]
    pub next_hop: String,
    /// 接口名
    #[serde(default)]
    pub iface: String,
    /// 是否由本程序添加
    #[serde(default)]
    pub owned: bool,
}

/// 连接前记录的网络环境快照，断开时据此还原
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkSnapshot {
    /// 原默认路由的下一跳
    #[serde(default)]
    pub default_gateway: String,
    /// 原默认路由所在接口索引
    #[serde(default)]
    pub default_if_index: u32,
    /// 原默认路由所在接口名
    #[serde(default)]
    pub default_if_alias: String,
    /// 物理网卡的本地网段（CIDR 列表），公司网络为主模式下作为直连例外
    #[serde(default)]
    pub local_subnets: Vec<String>,
}

/// 会话状态：worker 写入 status.json，主进程轮询后推给前端
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnStatus {
    pub phase: VpnPhase,
    /// 一句话状态描述
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub profile_id: String,
    #[serde(default)]
    pub profile_name: String,
    /// 通道身份哈希：连接参数相同的配置共享这条隧道
    #[serde(default)]
    pub conn_identity: String,
    /// 实际使用的连接器 kind
    #[serde(default)]
    pub connector: String,
    #[serde(default)]
    pub connector_name: String,
    /// 隧道接口名
    #[serde(default)]
    pub tun_dev: String,
    /// 隧道接口索引（Windows）
    #[serde(default)]
    pub tun_index: u32,
    /// 本次会话**实际生效**的代理模式（rule/global/direct）。
    ///
    /// 界面上的选择是「下次连接用哪个模式」，两者可能不一致：分流布线是连接时
    /// 一次做好的，运行期切模式要重布线。把实际模式写出来，界面才不会显示一个
    /// 正在运行却并不存在的模式（真机上已经误导过一次）。
    #[serde(default)]
    pub applied_mode: String,
    /// 隧道内网地址
    #[serde(default)]
    pub vpn_ip: String,
    /// 隧道内网网关
    #[serde(default)]
    pub vpn_gateway: String,
    /// 服务端下发的 DNS
    #[serde(default)]
    pub dns: Vec<String>,
    #[serde(default)]
    pub mtu: u32,
    /// 当前生效的分流路由
    #[serde(default)]
    pub routes: Vec<AppliedRoute>,
    /// 连接前的网络快照
    #[serde(default)]
    pub snapshot: Option<NetworkSnapshot>,
    /// 失败原因
    #[serde(default)]
    pub error: String,
    /// worker 进程 pid
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for VpnStatus {
    fn default() -> Self {
        VpnStatus {
            phase: VpnPhase::Disconnected,
            message: String::new(),
            profile_id: String::new(),
            profile_name: String::new(),
            conn_identity: String::new(),
            connector: String::new(),
            connector_name: String::new(),
            tun_dev: String::new(),
            tun_index: 0,
            applied_mode: String::new(),
            vpn_ip: String::new(),
            vpn_gateway: String::new(),
            dns: Vec::new(),
            mtu: 0,
            routes: Vec::new(),
            snapshot: None,
            error: String::new(),
            pid: 0,
            started_at: String::new(),
            updated_at: String::new(),
        }
    }
}

/// 写入 request.json 的连接请求（worker 读取）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnRequest {
    /// 代理模式：rule / global / direct
    #[serde(default = "default_proxy_mode")]
    pub proxy_mode: String,
    /// 通道身份哈希：判断两个配置能否共享同一条隧道
    #[serde(default)]
    pub conn_identity: String,
    pub profile_id: String,
    pub profile_name: String,
    pub server: String,
    pub username: String,
    /// 明文密码，只存在于本机会话目录，连接完成后由 worker 删除
    pub password: String,
    pub protocol: String,
    pub connector: ConnectorKind,
    /// 连接器可执行文件路径
    pub connector_path: String,
    /// 本次要下发的有序规则（已过滤掉停用项）
    #[serde(default)]
    pub rules: Vec<VpnRule>,
    /// 规则全不命中时的兜底走向
    #[serde(default = "default_policy_str")]
    pub default_policy: String,
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// 生成的 vpnc-script 绝对路径
    pub script_path: String,
    /// 路由钩子可执行文件（就是本程序自身）
    pub hook_exe: String,
    #[serde(default)]
    pub created_at: String,
}

/// 会话目录的实际位置与来源
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDirInfo {
    pub path: String,
    /// 应用数据目录 / 本地数据目录 / 临时目录
    pub origin: String,
    /// 是否发生了回退。回退时首选的 Roaming 目录不可写，
    /// 用户按默认路径是找不到日志的，界面要明确提示。
    pub fallback: bool,
}

/// 安装引导条目：连接器全都不可用时前端展示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallGuide {
    pub platform: String,
    /// 推荐方案标题
    pub title: String,
    /// 说明段落
    pub description: String,
    /// 步骤列表
    pub steps: Vec<GuideStep>,
    /// 参考链接
    #[serde(default)]
    pub links: Vec<GuideLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuideStep {
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// 需要执行的命令（可一键复制）
    #[serde(default)]
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuideLink {
    pub label: String,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connector_kind_round_trips() {
        assert_eq!(
            serde_json::to_string(&ConnectorKind::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::from_str::<ConnectorKind>("\"openconnect\"").unwrap(),
            ConnectorKind::Openconnect
        );
    }

    #[test]
    fn retired_cisco_connector_falls_back_to_auto() {
        // 老档案里存着 "cisco"。这条通道已经拆掉，但绝不能因为一个过时字段
        // 就让整份配置反序列化失败 —— 那会连服务器地址和密码一起丢掉。
        assert_eq!(
            serde_json::from_str::<ConnectorKind>("\"cisco\"").unwrap(),
            ConnectorKind::Auto
        );
        // 将来扩通道时出现的新取值同样按 auto 处理，不会炸在读配置这一步。
        assert_eq!(
            serde_json::from_str::<ConnectorKind>("\"whatever\"").unwrap(),
            ConnectorKind::Auto
        );
    }

    #[test]
    fn config_with_retired_connector_still_loads() {
        let raw = r#"{
            "connector": "cisco",
            "id": "p1",
            "name": "公司 VPN",
            "server": "vpn.example.com:443",
            "username": "alice",
            "password": "cipher",
            "password_encrypted": true,
            "protocol": "anyconnect",
            "mode": "customer_first",
                        "use_vpn_dns": false,
            "extra_args": [],
            "connector_path": "",
            "created_at": "",
            "updated_at": ""
        }"#;
        let profile: VpnProfile = serde_json::from_str(raw).expect("老档案必须还能读出来");
        assert_eq!(profile.connector, ConnectorKind::Auto);
        assert_eq!(profile.server, "vpn.example.com:443");
    }

    #[test]
    fn default_rule_is_domain_match_direct() {
        let rule: VpnRule = serde_json::from_str(
            r#"{"id":"r1","kind":"DOMAIN","value":"a.com","policy":"DIRECT","remark":"","enabled":true}"#,
        ).unwrap();
        assert_eq!(rule.kind, "DOMAIN");
        assert_eq!(rule.policy, "DIRECT");
    }

    #[test]
    fn old_profile_with_mode_still_loads() {
        let raw = r#"{"id":"p1","name":"公司 VPN","server":"vpn.example.com:443","username":"alice","password":"cipher","password_encrypted":true,"protocol":"anyconnect","mode":"company_first","rule_set_id":"rs1","use_vpn_dns":false,"extra_args":[],"connector_path":"","created_at":"","updated_at":""}"#;
        let profile: VpnProfile = serde_json::from_str(raw).expect("老档案必须还能读");
        assert_eq!(profile.server, "vpn.example.com:443");
        // 老配置没有 proxy_mode，必须默认落在规则模式，不能变成全局或直连
        assert_eq!(profile.proxy_mode, "rule");
    }

    #[test]
    fn profile_rules_survive_save_and_load() {
        let raw = r#"{"id":"p1","name":"A","server":"vpn.example.com:443","username":"alice",
            "rules":[{"id":"r1","kind":"DOMAIN-SUFFIX","value":"corp.example.com","policy":"VPN","remark":"","enabled":true}],
            "rules_default_policy":"DIRECT"}"#;
        let profile: VpnProfile = serde_json::from_str(raw).expect("带规则的配置必须能读出来");
        assert_eq!(profile.rules.len(), 1);
        assert_eq!(profile.rules[0].value, "corp.example.com");
        assert_eq!(profile.rules[0].policy, "VPN");
        // 落盘再读回来不能丢
        let text = serde_json::to_string(&profile).unwrap();
        assert!(text.contains("corp.example.com"));
        let again: VpnProfile = serde_json::from_str(&text).unwrap();
        assert_eq!(again.rules.len(), 1);
        assert_eq!(again.rules_default_policy, "DIRECT");
    }

    #[test]
    fn rule_set_default_policy_defaults_to_direct() {
        let set: VpnRuleSet = serde_json::from_str(r#"{"id":"rs1","name":"默认"}"#).unwrap();
        assert_eq!(set.default_policy, "DIRECT");
    }

    fn sample_profile() -> VpnProfile {
        serde_json::from_str(
            r#"{"id":"p1","name":"A","server":"vpn.example.com:443","username":"alice","password":"secret","protocol":"anyconnect","extra_args":[],"connector_path":"","created_at":"","updated_at":""}"#,
        )
        .unwrap()
    }

    #[test]
    fn conn_identity_ignores_name_rules_and_mode() {
        let a = sample_profile();
        let mut b = a.clone();
        b.name = "另一个名字".to_string();
        b.proxy_mode = "global".to_string();
        assert_eq!(conn_identity(&a), conn_identity(&b));
        // 同一配置多次计算结果必须稳定
        assert_eq!(conn_identity(&a), conn_identity(&a));
    }

    #[test]
    fn conn_identity_changes_with_each_connection_field() {
        let base = sample_profile();
        let mut cases: Vec<VpnProfile> = Vec::new();
        let mut p = base.clone();
        p.server = "other.example.com:443".to_string();
        cases.push(p);
        let mut p = base.clone();
        p.username = "bob".to_string();
        cases.push(p);
        let mut p = base.clone();
        p.password = "another".to_string();
        cases.push(p);
        let mut p = base.clone();
        p.protocol = "gp".to_string();
        cases.push(p);
        let mut p = base.clone();
        p.connector = ConnectorKind::Openconnect;
        cases.push(p);
        let mut p = base.clone();
        p.connector_path = "/usr/local/bin/openconnect".to_string();
        cases.push(p);
        let mut p = base.clone();
        p.extra_args = vec!["--no-dtls".to_string()];
        cases.push(p);
        for case in &cases {
            assert_ne!(
                conn_identity(&base),
                conn_identity(case),
                "这些连接字段必须参与通道身份计算"
            );
        }
    }
}
