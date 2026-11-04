/**
 * 操作系统判定：前端唯一的平台真相来源。
 *
 * 之前 `App.vue` 里是 `navigator.userAgent.includes("Macintosh")` 一个布尔值，
 * 只能区分「是不是 Mac」，非 Mac 就一律当 Windows 处理 ——
 * 在 Linux 上会把 macOS 的窗口内边距、Windows 的路径分隔符全用错。
 *
 * 现在统一走这里：
 * - 先按 userAgent 给一份同步兜底值，保证首帧渲染就有正确的分支
 * - 应用启动后调后端的 `get_platform_info` 覆盖成权威结果
 *   （后端用的是编译期 target，比 userAgent 可靠）
 */

import { reactive, readonly } from 'vue'
import { invoke } from '@tauri-apps/api/core'

export type Os = 'windows' | 'macos' | 'linux'

export interface PlatformInfo {
  os: Os
  /** 展示名：Windows / macOS / Linux */
  label: string
  arch: string
  target: string
  is_windows: boolean
  is_macos: boolean
  is_linux: boolean
  /** 可执行文件后缀 */
  exe_suffix: string
  /** VPN 是否必须提权 */
  vpn_needs_admin: boolean
  /** 是否支持「手动指定应用路径」这类 Windows 专有设置 */
  supports_app_path_settings: boolean
}

/** userAgent 兜底，仅在拿到后端结果之前生效 */
function fromUserAgent(): PlatformInfo {
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent
  let os: Os = 'linux'
  if (/Windows/i.test(ua)) os = 'windows'
  else if (/Macintosh|Mac OS X/i.test(ua)) os = 'macos'
  else if (/Linux|X11/i.test(ua)) os = 'linux'

  return {
    os,
    label: os === 'windows' ? 'Windows' : os === 'macos' ? 'macOS' : 'Linux',
    arch: '',
    target: '',
    is_windows: os === 'windows',
    is_macos: os === 'macos',
    is_linux: os === 'linux',
    exe_suffix: os === 'windows' ? '.exe' : '',
    vpn_needs_admin: true,
    supports_app_path_settings: os === 'windows',
  }
}

const state = reactive<PlatformInfo>(fromUserAgent())

/** 只读的平台信息，模板里直接用 `platform.is_windows` */
export const platform = readonly(state)

/** 是否已经从后端拿到权威结果 */
let resolved = false

/** 应用启动时调一次，用后端的编译期平台信息覆盖兜底值 */
export async function initPlatform(): Promise<PlatformInfo> {
  if (resolved) return state
  try {
    const info = await invoke<PlatformInfo>('get_platform_info')
    Object.assign(state, info)
    resolved = true
  } catch {
    // 拿不到就继续用 userAgent 的兜底值，不影响使用
  }
  return state
}

/** 便捷判断，避免到处写 platform.is_xxx */
export const isWindows = () => state.is_windows
export const isMacos = () => state.is_macos
export const isLinux = () => state.is_linux

/** Windows 专有设置（应用路径手动指定）是否可用 */
export const supportsAppPathSettings = () => state.supports_app_path_settings
