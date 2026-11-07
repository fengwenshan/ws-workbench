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
  /** 当前会话由哪条自动规则开启；手动开启为 null */
  active_trigger: string | null
}

/** 自动保持规则的类型（后端 `sleep::triggers::TriggerKind`，serde snake_case） */
export type SleepTriggerKind = 'app_running' | 'ac_power' | 'external_display'

/** 一条自动保持规则 */
export interface SleepTrigger {
  id: string
  kind: SleepTriggerKind
  /** app_running 的进程名/片段；另外两类为空串 */
  value: string
  enabled: boolean
}

/** 当前探测到的环境条件（用于展示「现在为什么保持」） */
export interface DetectedConditions {
  on_ac_power: boolean
  external_display: boolean
  /** 当前命中的规则 id（即使被手动会话挡住也会上报） */
  matched_trigger: string | null
}

/** 自动保持规则的整体状态（后端 `sleep::triggers::TriggersState`） */
export interface TriggersState {
  /** 自动规则总开关 */
  armed: boolean
  triggers: SleepTrigger[]
  /** 当前由哪条规则真正生效 */
  active_trigger: string | null
  detected: DetectedConditions
}

/** 时长选项：0 表示无限（设计 4.1：15/30/60/120 分钟 / 无限） */
export type KeepAwakeDurationMinutes = 0 | 15 | 30 | 60 | 120
