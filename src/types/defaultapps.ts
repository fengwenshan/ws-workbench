/**
 * 「默认应用」页签的前后端契约（设计 4.4）。
 *
 * 字段名与后端 serde 一致，全部 snake_case；invoke 参数按 Tauri 习惯用 camelCase，
 * 见 `src/api/defaultapps.ts`。
 */

/** 常用文件类型的当前默认打开应用（列表一行） */
export interface DefaultAppEntry {
  /** 展示用扩展名，带点：`.txt` */
  extension: string
  /** 当前默认应用名；没有关联时为「未设置」 */
  current_app_name: string
  /** macOS 是 bundle id，Windows 是注册表 ProgId；没有关联时为空串 */
  current_app_id: string
  /** 能否在本应用内直接修改（macOS true；Windows false，系统限制） */
  can_change: boolean
  /** 备注：UTI / ProgId / 引导说明 */
  detail: string
}

/** 候选应用（macOS：本机能打开该类型的 bundle） */
export interface DefaultAppCandidate {
  /** macOS 是 bundle id；Windows 为空串 */
  app_id: string
  name: string
  /** `.app` 包路径；Windows 为空串 */
  path: string
}
