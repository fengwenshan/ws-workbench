import { invoke } from '@tauri-apps/api/core'
import type { ProjectType, VcsInfo } from '@/types'

/** 项目识别相关命令 */

export function detectProjectType(sourceDir: string): Promise<ProjectType> {
  return invoke<ProjectType>('detect_project_type', { sourceDir })
}

export function detectVcs(sourceDir: string): Promise<VcsInfo> {
  return invoke<VcsInfo>('detect_vcs', { sourceDir })
}

/** 批量识别（快速搜索用）：返回顺序与入参一致 */
export function detectVcsMany(sourceDirs: string[]): Promise<VcsInfo[]> {
  return invoke<VcsInfo[]>('detect_vcs_many', { sourceDirs })
}
