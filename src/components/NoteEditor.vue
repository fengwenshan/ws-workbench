<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import Vditor from 'vditor'
import 'vditor/dist/index.css'
import type { LogLevel } from '@/types'
import { encryptUploadBytes, resolveUploadDir } from '@/api/attachments'
import { noteImageUrl } from '@/utils/note-assets'
import { parseNoteHeadings, countNoteWords } from '@/utils/note-text'
import { errorText } from '@/utils/error'

const props = defineProps<{
  note: string
  /** 设置里配置的全局上传目录（null 表示 App 私有目录） */
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  update: [note: string]
  /** 每次输入立即回传（不落盘），供父级保持侧栏解析用的正文最新 */
  change: [note: string]
  log: [level: LogLevel, message: string]
}>()

/**
 * 单栏即时渲染（IR）编辑器，基于 Vditor。
 *
 * 数据流：
 * - 编辑器 input → 500ms 防抖 emit update（沿用旧编辑器的保存节奏）
 * - 外部（切换实体 / 侧栏重命名或删除资源）改正文 → watch 静默 setValue，不回传
 * - 图片：选图 / 粘贴 / 拖入都走 upload.handler，用原来的加密上传链路落盘，
 *   拿到 noteImageUrl 后自己插入正文（Vditor 只要配置了 handler 就不会自动插入）
 */

const hostRef = ref<HTMLDivElement | null>(null)
const vditor = shallowRef<Vditor | null>(null)

/** 当前编辑器里的 markdown，仅用于字数与保存状态展示 */
const content = ref(props.note ?? '')
const editing = ref(false)
const wordCount = computed(() => countNoteWords(content.value))

/** 编辑器内部最后一次同步过的正文，用来识别「是不是自己刚写出去的」 */
let lastKnown = props.note ?? ''
let ready = false
let destroyed = false
let emitTimer: number | undefined
/** 编辑器就绪前收到的插入请求 */
let pendingInsert: string | null = null

const TOOLBAR = [
  'headings',
  'bold',
  'italic',
  'strike',
  'link',
  '|',
  'list',
  'ordered-list',
  'check',
  'quote',
  'line',
  'code',
  'inline-code',
  '|',
  'upload',
  'table',
  '|',
  'undo',
  'redo',
  '|',
  'edit-mode',
  'preview',
]

// ========== 编辑器生命周期 ==========
onMounted(() => {
  const host = hostRef.value
  if (!host) return
  vditor.value = new Vditor(host, {
    mode: 'ir',
    height: '100%',
    minHeight: 200,
    value: lastKnown,
    cdn: '/vditor',
    theme: 'classic',
    lang: 'zh_CN',
    icon: 'material',
    cache: { enable: false },
    outline: { enable: false, position: 'left' },
    counter: { enable: false },
    placeholder: '记录一下…',
    toolbar: TOOLBAR,
    toolbarConfig: { pin: true },
    upload: {
      accept: 'image/*',
      multiple: true,
      handler: handleUpload,
    },
    input(value: string) {
      onInput(value)
    },
    after() {
      onReady()
    },
  })
})

function onReady() {
  ready = true
  const editor = vditor.value
  if (!editor) return
  // 初始化期间外部又换了实体：把最新值补进去
  if (editor.getValue() !== lastKnown) editor.setValue(lastKnown, true)
  content.value = lastKnown
  if (pendingInsert) {
    const md = pendingInsert
    pendingInsert = null
    insertMarkdown(md)
  }
}

onBeforeUnmount(() => {
  destroyed = true
  if (emitTimer) {
    window.clearTimeout(emitTimer)
    emitTimer = undefined
  }
  // 卸载前把未落盘的内容补交，避免快速切换实体丢内容。
  // Vditor 的 input 回调要等 undoDelay，这里直接取编辑器当前值兜底。
  const latest = currentMarkdown()
  if (latest !== (props.note ?? '')) emit('update', latest)
  const editor = vditor.value
  vditor.value = null
  ready = false
  if (editor) {
    try {
      editor.destroy()
    } catch {
      // 编辑器初始化中途卸载时 destroy 可能抛错，忽略即可
    }
  }
})

