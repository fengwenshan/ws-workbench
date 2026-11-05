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
    /// Clash 规范形式：DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD / PROCESS-NAME / DST-PORT / IP-CIDR / RULE-SET
    pub kind: String,
    pub value: String,
    #[serde(default = "default_policy_str")]
    pub policy: String,
    #[serde(default = "default_true")]
    pub no_resolve: bool,
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
    /// 绑定的规则集 id
    #[serde(default)]
    pub rule_set_id: String,
    /// fake-ip 地址池，用于按域名分流时反查真实目标
    #[serde(default = "default_fake_ip_range")]
    pub fake_ip_range: String,
    /// 不进入 fake-ip 池的地址段
    #[serde(default)]
    pub fake_ip_filter: Vec<String>,
    /// 是否把服务端下发的 DNS 写入 VPN 网卡
    #[serde(default = "default_true")]
    pub use_vpn_dns: bool,
    /// 连接期间是否开启学习模式。开着才采集远端连接，关掉不影响正常连接。
    #[serde(default)]
    pub learn: bool,
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

fn default_protocol() -> String {
    "anyconnect".to_string()
}

fn default_proxy_mode() -> String {
    "rule".to_string()
}

fn default_fake_ip_range() -> String {
    "198.18.0.0/16".to_string()
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

/// 学习模式采集到的一台远端主机
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedHost {
    /// 远端 IPv4
    pub ip: String,
    /// 观察到的远端端口
    #[serde(default)]
    pub ports: Vec<u16>,
    /// 累计命中次数
    #[serde(default)]
    pub hits: u32,
    /// 该连接是否确实走了隧道（连接的本地地址就是隧道地址）
    #[serde(default)]
    pub via_vpn: bool,
    /// 发起连接的进程名
    #[serde(default)]
    pub processes: Vec<String>,
    #[serde(default)]
    pub first_seen: String,
    #[serde(default)]
    pub last_seen: String,
}

/// 由观察结果反推出来的候选分流网段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSubnet {
    pub cidr: String,
    /// 覆盖到的远端主机数
    #[serde(default)]
    pub hosts: u32,
    /// 累计连接次数
    #[serde(default)]
    pub hits: u32,
    /// 是否为「放大后的网段」：跨度大于 /24，是同一个上层里出现多个 /24 时汇聚出来的
    #[serde(default)]
    pub broad: bool,
    /// 现有规则表里是否已有条目能把它整个包含进去（也就是「已经照顾到」）
    #[serde(default)]
    pub covered: bool,
    /// 现有规则表里是否已有完全相同的网段
    #[serde(default)]
    pub adopted: bool,
    /// 采样到的远端地址，只留几条供人工判断归属
    #[serde(default)]
    pub samples: Vec<String>,
}

/// 学习报告：worker 每轮采样后落盘到会话目录 learn.json
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearnReport {
    /// 本次会话是否开着学习模式
    pub enabled: bool,
    /// 已完成的采样轮次
    #[serde(default)]
    pub scans: u32,
    #[serde(default)]
    pub updated_at: String,
    /// 观察到的主机明细
    #[serde(default)]
    pub hosts: Vec<LearnedHost>,
    /// 反推出来的候选网段
    #[serde(default)]
    pub candidates: Vec<LearnedSubnet>,
}

/// 状态文件里的学习模式摘要。
///
/// 明细走 learn.json：观察到的远端主机可能有几百条，塞进 status.json 会让
/// 每轮状态推送都背上几十 KB 的负载，而状态是每 600ms 比对一次的。
/// 这里只放几个计数，界面据此决定要不要去拉明细。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearnSummary {
    pub enabled: bool,
    #[serde(default)]
    pub scans: u32,
    /// 观察到的远端主机数
    #[serde(default)]
    pub hosts: u32,
    /// 反推出的候选网段数
    #[serde(default)]
    pub candidates: u32,
    /// 其中尚未被规则表覆盖的数量 —— 值得用户看的正是这些
    #[serde(default)]
    pub uncovered: u32,
    /// 采样失败的说明；反复失败后学习模式会自动停用，这里给出原因
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub updated_at: String,
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
    /// 学习模式摘要；没开学习模式时为 None
    #[serde(default)]
    pub learn: Option<LearnSummary>,
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
            connector: String::new(),
            connector_name: String::new(),
            tun_dev: String::new(),
            tun_index: 0,
            vpn_ip: String::new(),
            vpn_gateway: String::new(),
            dns: Vec::new(),
            mtu: 0,
            routes: Vec::new(),
            snapshot: None,
            learn: None,
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
    /// fake-ip 地址池
    #[serde(default = "default_fake_ip_range")]
    pub fake_ip_range: String,
    /// 不进入 fake-ip 池的地址段
    #[serde(default)]
    pub fake_ip_filter: Vec<String>,
    pub use_vpn_dns: bool,
    /// 连接期间是否开启学习模式：周期性采集实际连通的远端，反推候选分流网段
    #[serde(default)]
    pub learn: bool,
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
            "rule_set_id": "",
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
        assert!(rule.no_resolve);
    }

    #[test]
    fn old_profile_with_mode_still_loads() {
        let raw = r#"{"id":"p1","name":"公司 VPN","server":"vpn.example.com:443","username":"alice","password":"cipher","password_encrypted":true,"protocol":"anyconnect","mode":"company_first","rule_set_id":"rs1","use_vpn_dns":false,"extra_args":[],"connector_path":"","created_at":"","updated_at":""}"#;
        let profile: VpnProfile = serde_json::from_str(raw).expect("老档案必须还能读");
        assert_eq!(profile.server, "vpn.example.com:443");
        assert_eq!(profile.fake_ip_range, "198.18.0.0/16");
        // 老配置没有 proxy_mode，必须默认落在规则模式，不能变成全局或直连
        assert_eq!(profile.proxy_mode, "rule");
    }

    #[test]
    fn rule_set_default_policy_defaults_to_direct() {
        let set: VpnRuleSet = serde_json::from_str(r#"{"id":"rs1","name":"默认"}"#).unwrap();
        assert_eq!(set.default_policy, "DIRECT");
    }
}
