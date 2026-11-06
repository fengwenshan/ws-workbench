<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { Button, Empty, Input, Modal, Tooltip, message } from 'ant-design-vue'
import {
  CopyOutlined,
  DeleteOutlined,
  DoubleRightOutlined,
  EditOutlined,
  ExportOutlined,
  EyeOutlined,
  FileImageOutlined,
  FolderOpenOutlined,
  InboxOutlined,
  PlusOutlined,
  UploadOutlined,
} from '@ant-design/icons-vue'
import type { AttachmentStat, LogLevel } from '@/types'
import {
  decryptExportFile,
  decryptToTemp,
  encryptUploadBytes,
  encryptUploadFile,
  removeAttachmentFile,
  resolveUploadDir,
  statAttachment,
} from '@/api/attachments'
import { openFile } from '@/api/system'
import { pickFiles, pickSavePath } from '@/utils/dialog'
import { basename } from '@/utils/path'
import { errorText } from '@/utils/error'
import {
  noteAssetDisplayName,
  noteAssetFileName,
  noteAssetMarkdown,
  noteImageUrl,
  parseNoteAssets,
  removeNoteAssetFromMarkdown,
  renameNoteAssetInMarkdown,
  type NoteAsset,
} from '@/utils/note-assets'
import { parseNoteHeadings } from '@/utils/note-text'

/**
 * 笔记右侧「附件 / 大纲」侧栏。
 *
 * 数据流是单向的：正文由父级（NoteWorkspace）持有，这里只解析 props.markdown；
 * 插入 / 改正文 / 滚动大纲 / 收起都通过 emit 交给父级，本组件不碰编辑器、不持有正文。
 *
 * 附件来自正文里内联的加密资源地址（wst-img:// 或 http://wst-img.localhost），
 * 所以老笔记不需要额外元数据也能在这里列出来。
 */
const props = defineProps<{
  markdown: string
  /** 设置里配置的全局上传目录（null 表示落到 App 私有目录） */
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  /** 插入 markdown 到编辑器光标处 */
  insert: [markdown: string]
  /** 重命名 / 删除资源后的新正文 */
  updateMarkdown: [markdown: string]
  /** 大纲点击：跳转到第 index 个标题（0 起） */
  scrollToHeading: [index: number]
  collapse: []
  log: [level: LogLevel, message: string]
}>()

const activeTab = ref<'assets' | 'outline'>('assets')

const assets = computed<NoteAsset[]>(() => parseNoteAssets(props.markdown ?? ''))
const headings = computed(() => parseNoteHeadings(props.markdown ?? ''))

// ========== 运行环境 ==========
// 浏览器里（没有 Tauri 注入）所有 invoke 都会失败，先判断再调，避免刷一屏红色报错；
// 即便判断失效，下面每个 invoke 也都有 try/catch 兜底，不会把界面炸掉。
function hasTauriRuntime(): boolean {
  try {
    return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
  } catch {
    return false
  }
}

// ========== 附件大小（懒加载 + 缓存） ==========
type StatEntry = AttachmentStat | 'loading' | 'missing'

// key = 资源 url：正文里同一地址出现多次也只 stat 一次；失败的结果同样缓存，
// 避免每次 markdown 变化都对着丢失的文件反复重试。
const statCache = reactive(new Map<string, StatEntry>())

function ensureStat(asset: NoteAsset) {
  if (statCache.has(asset.url)) return
  if (!hasTauriRuntime()) {
    statCache.set(asset.url, 'missing')
    return
  }
  statCache.set(asset.url, 'loading')
  void statAttachment(asset.dir, asset.id)
    .then((stat) => statCache.set(asset.url, stat))
    .catch(() => statCache.set(asset.url, 'missing'))
}

watch(
  assets,
  (list) => {
    for (const asset of list) ensureStat(asset)
  },
  { immediate: true },
)

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

function assetMetaText(asset: NoteAsset): string {
  const ext = (asset.ext || '图片').toUpperCase()
  const entry = statCache.get(asset.url)
  if (entry === 'loading') return `${ext} · 读取中…`
  if (entry === 'missing') return `${ext} · 文件缺失`
  if (entry) return `${ext} · ${formatSize(entry.size)}`
  return ext
}

function isMissing(asset: NoteAsset): boolean {
  return statCache.get(asset.url) === 'missing'
}

