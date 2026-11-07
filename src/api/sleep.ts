import { invoke } from '@tauri-apps/api/core'
import type { KeepAwakeStatus, SleepTriggerKind, TriggersState } from '@/types/sleep'

/** 「屏幕休眠」页签的命令入口；命令名与设计 4.1 一致 */

/** 当前防休眠状态（页签激活时每 5 秒轮询一次） */
export function keepAwakeStatus(): Promise<KeepAwakeStatus> {
  return invoke<KeepAwakeStatus>('keepawake_status')
}

/**
 * 开始保持唤醒。
 * @param durationSeconds 时长（秒），null 表示无限
 * @param allowDisplaySleep true 时允许屏幕关闭，只拦系统空闲睡眠
 */
export function keepAwakeStart(
  durationSeconds: number | null,
  allowDisplaySleep: boolean,
): Promise<KeepAwakeStatus> {
  return invoke<KeepAwakeStatus>('keepawake_start', {
    durationSeconds,
    allowDisplaySleep,
  })
}

/** 停止保持唤醒（后端幂等：未激活时也返回当前状态） */
export function keepAwakeStop(): Promise<KeepAwakeStatus> {
  return invoke<KeepAwakeStatus>('keepawake_stop')
}

// ============================== 自动保持规则 ==============================

/** 自动规则状态（含当前探测到的电源/显示器条件） */
export function sleepTriggersState(): Promise<TriggersState> {
  return invoke<TriggersState>('sleep_triggers_state')
}

/** 开 / 关自动规则总开关；关闭时后端会释放规则自己开的会话 */
export function sleepTriggersSetArmed(armed: boolean): Promise<TriggersState> {
  return invoke<TriggersState>('sleep_triggers_set_armed', { armed })
}

/**
 * 新增一条规则。
 * @param kind 规则类型
 * @param value app_running 填进程名/片段，其它类型传空串
 */
export function sleepTriggersAdd(
  kind: SleepTriggerKind,
  value: string,
): Promise<TriggersState> {
  return invoke<TriggersState>('sleep_triggers_add', { kind, value })
}

/** 删除一条规则 */
export function sleepTriggersRemove(id: string): Promise<TriggersState> {
  return invoke<TriggersState>('sleep_triggers_remove', { id })
}

/** 启用 / 停用一条规则（留档但不生效） */
export function sleepTriggersSetEnabled(
  id: string,
  enabled: boolean,
): Promise<TriggersState> {
  return invoke<TriggersState>('sleep_triggers_set_enabled', { id, enabled })
}