/** 编辑器当前正文；未就绪时退回最后同步过的值 */
function currentMarkdown(): string {
  const editor = vditor.value
  if (!editor || !ready) return lastKnown
  try {
    return editor.getValue()
  } catch {
    return lastKnown
  }
}

// ========== 编辑 / 保存状态 ==========
function scheduleEmit(value: string) {
  if (emitTimer) window.clearTimeout(emitTimer)
  emitTimer = window.setTimeout(() => {
    emitTimer = undefined
    editing.value = false
    emit('update', value)
  }, 500)
}

/** 把编辑器里的新内容同步到本地状态，并安排防抖回传 */
function applyLocalValue(value: string) {
  if (value === lastKnown) return
  content.value = value
  lastKnown = value
  editing.value = true
  // 立即通知父级：侧栏的附件/大纲解析要用最新正文（落盘仍走 500ms 防抖）
  emit('change', value)
  scheduleEmit(value)
}

function onInput(value: string) {
  applyLocalValue(value)
}

/**
 * Vditor 的 options.input 要等 undoDelay（默认 800ms）才回调，
 * 直接监听编辑区 DOM input，让字数 / 保存状态和防抖保存都从按键当下开始算。
 * 事件先经 Vditor 自己的监听处理完再冒泡到这里，拿到的已是规范化后的 markdown。
 */
function onEditorDomInput(event: Event) {
  const host = hostRef.value
  const editor = vditor.value
  if (!host || !editor || !ready) return
  const root = contentRoot(host, editor)
  const target = event.target
  if (!root || !(target instanceof Node) || !root.contains(target)) return
  applyLocalValue(editor.getValue())
}

// 外部（切换实体 / 侧栏改正文）传入新笔记时同步到编辑器
watch(
  () => props.note,
  (value) => {
    const next = value ?? ''
    if (next === lastKnown) return
    lastKnown = next
    content.value = next
    editing.value = false
    if (emitTimer) {
      window.clearTimeout(emitTimer)
      emitTimer = undefined
    }
    const editor = vditor.value
    if (ready && editor && editor.getValue() !== next) editor.setValue(next, true)
  },
)

// ========== 对外方法 ==========
function insertMarkdown(md: string) {
  if (!md) return
  const editor = vditor.value
  if (!editor || !ready) {
    pendingInsert = md
    return
  }
  editor.focus()
  // Vditor 会在焦点处插入内容并完成 IR 渲染
  editor.insertValue(md, true)
}

/** 静默回写正文：不触发 update，也不把这次变化当成本地编辑 */
function setMarkdown(md: string) {
  const next = md ?? ''
  lastKnown = next
  content.value = next
  editing.value = false
  if (emitTimer) {
    window.clearTimeout(emitTimer)
    emitTimer = undefined
  }
  if (!ready) return
  const editor = vditor.value
  if (editor && editor.getValue() !== next) editor.setValue(next, true)
}

function focus() {
  vditor.value?.focus()
}

/** 当前编辑模式下的内容容器。注意：wysiwyg 容器在 DOM 里排在 ir 前面且隐藏，不能混着选 */
function contentRoot(host: HTMLElement, editor: Vditor): HTMLElement | null {
  const mode = editor.getCurrentMode()
  if (mode === 'wysiwyg') return host.querySelector<HTMLElement>('.vditor-wysiwyg')
  if (mode === 'sv') return host.querySelector<HTMLElement>('textarea.vditor-sv')
  return host.querySelector<HTMLElement>('.vditor-ir')
}

