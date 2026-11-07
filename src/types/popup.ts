/**
 * 「弹窗拦截」页签（仅 Windows）的前后端契约。
 *
 * 字段名与后端 serde 一致，全部 snake_case；invoke 参数按 Tauri 习惯用 camelCase。
 */

/** 检测状态（后端 `cleaner::popup::PopupStatus`） */
export interface PopupStatus {
  /** 当前平台是否支持（非 Windows 恒为 false） */
  supported: boolean
  /** 检测线程是否在跑 */
  running: boolean
  /** 缓冲区里当前记录数（上限 200） */
  count: number
}

/** 一条被记录下来的可疑弹窗 */
export interface PopupHit {
  id: number
  /** 出现时间（epoch 毫秒） */
  at_ms: number
  /** 进程名，如 `PotPlayerMini64.exe` */
  process: string
  /** 窗口标题（已截断到 120 字符） */
  title: string
  /** 窗口类名 */
  class_name: string
  width: number
  height: number
  /** 可疑度分值，越大越可疑 */
  score: number
  /** 命中原因，直接展示 */
  reasons: string[]
  /**
   * 拦截模式自动处理动作：`null` 表示只记录；
   * `close` / `hide` 表示拦截模式已经自动处理过
   */
  auto_action: string | null
}

/**
 * 拦截模式设置（后端 `cleaner::popup::PopupSettings`）。
 *
 * 默认全部关闭：只检测、不动手。打开后由后端闸门决定是否自动处理，
 * 系统进程/外壳窗口、忽略名单、同进程 5 秒冷却、每分钟上限都在后端兜着。
 */
export interface PopupSettings {
  /** 是否开启拦截模式 */
  intercept: boolean
  /** 自动处理动作 */
  action: 'close' | 'hide'
  /** 自动处理的最低可疑分（后端会收进 5–30） */
  min_score: number
  /** 每分钟最多自动处理几个窗口（后端会收进 1–30） */
  max_per_minute: number
}
