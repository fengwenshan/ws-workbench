import { invoke } from '@tauri-apps/api/core'
import type { AppConfig } from '@/types'

/** 配置与存储相关命令的统一入口 */

export function loadAppConfig(): Promise<AppConfig> {
  return invoke<AppConfig>('get_config')
}

export function saveAppConfig(config: AppConfig): Promise<void> {
  return invoke('save_config', { config })
}

export function getConfigPath(): Promise<string> {
  return invoke<string>('get_config_path')
}

export interface StorageInfo {
  db_path: string
  settings_path: string
  ok: boolean
  error: string
}

export function getStorageInfo(): Promise<StorageInfo> {
  return invoke<StorageInfo>('storage_info')
}

export function getDefaultExcludeRules(): Promise<string[]> {
  return invoke<string[]>('get_default_exclude_rules')
}

/** 供 localStorage 加密层使用：整段文本加密/解密 */
export function encryptText(plain: string): Promise<string> {
  return invoke<string>('vault_encrypt', { plain })
}

export function decryptText(cipher: string): Promise<string> {
  return invoke<string>('vault_decrypt', { cipher })
}