/** 滚到第 index 个标题（index 来自 note-text.ts 的 NoteHeading.index） */
function scrollToHeading(index: number) {
  const host = hostRef.value
  const editor = vditor.value
  if (!host || !editor || !ready || index < 0) return

  if (editor.getCurrentMode() !== 'sv') {
    const root = contentRoot(host, editor)
    if (!root) return
    const headings = root.querySelectorAll<HTMLElement>('h1, h2, h3, h4, h5, h6')
    const target = headings[index]
    if (!target) return
    const scroller = root.querySelector<HTMLElement>('pre.vditor-reset') ?? root
    const top =
      target.getBoundingClientRect().top -
      scroller.getBoundingClientRect().top +
      scroller.scrollTop -
      8
    scroller.scrollTo({ top: Math.max(0, top), behavior: 'smooth' })
    return
  }

  // 源码模式没有渲染后的标题 DOM，按行号近似定位
  const textarea = host.querySelector<HTMLTextAreaElement>('textarea.vditor-sv')
  if (!textarea) return
  const value = editor.getValue()
  const line = parseNoteHeadings(value)[index]?.line
  if (line == null) return
  const lines = value.split('\n')
  let position = 0
  for (let i = 0; i < Math.min(line, lines.length); i++) position += lines[i].length + 1
  textarea.focus()
  textarea.setSelectionRange(position, Math.min(position + (lines[line]?.length ?? 0), textarea.value.length))
  textarea.scrollTop = Math.max(0, (line / Math.max(1, lines.length)) * textarea.scrollHeight - textarea.clientHeight / 3)
}

defineExpose({ insertMarkdown, setMarkdown, focus, scrollToHeading })

// ========== 图片上传（加密链路） ==========
function tip(text: string, time = 3000) {
  vditor.value?.tip(text, time)
}

function isImageFile(file: File): boolean {
  if (file.type && file.type.startsWith('image/')) return true
  // 部分 WebView 拿不到 type，用扩展名兜底
  return /\.(png|jpe?g|gif|webp|bmp|svg|avif|ico)$/i.test(file.name || '')
}

function extOf(file: File): string {
  const name = file.name || ''
  const idx = name.lastIndexOf('.')
  if (idx >= 0 && idx < name.length - 1) return name.slice(idx + 1).toLowerCase()
  const type = file.type || ''
  const slash = type.indexOf('/')
  return slash >= 0 ? type.slice(slash + 1).toLowerCase() : ''
}

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = reader.result
      if (typeof result !== 'string') {
        reject(new Error('读取文件失败'))
        return
      }
      const idx = result.indexOf(',')
      resolve(idx >= 0 ? result.slice(idx + 1) : result)
    }
    reader.onerror = () => reject(reader.error ?? new Error('读取文件失败'))
    reader.readAsDataURL(file)
  })
}

function pastedImageName(index: number, total: number, ext: string): string {
  const now = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  const stamp =
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  const suffix = total > 1 ? `-${index + 1}` : ''
  return `粘贴图片-${stamp}${suffix}.${ext || 'png'}`
}

/** 剪贴板 / 文件选择里的 File 统一命名：有名字用原名，没名字按粘贴时间生成 */
function uploadFileName(file: File, index: number, total: number): string {
  const name = (file.name || '').trim()
  return name || pastedImageName(index, total, extOf(file))
}

/** 记录弹上传选择框前光标所在的 range，异步上传完成后尽量插回原位置 */
function captureEditorRange(): Range | null {
  const host = hostRef.value
  const editor = vditor.value
  if (!host || !editor) return null
  const root = contentRoot(host, editor)
  // 源码模式是 textarea，选区不在 document selection 里，交给 Vditor 自己处理
  if (!root || root instanceof HTMLTextAreaElement) return null
  const selection = window.getSelection()
  if (!selection || selection.rangeCount === 0) return null
  const range = selection.getRangeAt(0)
  if (!root.contains(range.startContainer)) return null
  return range.cloneRange()
}

function restoreEditorRange(range: Range | null) {
  if (!range) return
  try {
    const selection = window.getSelection()
    if (!selection) return
    selection.removeAllRanges()
    selection.addRange(range)
  } catch {
    // range 已失效，交给 Vditor 自己保存的光标位置
  }
}

