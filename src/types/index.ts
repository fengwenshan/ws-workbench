import type { VpnProfile, VpnRuleSet } from './vpn'

export type EntityKind = 'group' | 'project'

export interface Selection {
  kind: EntityKind
  id: string
}

export interface AttachmentMeta {
  /** uuid v4，同时是密文 blob 文件名（不含扩展名） */
  id: string
  /** 原始文件名（含扩展名） */
  name: string
  /** 原始字节数 */
  size: number
  /** 扩展名（小写，不含点） */
  ext: string
  /** 毫秒时间戳 */
  created_at: number
  /** 上传时的绝对目录，blob 路径 = <dir>/<id>.wst */
  dir: string
}

/** 快速搜索（⌘/Ctrl+P）的附件条目：附件本身 + 所属实体，用于展示位置并跳转 */
export interface QuickAttachmentItem extends AttachmentMeta {
  entityKind: EntityKind
  entityId: string
  /** 面包屑：分组名，或「分组 / 项目」 */
  entityPath: string
}

/** 笔记内联资源的只读元信息（来自密文 WST1 明文头，不触发解密） */
export interface AttachmentStat {
  /** 原始（明文）字节数 */
  size: number
  /** 密文文件最后修改时间（毫秒时间戳），取不到为 0 */
  modified_at: number
}

export interface ProjectConfig {
  id: string
  name: string
  source_dir: string
  output_dir: string
  created_at: string
  updated_at: string
  note: string
  attachments: AttachmentMeta[]
}

export interface ProjectGroup {
  id: string
  name: string
  projects: ProjectConfig[]
  /** 是否为默认分组（默认分组不允许删除，但内部项目可删除） */
  is_default?: boolean
  created_at: string
  note: string
  attachments: AttachmentMeta[]
}

export interface AppConfig {
  groups: ProjectGroup[]
  projects: ProjectConfig[]
  default_include: string[]
  default_include_version?: number
  /** VPN 配置列表 */
  vpn_profiles: VpnProfile[]
  /** @deprecated 老版本全局规则集，只读，仅用于一次性迁移到各配置 */
  vpn_rule_sets?: VpnRuleSet[]
  /** 上次选中的 VPN 配置 */
  vpn_active_profile_id: string
  /** 全局默认附件上传目录 */
  default_upload_dir: string | null
}

export interface PackResult {
  success: boolean
  total_files: number
  copied_files: number
  skipped_files: number
  elapsed_ms: number
  errors: string[]
}

export interface PackProgress {
  phase: string
  current: number
  total: number
  current_file: string
  percentage: number
}

export type LogLevel = 'info' | 'warn' | 'error' | 'success'

export interface LogEntry {
  id: number
  timestamp: string
  level: LogLevel
  message: string
}

export interface ProjectValidation {
  valid: boolean
  source_exists: boolean
  file_count: number
  warnings: string[]
}

export type ProjectType =
  | 'vue2'
  | 'vue3'
  | 'react'
  | 'layui'
  | 'springboot'
  | 'spring'
  | 'struts2'
  | 'java'
  | 'unknown'

export type VcsType = 'git' | 'svn' | 'none'

export interface VcsInfo {
  vcs_type: VcsType
  /** 远程仓库地址，未检测到为空字符串 */
  url: string
}

export * from './vpn'
