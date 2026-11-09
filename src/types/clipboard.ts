/** 剪贴板历史类型（与 Rust 侧命令返回值对应） */

export type ClipboardKind = 'text' | 'url' | 'code' | 'image' | 'rich'

export interface ClipboardImageMeta {
  id: string
  mime: string
  width: number
  height: number
  orig_size: number
}

/** 列表项摘要：正文只给 240 字预览，完整内容留在库里 */
export interface ClipboardItemSummary {
  id: string
  kind: ClipboardKind
  created_at: number
  text_preview: string
  image_count: number
  byte_size: number
  images: ClipboardImageMeta[]
}

export interface ClipboardPage {
  items: ClipboardItemSummary[]
  has_more: boolean
}

export interface ClipboardStats {
  items: number
  images: number
  bytes: number
}

export interface ClipboardClearResult {
  removed_items: number
  removed_images: number
}

export interface ClipboardLimits {
  /** 呼出面板的加速键字符串（`CommandOrControl+Shift+V`），显示前按平台格式化 */
  shortcut: string
  max_text_chars: number
}