function markMissing(asset: NoteAsset) {
  statCache.set(asset.url, 'missing')
}

// ========== 缩略图 ==========
// 自定义协议在浏览器里必然加载失败；失败后显示占位图标，不留白块。
const failedThumbs = reactive(new Set<string>())

function onThumbError(asset: NoteAsset) {
  failedThumbs.add(asset.url)
}

// ========== 插入 / 复制 ==========
function insertAsset(asset: NoteAsset) {
  emit('insert', noteAssetMarkdown(asset))
  emit('log', 'info', `已插入资源引用: ${noteAssetDisplayName(asset)}`)
}

/**
 * 写剪贴板：Tauri WebView 里 Clipboard API 可能被判成非安全上下文，
 * 失败就退回隐藏 textarea + execCommand（和 LogDrawer 一个套路）。
 */
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    // 继续走兜底
  }
  try {
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.style.position = 'fixed'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(textarea)
    return ok
  } catch {
    return false
  }
}

async function copyAssetMarkdown(asset: NoteAsset) {
  const name = noteAssetDisplayName(asset)
  const ok = await copyText(noteAssetMarkdown(asset))
  if (ok) {
    message.success('已复制 Markdown')
    emit('log', 'info', `已复制资源 Markdown: ${name}`)
  } else {
    message.warning('复制失败，请手动复制')
    emit('log', 'warn', `复制资源 Markdown 失败: ${name}`)
  }
}

// ========== 预览 ==========
const previewOpen = ref(false)
const previewAsset = ref<NoteAsset | null>(null)
const previewBroken = ref(false)

const previewTitle = computed(() =>
  previewAsset.value ? noteAssetDisplayName(previewAsset.value) : '预览资源',
)
const previewUrl = computed(() => previewAsset.value?.url ?? '')

function openPreview(asset: NoteAsset) {
  previewAsset.value = asset
  previewBroken.value = failedThumbs.has(asset.url)
  previewOpen.value = true
}

watch(previewOpen, (open) => {
  if (!open) {
    previewAsset.value = null
    previewBroken.value = false
  }
})

// ========== 打开 / 导出 ==========
const busyUrl = ref<string | null>(null)

async function handleOpen(asset: NoteAsset) {
  if (busyUrl.value) return
  const name = noteAssetFileName(asset)
  const label = noteAssetDisplayName(asset)
  busyUrl.value = asset.url
  try {
    let tempPath: string
    try {
      tempPath = await decryptToTemp(asset.dir, asset.id, name)
    } catch (err) {
      markMissing(asset)
      // 文件真的没了 / 浏览器环境：只降级提示，不抛异常
      emit('log', 'error', `解密打开失败（${label}）: ${errorText(err)}`)
      message.error(`打开失败: ${errorText(err)}`)
      return
    }
    try {
      await openFile(tempPath)
      emit('log', 'info', `已打开资源: ${label}`)
    } catch (err) {
      // 解密成功、失败的是「用系统程序打开」，分开提示
      emit('log', 'error', `用系统程序打开失败（${label}）: ${errorText(err)}`)
      message.error(`打开失败: ${errorText(err)}`)
    }
  } finally {
    busyUrl.value = null
  }
}

async function handleExport(asset: NoteAsset) {
  if (busyUrl.value) return
  const name = noteAssetFileName(asset)
  const label = noteAssetDisplayName(asset)
  let target: string | null = null
  try {
    target = await pickSavePath(name)
  } catch (err) {
    emit('log', 'error', `选择保存位置失败（${label}）: ${errorText(err)}`)
    message.error('无法打开保存对话框')
    return
  }
  if (!target) return
  busyUrl.value = asset.url
  try {
    try {
      await decryptExportFile(asset.dir, asset.id, target)
      message.success('已导出')
      emit('log', 'success', `已导出资源: ${label}`)
    } catch (err) {
      markMissing(asset)
      emit('log', 'error', `导出失败（${label}）: ${errorText(err)}`)
      message.error(`导出失败: ${errorText(err)}`)
    }
  } finally {
    busyUrl.value = null
  }
}

// ========== 重命名 ==========
const renameOpen = ref(false)
const renameTarget = ref<NoteAsset | null>(null)
const renameValue = ref('')
const renameInputRef = ref<{ focus?: () => void; select?: () => void } | null>(null)

