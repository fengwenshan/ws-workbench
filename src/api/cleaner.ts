import { invoke } from '@tauri-apps/api/core'
import type {
  DeleteOutcome,
  InstalledApp,
  LeftoverItem,
  MemorySnapshot,
  QuitOutcome,
  ReleaseOutcome,
  UninstallOutcome,
} from '@/types/cleaner'

/** 系统清理工具的命令入口；命令名与设计 3.2 一致 */

/** 进度事件名，与后端 `AppHandle.emit("cleaner-progress", …)` 保持一致 */
export const CLEANER_PROGRESS_EVENT = 'cleaner-progress'

/**
 * 列出已安装应用。
 * 有缓存直接返回；refresh=true 重新扫描；扫描中重复调用后端会返回「正在扫描」错误。
 */
export function listInstalledApps(refresh = false): Promise<InstalledApp[]> {
  return invoke<InstalledApp[]>('cleaner_list_apps', { refresh })
}

/** 执行卸载（阻塞到卸载程序退出或 15 分钟超时） */
export function uninstallApp(appId: string): Promise<UninstallOutcome> {
  return invoke<UninstallOutcome>('cleaner_uninstall_app', { appId })
}

/** 扫描卸载残留；也允许对未卸载的应用做预览 */
export function scanLeftovers(appId: string): Promise<LeftoverItem[]> {
  return invoke<LeftoverItem[]>('cleaner_scan_leftovers', { appId })
}

/** 删除残留：后端白名单校验后逐条进废纸篓，需提权的合并成一次系统授权 */
export function deleteLeftovers(
  appId: string,
  paths: string[],
): Promise<DeleteOutcome> {
  return invoke<DeleteOutcome>('cleaner_delete_leftovers', { appId, paths })
}

/** 单次内存采样（前端按 2 秒轮询） */
export function memorySnapshot(): Promise<MemorySnapshot> {
  return invoke<MemorySnapshot>('cleaner_memory_snapshot')
}

/** 结束进程：优雅退出优先；force=true 时直接强制结束 */
export function quitApps(appIds: string[], force = false): Promise<QuitOutcome> {
  return invoke<QuitOutcome>('cleaner_quit_apps', { appIds, force })
}

/** 深度释放（Windows 清工作集/待机列表，macOS 提权 purge） */
export function deepRelease(): Promise<ReleaseOutcome> {
  return invoke<ReleaseOutcome>('cleaner_deep_release')
}
