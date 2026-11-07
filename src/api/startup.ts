import { invoke } from '@tauri-apps/api/core'
import type { StartupItem } from '@/types/startup'

/** 开机自启工具的命令入口；命令名与设计 4.2 一致 */

/** 列出开机启动项（Windows：注册表 + 启动文件夹；macOS：LaunchAgents + 登录项） */
export function listStartupItems(): Promise<StartupItem[]> {
  return invoke<StartupItem[]>('startup_list')
}

/** 启用 / 禁用一条启动项；后端返回刷新后的完整列表 */
export function setStartupEnabled(
  id: string,
  enabled: boolean,
): Promise<StartupItem[]> {
  return invoke<StartupItem[]>('startup_set_enabled', { id, enabled })
}

/** 本应用是否设置为登录时启动 */
export function getAppAutostart(): Promise<boolean> {
  return invoke<boolean>('startup_app_autostart_get')
}

/** 设置 / 取消本应用登录时启动，返回设置后的状态 */
export function setAppAutostart(enabled: boolean): Promise<boolean> {
  return invoke<boolean>('startup_app_autostart_set', { enabled })
}