function setRenameInputEl(el: unknown) {
  renameInputRef.value = (el as { focus?: () => void; select?: () => void } | null) ?? null
}

function openRename(asset: NoteAsset) {
  renameTarget.value = asset
  renameValue.value = asset.alt.trim() || noteAssetDisplayName(asset)
  renameOpen.value = true
}

// 弹窗打开后聚焦并全选，方便直接改名
watch(renameOpen, async (open) => {
  if (!open) return
  await nextTick()
  renameInputRef.value?.focus?.()
  renameInputRef.value?.select?.()
})

function confirmRename() {
  const asset = renameTarget.value
  if (!asset) return
  const name = renameValue.value.trim()
  if (!name) {
    message.warning('名称不能为空')
    return
  }
  renameOpen.value = false
  if (name === noteAssetDisplayName(asset)) return
  // 只改正文里的 alt，密文文件名（id.wst）保持不变
  emit('updateMarkdown', renameNoteAssetInMarkdown(props.markdown ?? '', asset.url, name))
  emit('log', 'info', `已重命名资源: ${noteAssetDisplayName(asset)} → ${name}`)
}

// ========== 删除 ==========
function confirmDelete(asset: NoteAsset) {
  const label = noteAssetDisplayName(asset)
  Modal.confirm({
    title: '删除资源',
    content: `确定要从笔记中删除「${label}」吗？删除后不可恢复。`,
    okText: '删除',
    okType: 'danger',
    cancelText: '取消',
    async onOk() {
      try {
        await removeAttachmentFile(asset.dir, asset.id)
        emit('log', 'info', `已删除资源文件: ${label}`)
      } catch (err) {
        // 密文文件可能早就丢了 / 浏览器环境：删除失败也要把正文引用摘掉
        emit('log', 'error', `删除资源文件失败（${label}）: ${errorText(err)}`)
        message.warning(`密文文件删除失败，已从笔记移除引用: ${errorText(err)}`)
      }
      statCache.delete(asset.url)
      failedThumbs.delete(asset.url)
      emit('updateMarkdown', removeNoteAssetFromMarkdown(props.markdown ?? '', asset.url))
      emit('log', 'info', `已从笔记移除资源引用: ${label}`)
    },
  })
}

// ========== 上传 ==========
const uploading = ref(false)

/** 拆文件名主体与扩展名：alt 用主体名，扩展名交给后端校验 */
function splitFileName(fileName: string): { stem: string; ext: string } {
  const dot = fileName.lastIndexOf('.')
  if (dot > 0 && dot < fileName.length - 1) {
    return { stem: fileName.slice(0, dot), ext: fileName.slice(dot + 1).toLowerCase() }
  }
  return { stem: fileName, ext: '' }
}

function safeAlt(name: string): string {
  return (name || '').replace(/[\]\n\r]/g, ' ').trim()
}

function markdownForUpload(name: string, id: string, ext: string, dir: string): string {
  return `![${safeAlt(name)}](${noteImageUrl(id, ext, dir)})`
}

/** 解析上传目录；失败返回 null，调用方直接结束 */
async function resolveDirOrWarn(): Promise<string | null> {
  try {
    return await resolveUploadDir(props.defaultUploadDir)
  } catch (err) {
    emit('log', 'error', `解析上传目录失败: ${errorText(err)}`)
    message.error('无法解析上传目录，资源未上传')
    return null
  }
}

async function handlePickUpload() {
  if (uploading.value) return
  if (!hasTauriRuntime()) {
    message.warning('浏览器预览环境不支持上传，请在桌面端使用')
    return
  }
  let paths: string[] = []
  try {
    paths = await pickFiles({ title: '选择要加密上传的资源' })
  } catch (err) {
    emit('log', 'error', `打开文件选择器失败: ${errorText(err)}`)
    message.error('无法打开文件选择器')
    return
  }
  if (!paths.length) return

  uploading.value = true
  try {
    const dir = await resolveDirOrWarn()
    if (!dir) return
    for (const path of paths) {
      const fileName = basename(path)
      try {
        const meta = await encryptUploadFile(path, dir, fileName)
        emit('insert', markdownForUpload(splitFileName(fileName).stem, meta.id, meta.ext, meta.dir))
        emit('log', 'success', `已加密上传资源: ${fileName}`)
      } catch (err) {
        emit('log', 'error', `资源上传失败（${fileName}）: ${errorText(err)}`)
        message.error(`上传失败: ${fileName}`)
      }
    }
  } finally {
    uploading.value = false
  }
}

