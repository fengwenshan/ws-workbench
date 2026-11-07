/**
 * 「开机自启」页签的前后端契约（设计 4.2）。
 *
 * 字段名与后端 serde 一致，全部 snake_case；invoke 参数按 Tauri 习惯用
 * camelCase，见 `src/api/startup.ts`。
 */

/** 启动项来源：Windows 注册表 Run/RunOnce、启动文件夹；macOS LaunchAgents、登录项 */
export type StartupSource =
  | 'run'
  | 'run_once'
  | 'startup_folder'
  | 'launch_agent'
  | 'login_item'

/** 作用域：user 可切换，machine 需要管理员 / root，本版本只读 */
export type StartupScope = 'user' | 'machine'

/** 一条开机启动项 */
export interface StartupItem {
  /** 启停时回传给后端的唯一标识（win:/mac: 前缀） */
  id: string
  name: string
  /** 启动命令行；启动文件夹里是快捷方式路径 */
  command: string
  source: StartupSource | string
  scope: StartupScope | string
  enabled: boolean
  /** 系统级 / RunOnce 为 false，只读展示 */
  can_toggle: boolean
  /** 来源位置、只读原因、授权提示等，直接展示给用户 */
  detail: string
}

/**
 * 自动化授权被拒时 macOS 后端会加这条只读占位项（与 Rust 常量
 * `LOGIN_ITEMS_UNAVAILABLE_ID` 保持一致），前端据此显示提示条。
 */
export const LOGIN_ITEMS_UNAVAILABLE_ID = 'mac:login-items:unavailable'
