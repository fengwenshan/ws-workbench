/**
 * 「屏幕休眠」页签（保持唤醒）的前后端契约。
 *
 * 字段名与后端 serde 一致，全部 snake_case（与 `src/types/cleaner.ts` 同一约定）；
 * invoke 参数按 Tauri 习惯用 camelCase，见 `src/api/sleep.ts`。
 */

/** 防休眠状态（后端 `cleaner::sleep::KeepAwakeStatus`） */
export interface KeepAwakeStatus {
  /** 是否正在阻止空闲睡眠 */
  active: boolean
  /** true 表示「允许屏幕关闭」，即只拦系统空闲睡眠 */
  allow_display_sleep: boolean
  /** 开始时刻（epoch 毫秒）；未激活为 null */
  started_at_ms: number | null
  /** 剩余秒数；无限时长与未激活都是 null */
  remaining_secs: number | null
  /** 平台限制说明（后端按平台给中文文案，直接展示） */
  platform_hint: string
}

/** 时长选项：0 表示无限（设计 4.1：15/30/60/120 分钟 / 无限） */
export type KeepAwakeDurationMinutes = 0 | 15 | 30 | 60 | 120