// ========== 拖拽上传 ==========
// 窗口关了 Tauri 原生拖放，所以走 HTML5 drop：从 dataTransfer 拿 File，
// 读成 base64 交给 encrypt_upload_bytes 逐条加密落盘。
let dragDepth = 0
const dragActive = ref(false)
// 拖拽走 IPC 传字节，大文件会占大量内存；超过这个大小建议用「上传资源」按钮
const DROP_SIZE_LIMIT = 100 * 1024 * 1024

function isFileDrag(event: DragEvent): boolean {
  const transfer = event.dataTransfer
  if (!transfer) return false
  if (Array.from(transfer.types ?? []).includes('Files')) return true
  // 某些 WebKit 版本在 dragover 阶段 types 不全，兜底看 files
  return (transfer.files?.length ?? 0) > 0
}

function onDragEnter(event: DragEvent) {
  if (!isFileDrag(event)) return
  event.preventDefault()
  dragDepth += 1
  dragActive.value = true
}

function onDragOver(event: DragEvent) {
  if (!isFileDrag(event)) return
  // 不 preventDefault 浏览器就不会把 drop 交给我们
  event.preventDefault()
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy'
  dragActive.value = true
}

function onDragLeave(event: DragEvent) {
  if (!isFileDrag(event)) return
  dragDepth = Math.max(0, dragDepth - 1)
  if (dragDepth === 0) dragActive.value = false
}

async function onDrop(event: DragEvent) {
  event.preventDefault()
  dragDepth = 0
  dragActive.value = false
  if (!isFileDrag(event)) return

  const files: File[] = []
  let skippedDir = false
  for (const item of Array.from(event.dataTransfer?.items ?? [])) {
    if (item.kind !== 'file') continue
    // 整目录拖进来暂不支持：没有递归遍历，跳过并提示
    if (item.webkitGetAsEntry?.()?.isDirectory) {
      skippedDir = true
      continue
    }
    const file = item.getAsFile()
    if (file) files.push(file)
  }
  if (!files.length) {
    for (const file of Array.from(event.dataTransfer?.files ?? [])) files.push(file)
  }
  if (skippedDir) message.warning('暂不支持拖入文件夹，请拖入具体文件')
  await uploadDroppedFiles(files)
}

/** File → base64（不含 data URL 前缀），后端解密后按字节加密落盘 */
function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = String(reader.result ?? '')
      const comma = result.indexOf(',')
      resolve(comma >= 0 ? result.slice(comma + 1) : result)
    }
    reader.onerror = () => reject(reader.error ?? new Error('读取文件失败'))
    reader.readAsDataURL(file)
  })
}

async function uploadDroppedFiles(files: File[]) {
  if (!files.length) return
  if (uploading.value) {
    message.warning('正在上传，请稍候')
    return
  }
  if (!hasTauriRuntime()) {
    message.warning('浏览器预览环境不支持上传，请在桌面端使用')
    return
  }
  uploading.value = true
  try {
    const dir = await resolveDirOrWarn()
    if (!dir) return
    for (const file of files) {
      if (file.size > DROP_SIZE_LIMIT) {
        const msg = `「${file.name}」超过 100MB，请用「上传资源」按钮选择大文件`
        message.error(msg)
        emit('log', 'warn', msg)
        continue
      }
      try {
        const dataB64 = await fileToBase64(file)
        const { stem, ext } = splitFileName(file.name)
        const meta = await encryptUploadBytes(dir, file.name, ext, dataB64)
        emit('insert', markdownForUpload(stem, meta.id, meta.ext, meta.dir))
        emit('log', 'success', `已加密上传资源: ${file.name}`)
      } catch (err) {
        emit('log', 'error', `拖入资源上传失败（${file.name}）: ${errorText(err)}`)
        message.error(`上传失败: ${file.name}`)
      }
    }
  } finally {
    uploading.value = false
  }
}

// ========== 大纲 ==========
function outlineIndent(level: number): string {
  return `${8 + Math.max(0, Math.min(level, 6) - 1) * 12}px`
}
</script>

