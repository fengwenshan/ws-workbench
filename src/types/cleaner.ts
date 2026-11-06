/**
 * 「系统清理」工具的前后端契约。
 *
 * 字段名与后端 serde 一致，全部 snake_case（设计文档 3.1）；
 * invoke 参数按 Tauri 习惯用 camelCase，见 `src/api/cleaner.ts`。
 */

/** 后台任务：安装列表扫描 / 残留扫描 / 卸载执行 */
export type CleanerTask = 'list_apps' | 'scan_leftovers' | 'uninstall'

/** 任务阶段（`cleaner-progress` 事件里用） */
export type CleanerPhase =
  | 'scanning'
  | 'loading_icons'
  | 'sizing'
  | 'running'
  | 'waiting'
  | 'done'

/** 安装列表来源：Windows 注册表 / macOS Applications / 系统应用 */
export type InstalledAppSource = 'registry' | 'applications' | 'system'

/** 卸载方式；force 表示没有 UninstallString，只能强制移除 */
export type UninstallKind = 'msi' | 'exe' | 'app_bundle' | 'system' | 'force'

/** 残留条目类型 */
export type LeftoverKind =
  | 'install_dir'
  | 'app_data'
  | 'registry'
  | 'cache'
  | 'preference'
  | 'log'
  | 'launch_agent'
  | 'shortcut'
  | 'container'
  | 'receipt'

/** 残留置信度：只有 high 默认勾选 */
export type LeftoverConfidence = 'high' | 'medium' | 'low'

/** 内存条目来源：按应用聚合 / 找不到应用归属的独立进程 */
export type MemoryAppKind = 'app' | 'process'

/** 已安装应用（Windows 读注册表，macOS 读 /Applications） */
export interface InstalledApp {
  /** "win:{hive}|{key_path}" / "mac:{bundle_path}" */
  id: string
  name: string
  version: string
  publisher: string
  install_location: string
  /** 0 表示大小未知 */
  size_bytes: number
  /** `data:image/png;base64,...`，取不到图标时为 null */
  icon: string | null
  source: InstalledAppSource | string
  /** 系统应用 / SIP 保护应用为 false */
  can_uninstall: boolean
  uninstall_kind: UninstallKind | string
  /** YYYY-MM-DD；未知为空串 */
  install_date: string
  /** 备注：安装日期、注册表键、bundle id 等 */
  detail: string
}

/** 卸载执行结果 */
export interface UninstallOutcome {
  ok: boolean
  /** 实际执行的命令（完成后展示给用户） */
  command: string
  /** 拿不到退出码时为 null */
  exit_code: number | null
  elevated: boolean
  message: string
}

/** 卸载后扫出的残留条目 */
export interface LeftoverItem {
  path: string
  kind: LeftoverKind | string
  size_bytes: number
  is_dir: boolean
  confidence: LeftoverConfidence | string
  /** 后端建议的默认勾选状态（high 为 true） */
  selected: boolean
  requires_elevation: boolean
  exists: boolean
}

/** 单条删除失败 */
export interface DeleteFailure {
  path: string
  error: string
}

/** 批量删除结果：成功条数 + 逐条失败原因 */
export interface DeleteOutcome {
  trashed: number
  failed: DeleteFailure[]
}

/** 按应用聚合的内存占用 */
export interface MemoryApp {
  /** bundle id / 应用目录 / exe 路径 */
  id: string
  name: string
  exe: string
  icon: string | null
  process_count: number
  /** 该应用所有进程 RSS 之和（字节） */
  memory: number
  pids: number[]
  kind: MemoryAppKind | string
}

/** 一次内存采样快照 */
export interface MemorySnapshot {
  total: number
  used: number
  available: number
  used_percent: number
  swap_total: number
  swap_used: number
  /** 按 memory 降序 */
  apps: MemoryApp[]
  /** epoch 毫秒 */
  sampled_at: number
}

/** 结束进程结果 */
export interface QuitOutcome {
  quit: number
  forced: number
  /** 未能结束的应用名/标识（后端原样回传） */
  failed: string[]
}

/** 深度释放结果 */
export interface ReleaseOutcome {
  ok: boolean
  before_used: number
  after_used: number
  /** after 用系统重采样，不夸张 */
  freed: number
  message: string
  /** 原理说明，直接展示在 UI */
  note: string
}

/** 残留扫描 / 卸载进度事件 `cleaner-progress` 的 payload */
export interface CleanerProgress {
  task: CleanerTask | string
  phase: CleanerPhase | string
  current: number
  total: number
  message: string
}
