import { invoke } from '@tauri-apps/api/core'
import type { HostsBackup, HostsSaveResult, HostsStatus } from '@/types/hosts'

/** 「Hosts 在线编辑」的命令入口；命令名与 lib.rs 已注册的一致 */

/** 页面状态：路径 / 是否可读 / 完整内容 / 行数 / 备份 / 提示 */
export function hostsStatus(): Promise<HostsStatus> {
  return invoke<HostsStatus>('hosts_status')
}

/** 保存整份 hosts 内容：会弹一次系统授权框，写入前自动备份 */
export function hostsSave(content: string): Promise<HostsSaveResult> {
  return invoke<HostsSaveResult>('hosts_save', { content })
}

/** 备份列表（新→旧） */
export function hostsBackups(): Promise<HostsBackup[]> {
  return invoke<HostsBackup[]>('hosts_backups')
}

/** 从备份还原 hosts（同样走提权 + 刷新 DNS，还原前会先备份当前文件） */
export function hostsRestore(backupId: string): Promise<HostsSaveResult> {
  return invoke<HostsSaveResult>('hosts_restore', { backupId })
}

/** 打开 hosts 所在目录 */
export function hostsOpenFileDir(): Promise<void> {
  return invoke('hosts_open_file_dir')
}