<template>
  <div
    class="note-sidebar"
    :class="{ 'drag-over': dragActive }"
    @dragenter="onDragEnter"
    @dragover="onDragOver"
    @dragleave="onDragLeave"
    @drop="onDrop"
  >
    <!-- 顶部页签 + 收起按钮 -->
    <div class="sidebar-tabs" role="tablist">
      <button
        type="button"
        role="tab"
        class="sidebar-tab"
        :class="{ active: activeTab === 'assets' }"
        :aria-selected="activeTab === 'assets'"
        @click="activeTab = 'assets'"
      >
        <span>附件</span>
        <span class="sidebar-tab-count">{{ assets.length }}</span>
      </button>
      <button
        type="button"
        role="tab"
        class="sidebar-tab"
        :class="{ active: activeTab === 'outline' }"
        :aria-selected="activeTab === 'outline'"
        @click="activeTab = 'outline'"
      >
        <span>大纲</span>
        <span class="sidebar-tab-count">{{ headings.length }}</span>
      </button>
      <span class="sidebar-tab-spacer" />
      <Tooltip title="收起侧栏">
        <Button
          type="text"
          size="small"
          class="sidebar-collapse"
          aria-label="收起侧栏"
          @click="emit('collapse')"
        >
          <DoubleRightOutlined />
        </Button>
      </Tooltip>
    </div>

    <!-- 附件页 -->
    <div v-show="activeTab === 'assets'" class="sidebar-body">
      <div class="assets-toolbar">
        <Button type="primary" size="small" :loading="uploading" @click="handlePickUpload">
          <template #icon><UploadOutlined /></template>
          上传资源
        </Button>
        <span class="assets-toolbar-hint">也可把图片拖入这里</span>
      </div>

      <!-- 空状态：没有资源时给粘贴 / 拖入的说明 -->
      <div v-if="assets.length === 0" class="assets-empty">
        <InboxOutlined class="assets-empty-icon" />
        <p class="assets-empty-title">还没有附件资源</p>
        <p class="assets-empty-text">
          在笔记里粘贴 / 拖入图片、或点上方「上传资源」，资源会出现在这里
        </p>
        <p class="assets-empty-formats">支持 png · jpg · jpeg · gif · webp · bmp · svg</p>
      </div>

      <!-- 资源卡片：缩略图 + 名称/大小 + hover 才出现的操作条 -->
      <div v-else class="assets-list">
        <div
          v-for="asset in assets"
          :key="asset.url"
          class="asset-card"
          :class="{ 'is-missing': isMissing(asset) }"
        >
          <div class="asset-row">
            <div class="asset-thumb">
              <img
                v-if="!failedThumbs.has(asset.url)"
                :src="asset.url"
                :alt="noteAssetDisplayName(asset)"
                loading="lazy"
                @error="onThumbError(asset)"
              />
              <FileImageOutlined v-else class="asset-thumb-icon" />
            </div>
            <div class="asset-main">
              <span class="asset-name" :title="noteAssetDisplayName(asset)">
                {{ noteAssetDisplayName(asset) }}
              </span>
              <span class="asset-meta" :class="{ missing: isMissing(asset) }">
                {{ assetMetaText(asset) }}
              </span>
            </div>
          </div>

          <div class="asset-actions">
            <Tooltip title="插入">
              <Button
                type="text"
                size="small"
                class="asset-action"
                aria-label="插入"
                @click="insertAsset(asset)"
              >
                <PlusOutlined />
              </Button>
            </Tooltip>
            <Tooltip title="预览">
              <Button
                type="text"
                size="small"
                class="asset-action"
                aria-label="预览"
                @click="openPreview(asset)"
              >
                <EyeOutlined />
              </Button>
            </Tooltip>
            <Tooltip title="复制 Markdown">
              <Button
                type="text"
                size="small"
                class="asset-action"
                aria-label="复制 Markdown"
                @click="copyAssetMarkdown(asset)"
              >
                <CopyOutlined />
              </Button>
            </Tooltip>
            <Tooltip title="打开">
              <Button
                type="text"
                size="small"
                class="asset-action"
                aria-label="打开"
                :disabled="busyUrl === asset.url"
                @click="handleOpen(asset)"
              >
                <FolderOpenOutlined />
              </Button>
            </Tooltip>
            <Tooltip title="导出">
              <Button
                type="text"
                size="small"
                class="asset-action"
                aria-label="导出"
                :disabled="busyUrl === asset.url"
                @click="handleExport(asset)"
              >
                <ExportOutlined />
              </Button>
            </Tooltip>
            <Tooltip title="重命名">
              <Button
                type="text"
                size="small"
                class="asset-action"
                aria-label="重命名"
                @click="openRename(asset)"
              >
                <EditOutlined />
              </Button>
            </Tooltip>
            <Tooltip title="删除">
              <Button
                type="text"
                size="small"
                class="asset-action danger"
                aria-label="删除"
                @click="confirmDelete(asset)"
              >
                <DeleteOutlined />
              </Button>
            </Tooltip>
          </div>
        </div>
      </div>
    </div>

    <!-- 大纲页 -->
    <div v-show="activeTab === 'outline'" class="sidebar-body">
      <div v-if="headings.length === 0" class="outline-empty">
        <Empty :image="Empty.PRESENTED_IMAGE_SIMPLE" description="笔记里还没有标题" />
        <p class="outline-empty-hint">用「# 标题」给笔记分节，就能在这里快速跳转</p>
      </div>
      <div v-else class="outline-list">
        <button
          v-for="heading in headings"
          :key="`${heading.index}-${heading.line}`"
          type="button"
          class="outline-item"
          :class="`outline-level-${Math.min(heading.level, 3)}`"
          :style="{ paddingLeft: outlineIndent(heading.level) }"
          :title="heading.text"
          @click="emit('scrollToHeading', heading.index)"
        >
          <span class="outline-level">H{{ heading.level }}</span>
          <span class="outline-text">{{ heading.text }}</span>
        </button>
      </div>
    </div>

    <!-- 预览大图：点遮罩 / 右上角都能关 -->
    <Modal
      v-model:open="previewOpen"
      :title="previewTitle"
      :footer="null"
      centered
      :width="720"
      :mask-closable="true"
      class="asset-preview-modal"
    >
      <div class="preview-body">
        <img
          v-if="!previewBroken"
          :src="previewUrl"
          :alt="previewTitle"
          class="preview-image"
          @error="previewBroken = true"
        />
        <div v-else class="preview-placeholder">
          <FileImageOutlined class="preview-placeholder-icon" />
          <span>资源暂时无法预览（密文文件可能已丢失）</span>
        </div>
      </div>
    </Modal>

    <!-- 重命名：只改正文 alt，不动磁盘上的密文文件 -->
    <Modal
      v-model:open="renameOpen"
      title="重命名资源"
      ok-text="保存"
      cancel-text="取消"
      :width="360"
      @ok="confirmRename"
    >
      <Input
        :ref="setRenameInputEl"
        v-model:value="renameValue"
        :maxlength="80"
        placeholder="资源名称（写入图片 alt）"
        @press-enter="confirmRename"
      />
      <p class="rename-hint">只修改正文里的 alt 文本，不会重命名磁盘上的密文文件。</p>
    </Modal>
  </div>
