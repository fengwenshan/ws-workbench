import { invoke } from '@tauri-apps/api/core'
import type {
  PopupAction,
  PopupAppMeta,
  PopupHit,
  PopupRule,
  PopupRuleAddResult,
  PopupRuleDraft,
  PopupSettings,
  PopupStatus,
  PopupWindow,
} from '@/types/popup'

/** 「弹窗拦截」页签的命令入口（仅 Windows 有真实实现） */

/** 事件名：后端检测到新弹窗（或规则命中拦截）时推送，payload 是 PopupHit */
export const POPUP_DETECTED_EVENT = 'popup-detected'

export function popupStatus(): Promise<PopupStatus> {
  return invoke<PopupStatus>('popup_status')
}

/** 开启拦截（同时持久化总开关，下次启动自动恢复） */
export function popupStart(): Promise<PopupStatus> {
  return invoke<PopupStatus>('popup_start')
}

export function popupStop(): Promise<PopupStatus> {
  return invoke<PopupStatus>('popup_stop')
}

/** 取记录（最新在前，最多 300 条） */
export function popupRecent(): Promise<PopupHit[]> {
  return invoke<PopupHit[]>('popup_recent')
}

export function popupClear(): Promise<void> {
  return invoke('popup_clear')
}

/** 用户手动处理一条待处理记录：action = 'close' | 'hide' */
export function popupDismiss(id: number, action: PopupAction): Promise<void> {
  return invoke('popup_dismiss', { id, action })
}

/** 按进程名加入 / 移出忽略名单（白名单），返回最新名单 */
export function popupIgnore(process: string, ignored: boolean): Promise<string[]> {
  return invoke<string[]>('popup_ignore', { process, ignored })
}

export function popupIgnored(): Promise<string[]> {
  return invoke<string[]>('popup_ignored')
}

export function popupSettingsGet(): Promise<PopupSettings> {
  return invoke<PopupSettings>('popup_settings_get')
}

/** 保存拦截设置；后端会校验并返回收进安全范围后的值 */
export function popupSettingsSet(settings: PopupSettings): Promise<PopupSettings> {
  return invoke<PopupSettings>('popup_settings_set', { settings })
}

export function popupRules(): Promise<PopupRule[]> {
  return invoke<PopupRule[]>('popup_rules')
}

/**
 * 新增规则；`applyNow` 为 true 时，当前已经开着的同类窗口会立刻被处理。
 * 重复的匹配条件会合并到已有规则上（返回的 id 是最终规则）。
 */
export function popupRuleAdd(
  draft: PopupRuleDraft,
  applyNow = false,
): Promise<PopupRuleAddResult> {
  return invoke<PopupRuleAddResult>('popup_rule_add', { draft, applyNow })
}

export function popupRuleRemove(id: string): Promise<PopupRule[]> {
  return invoke<PopupRule[]>('popup_rule_remove', { id })
}

export function popupRuleToggle(id: string, enabled: boolean): Promise<PopupRule[]> {
  return invoke<PopupRule[]>('popup_rule_toggle', { id, enabled })
}

/** 按应用整体启用 / 停用（火绒式「每个软件一行」的开关） */
export function popupAppToggle(process: string, enabled: boolean): Promise<PopupRule[]> {
  return invoke<PopupRule[]>('popup_app_toggle', { process, enabled })
}

/** 对已有规则立即执行一次，返回处理了几个窗口 */
export function popupRuleApply(ruleId: string): Promise<number> {
  return invoke<number>('popup_rule_apply', { ruleId })
}

/** 当前可见的顶层窗口（窗口选择器用） */
export function popupWindows(): Promise<PopupWindow[]> {
  return invoke<PopupWindow[]>('popup_windows')
}

/** 批量取应用展示名 + 图标；没有缓存的路径会走一次 PowerShell（可能 1~2 秒） */
export function popupAppMeta(paths: string[]): Promise<Record<string, PopupAppMeta>> {
  return invoke<Record<string, PopupAppMeta>>('popup_app_meta', { paths })
}