/** 加密上传一批图片，返回插入正文用的资源地址 */
async function uploadImages(files: File[]): Promise<string[]> {
  const images = files.filter(isImageFile)
  if (images.length === 0) {
    emit('log', 'warn', '仅支持插入图片')
    tip('仅支持插入图片')
    return []
  }

  let uploadDir: string
  try {
    uploadDir = await resolveUploadDir(props.defaultUploadDir)
  } catch (e) {
    emit('log', 'error', `解析上传目录失败: ${errorText(e)}`)
    tip(`解析上传目录失败: ${errorText(e)}`)
    return []
  }

  const urls: string[] = []
  let failed = 0
  for (const [index, file] of images.entries()) {
    try {
      const dataB64 = await readFileAsBase64(file)
      if (!dataB64) throw new Error('文件内容为空')
      const meta = await encryptUploadBytes(
        uploadDir,
        uploadFileName(file, index, images.length),
        extOf(file),
        dataB64,
      )
      // 笔记里的图片只在笔记里用，不进「上传」页的附件列表
      emit('log', 'success', `笔记图片已加密保存: ${meta.name}`)
      urls.push(noteImageUrl(meta.id, meta.ext, meta.dir))
    } catch (e) {
      failed += 1
      emit(
        'log',
        'error',
        `图片上传失败: ${file.name || '剪贴板图片'} - ${errorText(e)}`,
      )
    }
  }

  if (failed > 0) tip(`有 ${failed} 张图片上传失败，详见日志`)
  return urls
}

function insertImageUrls(urls: string[], range: Range | null) {
  const editor = vditor.value
  if (!editor || urls.length === 0) return
  restoreEditorRange(range)
  editor.insertValue(
    urls.map((url) => `![](${url})`).join('\n'),
    true,
  )
  editor.focus()
}

/** Vditor upload.handler：选图 / 粘贴 / 拖入最终都到这里 */
async function handleUpload(files: File[]): Promise<null> {
  const range = captureEditorRange()
  try {
    const urls = await uploadImages(files)
    if (!destroyed) insertImageUrls(urls, range)
  } catch (e) {
    emit('log', 'error', `图片上传失败: ${errorText(e)}`)
    tip(`图片上传失败: ${errorText(e)}`)
  }
  // 配置了 handler 后 Vditor 不会自动插入，返回 null 由我们自己插入
  return null
}

// ========== 粘贴兜底 ==========
// Vditor 自带的粘贴只认 clipboardData.files，个别 WebView 里图片只出现在
// clipboardData.items 中，这里补一条兜底；有 files 时仍交给 Vditor 原生流程。
function onPasteCapture(event: ClipboardEvent) {
  const data = event.clipboardData
  if (!data) return
  if (Array.from(data.files ?? []).some(isImageFile)) return

  const files: File[] = []
  for (const item of Array.from(data.items ?? [])) {
    if (item.kind !== 'file' || !item.type.startsWith('image/')) continue
    const file = item.getAsFile()
    if (file) files.push(file)
  }
  if (files.length === 0) return

  event.preventDefault()
  event.stopPropagation()
  void handleUpload(files)
}
</script>

<template>
  <div class="note-editor" @paste.capture="onPasteCapture" @input="onEditorDomInput">
    <div ref="hostRef" class="note-editor-vditor"></div>
    <div class="note-editor-statusbar">
      <span class="note-editor-words">{{ wordCount }} 字</span>
      <span class="note-editor-state" :class="{ 'is-editing': editing }">
        {{ editing ? '编辑中…' : '已保存' }}
      </span>
    </div>
  </div>
</template>

<style scoped>
/* 编辑器外壳：边框 / 圆角 / 背景都由这里负责，Vditor 本体只做内部布局 */
.note-editor {
  --note-border-color: var(--border-color);
  --note-bg: var(--bg-primary);
  --note-bg-soft: var(--bg-secondary);
  --note-text: var(--text-primary);
  --note-text-secondary: var(--text-secondary);
  --note-text-muted: var(--text-muted);
  --note-primary: var(--primary);

  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--note-border-color);
  border-radius: 8px;
  background: var(--note-bg);
}

.note-editor-vditor {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

/* 底部状态栏：字数 / 保存状态，保持低调 */
.note-editor-statusbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-shrink: 0;
  height: 26px;
  padding: 0 10px;
  border-top: 1px solid var(--note-border-color);
  background: var(--note-bg);
  font-size: 12px;
  line-height: 1;
  color: var(--note-text-muted);
  user-select: none;
}