</template>

<style scoped>
.note-sidebar {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  position: relative;
  overflow: hidden;
  background: var(--bg-primary);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  transition: background 0.15s, outline-color 0.15s;
}
/* 拖文件进来：整块侧栏都是落点，横向一圈虚线提示 */
.note-sidebar.drag-over {
  outline: 1px dashed var(--primary);
  outline-offset: -3px;
  background: rgba(59, 130, 246, 0.04);
}

/* ===== 顶部页签 ===== */
.sidebar-tabs {
  display: flex;
  align-items: center;
  gap: 2px;
  height: var(--panel-header-height);
  padding: 0 4px 0 8px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}
.sidebar-tab {
  appearance: none;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 8px;
  border: none;
  border-radius: 6px;
  background: transparent;
  font-family: inherit;
  font-size: 13px;
  line-height: 1;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.sidebar-tab:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.sidebar-tab.active {
  color: var(--primary);
  background: var(--bg-active);
  font-weight: 600;
}
.sidebar-tab-count {
  min-width: 16px;
  padding: 0 5px;
  border-radius: 8px;
  font-size: 11px;
  font-weight: 400;
  line-height: 16px;
  text-align: center;
  color: var(--text-tag);
  background: var(--bg-tag);
}
.sidebar-tab.active .sidebar-tab-count {
  color: var(--primary);
  background: rgba(59, 130, 246, 0.14);
}
.sidebar-tab-spacer {
  flex: 1;
}
.sidebar-collapse {
  color: var(--text-muted);
  flex-shrink: 0;
}
.sidebar-collapse:hover {
  color: var(--primary);
}

/* ===== 内容区（列表内部滚动） ===== */
.sidebar-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* ===== 附件工具条 ===== */
.assets-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 10px 8px;
  flex-shrink: 0;
}
.assets-toolbar-hint {
  font-size: 11px;
  line-height: 1.4;
  color: var(--text-muted);
}

