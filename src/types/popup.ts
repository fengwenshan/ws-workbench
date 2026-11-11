/**
 * 「弹窗拦截」页签（仅 Windows）的前后端契约。
 *
 * 字段名与后端 serde 一致，全部 snake_case；invoke 参数按 Tauri 习惯用 camelCase。
 */

/** 检测状态（后端 `cleaner::popup::PopupStatus`） */
export interface PopupStatus {
  /** 当前平台是否支持（非 Windows 恒为 false） */
  supported: boolean
  /** 拦截线程是否在跑 */
  running: boolean
  /** 缓冲区里当前记录数（上限 300） */
  count: number
  /** 其中已自动拦截的条数 */
  blocked: number
}

/** 一条弹窗记录：要么在等用户处理，要么已经被自动处理 */
export interface PopupHit {
  id: number
  /** 出现时间（epoch 毫秒） */
  at_ms: number
  /** 进程名，如 `wps.exe` */
  process: string
  /** 进程完整路径（取图标 / 添加规则用，可能为空串） */
  path: string
  /** 窗口标题（已截断到 120 字符） */
  title: string
  /** 窗口类名 */
  class_name: string
  width: number
  height: number
  /** 相对屏幕的位置（右下窗口 / 中间窗口 / 任意位置） */
  position: string
  /** 可疑度分值，越大越可疑；命中规则时为 0 */
  score: number
  /** 命中原因，直接展示 */
  reasons: string[]
  /** 已自动处理的动作：`null` 表示还在等用户处理 */
  auto_action: 'close' | 'hide' | null
  /** 命中的规则 id（智能拦截 / 纯检测时为空） */
  rule_id: string | null
  /** 记录来源 */
  source: 'rule' | 'smart' | 'detect'
}

/** 规则的匹配范围：仅此弹窗 / 同类弹窗 / 该程序全部 */
export type PopupScope = 'window' | 'class' | 'app'
export type PopupAction = 'close' | 'hide'

/** 拦截规则（后端 `cleaner::popup::PopupRule`） */
export interface PopupRule {
  id: string
  process: string
  /** 进程完整路径，仅用于展示图标 */
  path: string
  /** 窗口类名（空 = 不按类名匹配） */
  class_name: string
  /** 标题子串（空 = 不按标题匹配） */
  title: string
  /** 展示名（火绒那种「广告弹窗 / 红包 / 软件推荐」），不参与匹配 */
  name: string
  /** 弹窗通常出现的位置（右下窗口 / 中间窗口 / 任意位置），仅展示 */
  position: string
  scope: PopupScope
  action: PopupAction
  enabled: boolean
  /** 累计命中次数 */
  hits: number
  /** 最近命中时间（epoch 毫秒，0 表示还没命中过） */
  last_hit_ms: number
  created_ms: number
}

/** 新增规则时提交的草稿 */
export interface PopupRuleDraft {
  process: string
  path: string
  class_name: string
  title: string
  /** 可选展示名；为空时后端按标题自动生成 */
  name?: string
  /** 可选位置；从窗口添加时带上 */
  position?: string
  scope: PopupScope
  action: PopupAction
}

/** 新增规则的结果：重复添加会合并到 `id` 指向的已有规则 */
export interface PopupRuleAddResult {
  id: string
  rules: PopupRule[]
}

/** 拦截设置（后端 `cleaner::popup::PopupSettings`） */
export interface PopupSettings {
  /** 总开关（`start` / `stop` 维护，后端会忽略前端传值） */
  enabled: boolean
  /** 新规则的默认动作，也用于智能拦截 */
  action: PopupAction
  /** 智能拦截：没有命中规则时也自动处理高可疑窗口 */
  smart: boolean
  /** 智能拦截的最低可疑分（后端收进 5–30） */
  min_score: number
  /** 每分钟最多自动处理几个窗口（后端收进 1–30） */
  max_per_minute: number
}

/** 「从窗口添加规则」里的一行 */
export interface PopupWindow {
  process: string
  path: string
  title: string
  class_name: string
  width: number
  height: number
  /** 相对屏幕的位置（右下窗口 / 中间窗口 / 任意位置） */
  position: string
  score: number
  reasons: string[]
  /** 是否是该应用的主窗口（主窗口永远不会被拦截） */
  main: boolean
  /** 系统外壳窗口、忽略名单里的程序、应用主窗口时为 false */
  blockable: boolean
  /** 是否在忽略名单里 */
  ignored: boolean
  /** 已经有一条启用中的规则能命中它 */
  ruled: boolean
}

/** 应用展示信息（exe 版本信息里的名字 / 图标） */
export interface PopupAppMeta {
  name: string
  icon: string | null
}