.note-editor-state {
  transition: color 0.15s;
}
.note-editor-state.is-editing {
  color: var(--note-primary);
}

/* ========== Vditor 外观 ========== */
.note-editor :deep(.vditor) {
  height: 100%;
  border: 0;
  border-radius: 0;
  background: var(--note-bg);
  font-family: inherit;

  /* Vditor 有自己的同名变量，这里用外壳变量重新映射一遍 */
  --border-color: var(--note-border-color);
  --second-color: var(--note-text-muted);
  --panel-background-color: var(--note-bg);
  --panel-shadow: 0 1px 2px rgba(15, 23, 42, 0.12);
  --toolbar-background-color: var(--note-bg);
  --toolbar-icon-color: var(--note-text-secondary);
  --toolbar-icon-hover-color: var(--note-primary);
  --textarea-background-color: var(--note-bg);
  --textarea-text-color: var(--note-text);
  --resize-icon-color: var(--note-border-color);
  --resize-background-color: var(--note-bg);
  --resize-hover-icon-color: var(--note-text-secondary);
  --resize-hover-background-color: var(--note-bg-soft);
  --count-background-color: var(--note-bg-soft);
  --heading-border-color: var(--note-border-color);
  --blockquote-color: var(--note-text-secondary);
  --blockquote-border-color: var(--note-border-color);
  --blockquote-text-color: var(--note-text-secondary);
  /* IR 模式下光标进入时会显示语法标记，统一成低调的灰色，避免紫红 / 亮蓝 */
  --ir-heading-color: var(--note-text-muted);
  --ir-title-color: var(--note-text-muted);
  --ir-bi-color: var(--note-text-muted);
  --ir-link-color: var(--note-primary);
  --ir-bracket-color: var(--note-text-muted);
  --ir-paren-color: var(--note-text-muted);
}

.note-editor :deep(.vditor-toolbar) {
  padding: 0 8px;
  background: var(--note-bg);
  border-bottom: 1px solid var(--note-border-color);
}

.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped) {
  color: var(--note-text-secondary);
}

.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped:hover),
.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped:focus) {
  color: var(--note-primary);
  background: var(--note-bg-soft);
  border-radius: 4px;
}

.note-editor :deep(.vditor-content) {
  background: var(--note-bg);
}

.note-editor :deep(.vditor-ir pre.vditor-reset),
.note-editor :deep(.vditor-wysiwyg pre.vditor-reset),
.note-editor :deep(.vditor-sv),
.note-editor :deep(.vditor-preview) {
  font-family: inherit;
  color: var(--note-text);
  background: var(--note-bg);
}

.note-editor :deep(.vditor-reset) {
  color: var(--note-text);
  --blockquote-border-color: var(--note-border-color);
  --blockquote-text-color: var(--note-text-secondary);
}

.note-editor :deep(.vditor-reset h1),
.note-editor :deep(.vditor-reset h2) {
  border-bottom-color: var(--note-border-color);
}

.note-editor :deep(.vditor-reset blockquote:not(.callout)) {
  color: var(--note-text-secondary);
}
.note-editor :deep(.vditor-reset blockquote:not(.callout)::before) {
  background-color: var(--note-border-color);
}

.note-editor :deep(.vditor-reset code:not(.hljs):not(.highlight-chroma)) {
  color: var(--note-text);
  background: var(--note-bg-soft);
}

.note-editor :deep(.vditor-reset pre > code) {
  padding: 10px 12px;
  background: var(--note-bg-soft);
  background-image: none;
  border: 1px solid var(--note-border-color);
  border-radius: 6px;
}

.note-editor :deep(.vditor-reset table tr) {
  background-color: var(--note-bg-soft);
  border-top-color: var(--note-border-color);
}
.note-editor :deep(.vditor-reset table td),
.note-editor :deep(.vditor-reset table th) {
  border-color: var(--note-border-color);
}
.note-editor :deep(.vditor-reset table tbody tr:nth-child(2n)) {
  background-color: var(--note-bg);
}
</style>
