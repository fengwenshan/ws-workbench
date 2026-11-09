import { invoke } from '@tauri-apps/api/core'
import { isWindows } from '@/platform'
import type {
  ClipboardClearResult,
  ClipboardItemSummary,
  ClipboardLimits,
  ClipboardPage,
  ClipboardStats,
} from '@/types/clipboard'

/**
 * 剪贴板历史命令封装。
 *
 * 图片不直接读文件：统一走 `wsclip` 自定义协议，由 Rust 侧解密后返回，
 * 前端只拿 URL（和笔记图片的 `wst-img` 同一套约定）。
 */

export function clipboardPage(
  beforeCreatedAt?: number,
  beforeId?: string,
  limit?: number,
): Promise<ClipboardPage> {
  return invoke<ClipboardPage>('clipboard_page', {
    beforeCreatedAt: beforeCreatedAt ?? null,
    beforeId: beforeId ?? null,
    limit: limit ?? null,
  })
}

/** 后端解密文本 / HTML 后按关键词匹配，最多返回 100 条 */
export function clipboardSearch(query: string, limit?: number): Promise<ClipboardItemSummary[]> {
  return invoke<ClipboardItemSummary[]>('clipboard_search', { query, limit: limit ?? null })
}

export function clipboardStats(): Promise<ClipboardStats> {
  return invoke<ClipboardStats>('clipboard_stats')
}

/** 把一条历史写回系统剪贴板（不再自动收起面板，由调用方决定） */
export function copyClipboardItem(id: string): Promise<void> {
  return invoke<void>('clipboard_copy', { id })
}

/** 只复制某张图片附件到剪贴板 */
export function copyClipboardImage(attachmentId: string): Promise<void> {
  return invoke<void>('clipboard_copy_image', { attachmentId })
}

/** 删除单条历史（级联删本地图片附件），返回删掉的图片数 */
export function deleteClipboardItem(id: string): Promise<number> {
  return invoke<number>('clipboard_delete', { id })
}

/** 清空全部历史（级联删本地图片附件） */
export function clearClipboardItems(): Promise<ClipboardClearResult> {
  return invoke<ClipboardClearResult>('clipboard_clear')
}

/** 把图片附件解密导出到指定路径 */
export function exportClipboardImage(attachmentId: string, targetPath: string): Promise<void> {
  return invoke<void>('clipboard_export_image', { attachmentId, targetPath })
}

export function hideClipboardPanel(): Promise<void> {
  return invoke<void>('clipboard_panel_hide')
}

export function getClipboardLimits(): Promise<ClipboardLimits> {
  return invoke<ClipboardLimits>('clipboard_limits')
}

/** 缩略图 / 原图的协议地址（macOS 用自定义 scheme，Windows 用 localhost 映射） */
export function clipboardImageUrl(id: string, variant: 'thumb' | 'full' = 'thumb'): string {
  const prefix = isWindows() ? 'http://wsclip.localhost' : 'wsclip://localhost'
  return `${prefix}/img?id=${encodeURIComponent(id)}&variant=${variant}`
}
