/** 连接器偏好。现在只有 openconnect 一条通道，两个取值等价 */
export type ConnectorKind = 'auto' | 'openconnect'

/** 连接阶段 */
export type VpnPhase =
  | 'disconnected'
  | 'preparing'
  | 'elevating'
  | 'connecting'
  | 'configuring_routes'
  | 'connected'
  | 'disconnecting'
  | 'failed'

/** 代理模式，语义与 Clash 的 mode 一致 */
export type VpnProxyMode = 'rule' | 'global' | 'direct'

/** 规则匹配类型，取值与后端 rule_engine::RuleKind 一致 */
export type VpnRuleKind =
  | 'DOMAIN'
  | 'DOMAIN-SUFFIX'
  | 'DOMAIN-KEYWORD'
  | 'PROCESS-NAME'
  | 'DST-PORT'
  | 'IP-CIDR'

/** 规则走向，取值与后端 rule_engine::Policy 一致 */
export type VpnPolicy = 'DIRECT' | 'VPN'

/** 单条分流规则 */
export interface VpnRule {
  id: string
  kind: VpnRuleKind
  value: string
  policy: VpnPolicy
  no_resolve: boolean
  remark: string
  enabled: boolean
}

/** @deprecated 仅用于老配置迁移（vpn_rule_sets → profile.rules），不再参与运行期 */
export interface VpnRuleSet {
  id: string
  name: string
  rules: VpnRule[]
  /** 所有规则都不命中时的兜底走向 */
  default_policy: VpnPolicy
  created_at: string
}

/** VPN 配置 */
export interface VpnProfile {
  id: string
  name: string
  /** 服务器地址，可带端口 */
  server: string
  username: string
  /** 密码密文，前端只做透传，不解读 */
  password: string
  password_encrypted: boolean
  /** openconnect 协议名 */
  protocol: string
  connector: ConnectorKind
  /** rule：按本配置的规则分流；global：全部走 VPN；direct：全部直连 */
  proxy_mode: VpnProxyMode
  /** 这份配置自带的分流规则；只有规则模式下会参与分流 */
  rules: VpnRule[]
  /** 所有规则都不命中时的兜底走向 */
  rules_default_policy: VpnPolicy
  /** fake-ip 地址池，用于按域名分流时反查真实目标 */
  fake_ip_range: string
  /** 不进入 fake-ip 池的地址段（*.后缀 或 关键字） */
  fake_ip_filter: string[]
  /** 连接期间是否开启学习模式：采集实际连通的远端地址，反推候选分流网段 */
  learn: boolean
  extra_args: string[]
  /** 手动指定的连接器路径 */
  connector_path: string
  created_at: string
  updated_at: string
}

/** 新建 VPN 配置弹窗提交的基础信息 */
export interface NewVpnProfileInput {
  name: string
  server: string
  username?: string
  password?: string
  protocol?: string
  extra_args?: string[]
  /** fake-ip 地址池；不填用后端默认值 */
  fake_ip_range?: string
  /** 不走 fake-ip 的域名（*.后缀 / .后缀 / 关键字） */
  fake_ip_filter?: string[]
}

/** 探测到的连接器 */
export interface ConnectorInfo {
  kind: 'openconnect' | string
  name: string
  path: string
  version: string
  available: boolean
  /** custom / managed / bundled / system / missing */
  source: string
  note: string
}

/** 已生效的路由 */
export interface AppliedRoute {
  cidr: string
  /** vpn / local */
  via: string
  next_hop: string
  iface: string
  owned: boolean
}

/** 连接前的网络快照 */
export interface NetworkSnapshot {
  default_gateway: string
  default_if_index: number
  default_if_alias: string
  local_subnets: string[]
}

/** 学习模式采集到的一台远端主机 */
export interface LearnedHost {
  ip: string
  ports: number[]
  /** 在多少轮采样里被看到过 */
  hits: number
  /** 至少有一条连接确实走了隧道 */
  via_vpn: boolean
  processes: string[]
  first_seen: string
  last_seen: string
}

