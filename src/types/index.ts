export interface ProjectConfig {
  id: string
  name: string
  source_dir: string
  output_dir: string
  exclude: string[]
  created_at: string
  updated_at: string
}

export interface ProjectGroup {
  id: string
  name: string
  projects: ProjectConfig[]
  /** 是否为默认分组（默认分组不允许删除，但内部项目可删除） */
  is_default?: boolean
  created_at: string
}

export interface AppConfig {
  groups: ProjectGroup[]
  projects: ProjectConfig[]
  default_exclude: string[]
  default_exclude_version?: number
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
