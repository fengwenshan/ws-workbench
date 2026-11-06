import type { LeftoverConfidence, LeftoverKind } from '@/types/cleaner'

/** 容量格式化：0 / 非法值表示「未知」，显示破折号 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '—'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`
}

/** 内存数值：统一 GB/MB 两级，避免总览卡里出现 0.98 GB 这种读数 */
export function formatMemory(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB'
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(0)} MB`
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`
}

const LEFTOVER_KIND_LABELS: Record<LeftoverKind, string> = {
  install_dir: '安装目录',
  app_data: '应用数据',
  registry: '注册表',
  cache: '缓存',
  preference: '偏好设置',
  log: '日志',
  launch_agent: '启动项',
  shortcut: '快捷方式',
  container: '沙盒容器',
  receipt: '安装收据',
}

/** 残留类型的中文标签；后端未来新增的类型原样展示 */
export function leftoverKindLabel(kind: string): string {
  return LEFTOVER_KIND_LABELS[kind as LeftoverKind] ?? kind
}

const CONFIDENCE_LABELS: Record<LeftoverConfidence, string> = {
  high: '高置信',
  medium: '中置信',
  low: '低置信',
}

export function confidenceLabel(confidence: string): string {
  return CONFIDENCE_LABELS[confidence as LeftoverConfidence] ?? confidence
}

/** high 绿 / medium 黄 / low 灰，和设计里的标签配色一致 */
export function confidenceClass(confidence: string): string {
  if (confidence === 'high') return 'tag-ok'
  if (confidence === 'medium') return 'tag-warn'
  return 'tag-muted'
}