/** 由观察结果反推出来的候选分流网段 */
export interface LearnedSubnet {
  cidr: string
  hosts: number
  hits: number
  /** 是向上汇聚出来的大网段（/16、/8），采纳前要多想一下 */
  broad: boolean
  /** 现有规则表里已有一条能把它整个包住 */
  covered: boolean
  /** 现有规则表里正好有这一条 */
  adopted: boolean
  /** 采样到的地址，作为线索 */
  samples: string[]
}

/** 学习报告明细（来自会话目录的 learn.json，按需拉取） */
export interface LearnReport {
  enabled: boolean
  scans: number
  updated_at: string
  hosts: LearnedHost[]
  candidates: LearnedSubnet[]
}

/** 状态文件里的学习模式摘要 */
export interface LearnSummary {
  enabled: boolean
  scans: number
  hosts: number
  candidates: number
  /** 还没进规则表的候选数量 */
  uncovered: number
  /** 采样失败的说明；连续失败会自动停用 */
  error: string
  updated_at: string
}

/** 连接状态 */
export interface VpnStatus {
  phase: VpnPhase
  message: string
  profile_id: string
  profile_name: string
  connector: string
  connector_name: string
  tun_dev: string
  tun_index: number
  vpn_ip: string
  vpn_gateway: string
  dns: string[]
  mtu: number
  routes: AppliedRoute[]
  snapshot: NetworkSnapshot | null
  /** 学习模式摘要；没开启学习模式时为 null。明细要走 vpn_read_learn_report */
  learn: LearnSummary | null
  error: string
  pid: number
  started_at: string
  updated_at: string
}

export interface VpnSnapshotPayload {
  status: VpnStatus
  elevation: string
}

export interface ConnectResult {
  connector_name: string
  connector_path: string
  elevation: string
}

export interface ElevationInfo {
  elevated: boolean
  task_ready: boolean
  platform: string
  hint: string
}

export interface GuideStep {
  title: string
  detail: string
  command: string
}

export interface GuideLink {
  label: string
  url: string
}

export interface InstallGuide {
  platform: string
  title: string
  description: string
  steps: GuideStep[]
  links: GuideLink[]
}

/** 会话目录的实际位置与来源 */
export interface SessionDirInfo {
  path: string
  /** 应用数据目录 / 本地数据目录 / 临时目录 */
  origin: string
  /** 是否发生了回退：首选目录不可写时才会发生 */
  fallback: boolean
}

/** 阶段到界面的展示映射 */
export const PHASE_LABELS: Record<VpnPhase, string> = {
  disconnected: '未连接',
  preparing: '准备中',
  elevating: '等待提权',
  connecting: '连接中',
  configuring_routes: '配置路由',
  connected: '已连接',
  disconnecting: '断开中',
  failed: '失败',
}

/** 规则类型到展示文案的映射（沿用 Clash 规范大写连字符形式） */
export const RULE_KIND_LABELS: Record<VpnRuleKind, string> = {
  DOMAIN: 'DOMAIN',
  'DOMAIN-SUFFIX': 'DOMAIN-SUFFIX',
  'DOMAIN-KEYWORD': 'DOMAIN-KEYWORD',
  'PROCESS-NAME': 'PROCESS-NAME',
  'DST-PORT': 'DST-PORT',
  'IP-CIDR': 'IP-CIDR',
}

/** 规则类型的中文含义，只用于下拉选项，不改动规则值本身 */
export const RULE_KIND_HINTS: Record<VpnRuleKind, string> = {
  DOMAIN: '域名',
  'DOMAIN-SUFFIX': '域名后缀',
  'DOMAIN-KEYWORD': '域名关键词',
  'PROCESS-NAME': '进程名',
  'DST-PORT': '目标端口',
  'IP-CIDR': 'IP 网段',
}

export const POLICY_LABELS: Record<VpnPolicy, string> = {
  DIRECT: '直连',
  VPN: '走 VPN',
}

/** 连接中的阶段，用于禁用按钮 */
export const BUSY_PHASES: VpnPhase[] = [
  'preparing',
  'elevating',
  'connecting',
  'configuring_routes',
  'disconnecting',
]
