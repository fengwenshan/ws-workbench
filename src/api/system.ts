import { invoke } from '@tauri-apps/api/core'
import type { PlatformInfo } from '@/platform'

/** 系统集成相关命令：打开目录/终端/编辑器、目录校验、平台信息 */

export function openDir(path: string): Promise<string> {
  return invoke<string>('open_dir', { path })
}

export function openParentDir(path: string): Promise<void> {
  return invoke('open_parent_dir', { path })
}

export function openInTerminal(path: string): Promise<string> {
  return invoke<string>('open_in_terminal', { path })
}

export function openInVscode(path: string): Promise<void> {
  return invoke('open_in_vscode', { path })
}

export function openInIdea(path: string): Promise<void> {
  return invoke('open_in_idea', { path })
}

export function openInTrae(path: string): Promise<void> {
  return invoke('open_in_trae', { path })
}

/** 校验并创建目录，返回规范化后的路径 */
export function ensureDir(path: string): Promise<string> {
  return invoke<string>('ensure_dir', { path })
}

export function getPlatformInfo(): Promise<PlatformInfo> {
  return invoke<PlatformInfo>('get_platform_info')
}
