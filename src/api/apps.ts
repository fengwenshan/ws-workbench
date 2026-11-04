import { invoke } from '@tauri-apps/api/core'
import type { AppId, AppLocation } from '@/types/apps'

/** 外部应用定位与路径配置命令 */

export function listAppLocations(): Promise<AppLocation[]> {
  return invoke<AppLocation[]>('list_app_locations')
}

/** 重新检测并清掉后端缓存 */
export function refreshAppSearch(): Promise<AppLocation[]> {
  return invoke<AppLocation[]>('refresh_app_search')
}

export function setAppPath(
  appId: AppId,
  path: string,
): Promise<AppLocation[]> {
  return invoke<AppLocation[]>('set_app_path', { appId, path })
}

export function clearAppPath(appId: AppId): Promise<AppLocation[]> {
  return invoke<AppLocation[]>('clear_app_path', { appId })
}

export function importApp(appId: AppId, sourceDir: string): Promise<string> {
  return invoke<string>('import_app', { appId, sourceDir })
}

export function removeManagedApp(appId: AppId): Promise<string> {
  return invoke<string>('remove_managed_app', { appId })
}
