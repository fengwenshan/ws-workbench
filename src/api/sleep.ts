import { invoke } from '@tauri-apps/api/core'
import type { KeepAwakeStatus } from '@/types/sleep'

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
