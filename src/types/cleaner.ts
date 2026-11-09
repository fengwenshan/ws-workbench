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
  | 'crash_report'
  | 'receipt'

/** 残留置信度：只有 high 默认勾选 */
export type LeftoverConfidence = 'high' | 'medium' | 'low'

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

/** 应用运行状态（cleaner_app_running 返回值） */
export interface RunningState {
  running: boolean
  /** 主进程 + helper 进程数（按 pid 去重） */
  processes: number
  /** 已去重的进程名，前端只展示前几个 */
  names: string[]
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

/** 残留扫描 / 卸载进度事件 `cleaner-progress` 的 payload */
export interface CleanerProgress {
  task: CleanerTask | string
  phase: CleanerPhase | string
  current: number
  total: number
  message: string
}
