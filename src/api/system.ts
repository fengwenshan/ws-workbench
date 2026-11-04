import { invoke } from '@tauri-apps/api/core'
import type { PlatformInfo } from '@/platform'

/** 系统集成相关命令：打开目录/文件、打开方式、目录校验、平台信息 */

export function openDir(path: string): Promise<string> {
  return invoke<string>('open_dir', { path })
}

/** 用系统默认程序打开文件（附件解密后的预览等） */
export function openFile(path: string): Promise<void> {
  return invoke('open_file', { path })
}

/** 打开方式菜单里的一项：后端按平台探测到的、真正装了的外部应用 */
export interface OpenWithApp {
  id: string
  name: string
  /** `data:image/png;base64,...`，取不到图标时为 null */
  icon: string | null
  /** 平台默认应用（访达 / 资源管理器） */
  is_default: boolean
}

/** 列出本机可用的打开方式（后端带缓存） */
export function listOpenWithApps(): Promise<OpenWithApp[]> {
  return invoke<OpenWithApp[]>('list_open_with_apps')
}

/** 重新探测（刚装完软件时用） */
export function refreshOpenWithApps(): Promise<OpenWithApp[]> {
  return invoke<OpenWithApp[]>('refresh_open_with_apps')
}

/** 用指定应用打开目录；目录不存在时后端自动退到最近存在的上级 */
export function openWith(path: string, appId: string): Promise<void> {
  return invoke('open_with', { path, appId })
}

/** 校验并创建目录，返回规范化后的路径 */
export function ensureDir(path: string): Promise<string> {
  return invoke<string>('ensure_dir', { path })
}

/** 目录检测结果（只读，不创建目录） */
export interface DirCheck {
  exists: boolean
  is_dir: boolean
}

/** 检测记录里保存的目录是否还存在，用于「目录失效」提示 */
export function checkDir(path: string): Promise<DirCheck> {
  return invoke<DirCheck>('check_dir', { path })
}

export function getPlatformInfo(): Promise<PlatformInfo> {
  return invoke<PlatformInfo>('get_platform_info')
}

/** 一个 IP 地址（前缀长度取不到时为 null） */
export interface NetworkAddressInfo {
  ip: string
  /** IPv4 / IPv6 */
  family: string
  prefix: number | null
}

/** 一块网卡及其全部 IP；mac 为 null 表示 utun 这类没有 MAC 的虚拟网卡 */
export interface NetworkInterfaceInfo {
  name: string
  mac: string | null
  addresses: NetworkAddressInfo[]
}

/** 本机网络接口信息（设置页「运行环境」展示用） */
export interface NetworkInfo {
  interfaces: NetworkInterfaceInfo[]
}

export function getNetworkInfo(): Promise<NetworkInfo> {
  return invoke<NetworkInfo>('get_network_info')
}
