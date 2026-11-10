/**
 * 「系统清理」工具的前后端契约。
 *
 * 字段名与后端 serde 一致，全部 snake_case（设计文档 3.1）；
 * invoke 参数按 Tauri 习惯用 camelCase，见 `src/api/cleaner.ts`。
 */

/** 后台任务：安装列表扫描 / 残留扫描 / 卸载执行 / 关联文件扫描 / 强制退出 */
export type CleanerTask =
  | 'list_apps'
  | 'scan_leftovers'
  | 'uninstall'
  | 'related_files'
  | 'force_quit'

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

/** 批量删除结果：成功条数 + 跳过条数 + 逐条失败原因 */
export interface DeleteOutcome {
  /** 成功移入废纸篓的条数 */
  trashed: number
  /** 卸载程序已经删掉 / 扫描后消失的路径，不算失败（后端 serde 默认 0） */
  skipped: number
  failed: DeleteFailure[]
}

// ============================ 关联文件（独立卸载窗口） ============================

/** 关联文件分组 id，与后端 `RelatedFileGroup.id` 一致 */
export type RelatedFileGroupId = 'program' | 'support' | 'cache'

/** 关联文件条目类型，与后端 `RelatedFileItem.kind` 一致 */
export type RelatedFileKind =
  | 'app_bundle'
  | 'app_data'
  | 'cache'
  | 'preference'
  | 'container'
  | 'launch_agent'
  | 'log'
  | 'crash_report'
  | 'install_dir'
  | 'registry'

/** 卸载窗口里的一条关联文件（后端扫描结果，字段 snake_case） */
export interface RelatedFileItem {
  /** 文件/目录绝对路径；Windows 注册表项为 "win:{hive}|{key_path}" */
  path: string
  /** 展示名（应用名 / 目录名 / 注册表键名） */
  name: string
  kind: RelatedFileKind | string
  size_bytes: number
  is_dir: boolean
  /** 仅 app_bundle 有版本号；未知为空串 */
  version: string
  confidence: LeftoverConfidence | string
  requires_elevation: boolean
  exists: boolean
  /** 默认勾选：confidence != "low" */
  selected: boolean
}

/** 关联文件分组：程序文件 / 支持文件 / 缓存文件 */
export interface RelatedFileGroup {
  id: RelatedFileGroupId | string
  label: string
  items: RelatedFileItem[]
}

/** `cleaner_related_files` 的返回值：三组清单 + 合计 */
export interface RelatedFiles {
  app_id: string
  groups: RelatedFileGroup[]
  /** 所有条目合计字节数 */
  total_bytes: number
  item_count: number
  /** "今天使用过" / "7天内使用过" / "30天内使用过" / "最近未使用" / "" */
  last_used: string
}

/** `cleaner_force_quit` 的返回值：是否已全部退出 */
export interface ForceQuitOutcome {
  /** true = 所有相关进程已退出 */
  ok: boolean
  /** 结束时仍在运行的进程数（ok 时为 0） */
  processes: number
  names: string[]
  message: string
}

/** 残留扫描 / 卸载进度事件 `cleaner-progress` 的 payload */
export interface CleanerProgress {
  task: CleanerTask | string
  phase: CleanerPhase | string
  current: number
  total: number
  message: string
}
