import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

/**
 * 快捷面板（/quick.html 独立窗口）与主进程之间的轻量契约。
 *
 * - `quick-panel`：快捷面板自身的模式由 Rust 侧的原子量决定（托盘左键有未读 →
 *   消息模式；右键 → 普通模式），首次创建时用 `quick_panel_take_mode` 取初始值；
 *   已存在的面板靠 `quick-panel://mode` 事件切换页签。
 * - `quick_panel_open_chat`：点消息列表里的会话，后端收起面板、显示主窗口并发
 *   `lan-share://open-chat`，主窗口据此切页并定位到会话。
 */

/** 快捷面板模式事件：已经打开的面板收到后切页签 */
export const EVENT_QUICK_PANEL_MODE = 'quick-panel://mode'

/** `messages` = 托盘左键因未读弹出的消息列表；`quick` = 常规（VPN / 休眠）面板 */
export type QuickPanelMode = 'messages' | 'quick'

interface QuickPanelModePayload {
  mode: QuickPanelMode
}

/** 取出并重置后端记录的面板模式（首次创建面板时决定初始页签） */
export function takeQuickPanelMode(): Promise<QuickPanelMode> {
  return invoke<QuickPanelMode>('quick_panel_take_mode')
}

/** 手动切换页签后回报模式，保证托盘左键「普通模式 → 切消息页签」判断准确 */
export function setQuickPanelMode(mode: QuickPanelMode): Promise<void> {
  return invoke<void>('quick_panel_set_mode', { mode })
}

/** 点消息列表里的会话：后端收起面板、显示主界面并通知主窗口定位 */
export function openQuickPanelChat(peerId: string): Promise<void> {
  return invoke<void>('quick_panel_open_chat', { peerId })
}

/** 订阅面板模式变化（面板已存在时托盘左键切换消息模式） */
export function onQuickPanelMode(handler: (mode: QuickPanelMode) => void): Promise<UnlistenFn> {
  return listen<QuickPanelModePayload>(EVENT_QUICK_PANEL_MODE, (event) =>
    handler(event.payload.mode),
  )
}
