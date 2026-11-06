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
export type VpnRuleKind = 'DOMAIN' | 'DOMAIN-SUFFIX' | 'IP-CIDR'

/**
 * 历史上还有这几种规则类型，但需要「按连接抓包 + 按进程/端口匹配」的数据面，
 * 路由注入模式没有落点，后端会直接拒绝。留着这份名单只为把老配置标成「不生效」。
 */
export const RETIRED_RULE_KINDS = ['DOMAIN-KEYWORD', 'PROCESS-NAME', 'DST-PORT', 'RULE-SET'] as const

/** 规则走向，取值与后端 rule_engine::Policy 一致 */
export type VpnPolicy = 'DIRECT' | 'VPN'

/** 单条分流规则 */
export interface VpnRule {
  id: string
  kind: VpnRuleKind
  value: string
  policy: VpnPolicy
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

/** 连接状态 */
export interface VpnStatus {
  phase: VpnPhase
  message: string
  /** 本次会话实际生效的模式（界面上的选择只代表「下次连接」） */
  applied_mode?: string
  profile_id: string
  profile_name: string
  /** 通道身份哈希：连接参数相同的配置共享同一条隧道 */
  conn_identity: string
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
  error: string
  pid: number
  started_at: string
  updated_at: string
}

export interface VpnSnapshotPayload {
  status: VpnStatus
  elevation: string
  /** 运行期「当前生效配置」；同通道切换后是切过去的那份配置 */
  policy_profile_id: string
}

export interface ConnectResult {
  connector_name: string
  connector_path: string
  elevation: string
}

export interface ElevationInfo {
  elevated: boolean
  /** 任务存在，但未必指向当前程序/会话目录 */
  task_exists: boolean
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
  'IP-CIDR': 'IP-CIDR',
}

/** 规则类型的中文含义，只用于下拉选项，不改动规则值本身 */
export const RULE_KIND_HINTS: Record<VpnRuleKind, string> = {
  DOMAIN: '域名',
  'DOMAIN-SUFFIX': '域名后缀',
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