/* ===== 附件空状态 ===== */
.assets-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 16px 16px 24px;
  text-align: center;
}
.assets-empty-icon {
  font-size: 26px;
  color: var(--text-muted);
}
.assets-empty-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-secondary);
}
.assets-empty-text {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
.assets-empty-formats {
  margin-top: 2px;
  padding: 2px 8px;
  border-radius: 10px;
  font-size: 11px;
  color: var(--text-muted);
  background: var(--bg-hover);
}

/* ===== 资源卡片 ===== */
.assets-list {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 0 10px 10px;
}
.asset-card {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  transition: border-color 0.15s, background 0.15s;
}
.asset-card:hover,
.asset-card:focus-within {
  border-color: #bfdbfe;
  background: var(--bg-secondary);
}
.asset-card.is-missing {
  border-style: dashed;
}
.asset-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.asset-thumb {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 38px;
  height: 38px;
  flex-shrink: 0;
  overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-hover);
}
.asset-thumb img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.asset-thumb-icon {
  font-size: 18px;
  color: var(--text-muted);
}
.asset-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}
.asset-name {
  font-size: 13px;
  line-height: 1.4;
  color: var(--text-primary);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.asset-meta {
  font-size: 11px;
  line-height: 1.4;
  color: var(--text-muted);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.asset-meta.missing {
  color: var(--warning);
}

/* 操作条：hover / 键盘聚焦才显示，但不 display:none，触屏与键盘仍可点 */
.asset-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  opacity: 0;
  transition: opacity 0.12s ease;
}
.asset-card:hover .asset-actions,
.asset-card:focus-within .asset-actions {
  opacity: 1;
}
@media (hover: none) {
  .asset-actions {
    opacity: 1;
  }
}
.asset-action {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border-radius: 6px;
  color: var(--text-secondary);
  font-size: 13px;
}
.asset-action:hover {
  color: var(--primary);
  background: var(--bg-active);
}
.asset-action.danger:hover {
  color: var(--danger);
  background: rgba(239, 68, 68, 0.08);
}

/* ===== 大纲 ===== */
.outline-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 16px;
  text-align: center;
}
.outline-empty :deep(.ant-empty-description) {
  color: var(--text-muted);
  font-size: 13px;
}
.outline-empty-hint {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
.outline-list {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 8px;
}
.outline-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  min-width: 0;
  padding: 5px 8px;
  border: none;
  border-radius: 6px;
  background: transparent;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.outline-item:hover {
  background: var(--bg-hover);
}
.outline-level {
  flex-shrink: 0;
  font-size: 10px;
  line-height: 1;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}
.outline-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 13px;
  line-height: 1.4;
  color: var(--text-secondary);
}
.outline-item:hover .outline-text {
  color: var(--text-primary);
}
.outline-level-1 .outline-text {
  font-weight: 600;
  color: var(--text-primary);
}
.outline-level-2 .outline-text {
  font-weight: 500;
}

/* ===== 预览 / 重命名弹窗 ===== */
.preview-body {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 220px;
  max-height: 68vh;
  overflow: auto;
  border-radius: 6px;
  background: var(--bg-secondary);
}
.preview-image {
  display: block;
  max-width: 100%;
  max-height: 66vh;
  object-fit: contain;
}
.preview-placeholder {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 40px 24px;
  font-size: 13px;
  color: var(--text-muted);
}
.preview-placeholder-icon {
  font-size: 34px;
  color: var(--text-muted);
}
.rename-hint {
  margin-top: 8px;
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-muted);
}
</style>
