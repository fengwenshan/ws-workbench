/** 需要定位的外部应用（与后端 Rust 的 AppId 一一对应） */
export type AppId = 'vscode' | 'idea' | 'trae' | 'openconnect'

/** 找到的一个候选 */
export interface AppCandidate {
  path: string
  /** custom / managed / bundled / registry / system / path */
  source: string
  /** 是否为需要 cmd.exe 解释的 .cmd/.bat */
  script: boolean
  exists: boolean
}

/** 一个应用的定位结果 */
export interface AppLocation {
  id: string
  name: string
  description: string
  /** 最终选中的可执行文件路径；空表示没找到 */
  path: string
  available: boolean
  source: string
  /** 设置页里手动指定的路径 */
  override_path: string
  note: string
  /**
   * 随程序分发的内置副本路径。
   * 它不参与「查找」—— 位置在编译期就确定了，单独一行展示即可。
   */
  builtin_path: string
  /** 是否支持「导入到应用数据目录」 */
  supports_import: boolean
  /** 是否支持手动指定路径 */
  supports_override: boolean
  /** 手动指定时该选哪个文件，避免用户挑错 */
  expected_names: string[]
  candidates: AppCandidate[]
}

/** 来源标识到展示文案 */
export const SOURCE_LABELS: Record<string, string> = {
  custom: '手动指定',
  managed: '程序托管',
  bundled: '内置资源',
  registry: '注册表',
  system: '安装目录',
  path: 'PATH',
  missing: '未找到',
}

/** 来源标签的配色分组 */
export function sourceTone(source: string): 'ok' | 'muted' | 'warn' {
  if (source === 'custom') return 'warn'
  if (source === 'missing') return 'muted'
  return 'ok'
}
