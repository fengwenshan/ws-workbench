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
}
