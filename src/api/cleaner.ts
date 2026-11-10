import { invoke } from '@tauri-apps/api/core'
import type {
  DeleteOutcome,
  ForceQuitOutcome,
  InstalledApp,
  LeftoverItem,
  RelatedFiles,
  RunningState,
  UninstallOutcome,
} from '@/types/cleaner'

/** 系统清理工具的命令入口；命令名与设计 3.2 一致 */

/** 进度事件名，与后端 `AppHandle.emit("cleaner-progress", …)` 保持一致 */
export const CLEANER_PROGRESS_EVENT = 'cleaner-progress'

/** 卸载/删除成功后广播：主窗口收到后重新拉安装列表 */
export const CLEANER_APPS_CHANGED_EVENT = 'cleaner-apps-changed'

/** 卸载窗口被复用、切换目标时广播：窗口重新拉目标与清单 */
export const CLEANER_TARGET_CHANGED_EVENT = 'cleaner-uninstall-target-changed'

/**
 * 列出已安装应用。
 * 有缓存直接返回；refresh=true 重新扫描；扫描中重复调用后端会返回「正在扫描」错误。
 */
export function listInstalledApps(refresh = false): Promise<InstalledApp[]> {
  return invoke<InstalledApp[]>('cleaner_list_apps', { refresh })
}

/** 检测应用是否仍在运行（只读） */
export function appRunning(appId: string): Promise<RunningState> {
  return invoke<RunningState>('cleaner_app_running', { appId })
}

/**
 * 执行卸载（阻塞到卸载程序退出或 15 分钟超时）。
 * `quitRunning=true` 时后端先优雅退出应用；退不掉直接返回 ok=false。
 */
export function uninstallApp(
  appId: string,
  quitRunning = false,
): Promise<UninstallOutcome> {
  return invoke<UninstallOutcome>('cleaner_uninstall_app', { appId, quitRunning })
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

// ============================ 独立卸载窗口 ============================

/**
 * 打开独立卸载窗口并把目标切到 `appId`。
 * 后端保证单例：窗口已存在时聚焦 + 广播 `cleaner-uninstall-target-changed`。
 */
export function openUninstallWindow(appId: string): Promise<void> {
  return invoke<void>('cleaner_open_uninstall_window', { appId })
}

/** 卸载窗口读取当前目标应用（没有目标时后端报错） */
export function uninstallTarget(): Promise<InstalledApp> {
  return invoke<InstalledApp>('cleaner_uninstall_target')
}

/** 扫描三组关联文件；后端同时把本次扫描到的路径写成删除白名单 */
export function relatedFiles(appId: string): Promise<RelatedFiles> {
  return invoke<RelatedFiles>('cleaner_related_files', { appId })
}

/** 强制退出目标应用的全部相关进程（阻塞到进程归零或超时） */
export function forceQuitApp(appId: string): Promise<ForceQuitOutcome> {
  return invoke<ForceQuitOutcome>('cleaner_force_quit', { appId })
}

/** 删除勾选的关联文件；只接受本次扫描缓存里的路径，已消失的记 skipped */
export function deleteRelated(
  appId: string,
  paths: string[],
): Promise<DeleteOutcome> {
  return invoke<DeleteOutcome>('cleaner_delete_related', { appId, paths })
}
