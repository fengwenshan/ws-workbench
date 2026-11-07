import { invoke } from '@tauri-apps/api/core'
import type { DefaultAppCandidate, DefaultAppEntry } from '@/types/defaultapps'

/** 系统「默认应用」的命令入口；命令名与设计 4.4 一致 */

/** 首批扩展名的当前默认应用 */
export function listDefaultApps(): Promise<DefaultAppEntry[]> {
  return invoke<DefaultAppEntry[]>('defaultapps_list')
}

/** 能打开该扩展名的候选应用（macOS 真实返回；Windows 为空表） */
export function listDefaultAppCandidates(
  extension: string,
): Promise<DefaultAppCandidate[]> {
  return invoke<DefaultAppCandidate[]>('defaultapps_candidates', { extension })
}

/** 修改默认应用（macOS 生效；Windows 返回「需在系统设置里确认」的错误） */
export function setDefaultApp(extension: string, appId: string): Promise<void> {
  return invoke('defaultapps_set', { extension, appId })
}

/** 打开系统的「默认应用」设置页 */
export function revealDefaultAppsSettings(): Promise<void> {
  return invoke('defaultapps_reveal_settings')
}

/** 调起系统的「打开方式」对话框（Windows 专用） */
export function openDefaultAppWith(extension: string): Promise<void> {
  return invoke('defaultapps_open_with', { extension })
}
