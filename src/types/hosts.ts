/**
 * 「Hosts 在线编辑」的前后端契约。
 *
 * 字段名与后端 serde 一致，全部 snake_case（与 `src/types/cleaner.ts` /
 * `src/types/sleep.ts` 同一约定）；invoke 参数按 Tauri 习惯用 camelCase，
 * 见 `src/api/hosts.ts`。
 *
 * 2026-11-09 修订：改为直接编辑整份 hosts 内容，不再有环境 / 条目结构。
 */

/** 页面状态（后端 `hosts::HostsStatus`） */
export interface HostsStatus {
  /** 固定路径：macOS `/etc/hosts`；Windows `C:\Windows\System32\drivers\etc\hosts` */
  path: string
  exists: boolean
  /** 是否已成功读出内容；false 时 content 为空、不能编辑 */
  readable: boolean
  /** 当前 hosts 完整内容（编辑器直接展示） */
  content: string
  line_count: number
  /** 内容字节数 */
  size: number
  backup_count: number
  last_backup_at: string
  /** macos / windows / linux */
  platform: string
  /** 写入 hosts 是否需要管理员授权 */
  needs_admin: boolean
  /** 平台相关的中文提示，直接展示 */
  hint: string
}

/** 保存 / 还原的结果（后端 `hosts::HostsSaveResult`） */
export interface HostsSaveResult {
  /** 是否真的执行了写操作（内容与磁盘一致时为 false） */
  saved: boolean
  path: string
  /** 本次写入前的备份 id（没有写入 / 原本没有 hosts 时为空串） */
  backup_id: string
  line_count: number
  size: number
  flushed_dns: boolean
  message: string
}

/** 一份备份（后端 `hosts::HostsBackup`） */
export interface HostsBackup {
  /** `<unix_ms>_hosts` */
  id: string
  /** 本地时间 `YYYY-MM-DD HH:MM:SS` */
  created_at: string
  size: number
}
