import { invoke } from '@tauri-apps/api/core'
import type { PopupHit, PopupStatus } from '@/types/popup'

/** 「弹窗拦截」页签的命令入口（仅 Windows 有真实实现） */

/** 事件名：后端检测到新弹窗时推送，payload 是 PopupHit */
export const POPUP_DETECTED_EVENT = 'popup-detected'

export function popupStatus(): Promise<PopupStatus> {
  return invoke<PopupStatus>('popup_status')
}

/** 开启检测（只观察，不自动关闭任何窗口） */
export function popupStart(): Promise<PopupStatus> {
  return invoke<PopupStatus>('popup_start')
}

export function popupStop(): Promise<PopupStatus> {
  return invoke<PopupStatus>('popup_stop')
}

/** 取缓冲区里的记录（最多 200 条） */
export function popupRecent(): Promise<PopupHit[]> {
  return invoke<PopupHit[]>('popup_recent')
}

export function popupClear(): Promise<void> {
  return invoke('popup_clear')
}

/** 用户逐条处理：action = 'close' | 'hide' */
export function popupDismiss(id: number, action: 'close' | 'hide'): Promise<void> {
  return invoke('popup_dismiss', { id, action })
}

/** 按进程名加入 / 移出忽略名单，返回最新名单 */
export function popupIgnore(process: string, ignored: boolean): Promise<string[]> {
  return invoke<string[]>('popup_ignore', { process, ignored })
}
