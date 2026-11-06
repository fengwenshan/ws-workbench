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

/**
 * 工具栏最后一项是「源码 / 即时渲染」切换：
 * - 保留 Vditor 自带的 edit-mode 项（但隐藏它），借它内部的三模式切换逻辑；
 * - 自定义一项，图标仍是铅笔，点击后直接进 / 出源码模式，不再弹出模式菜单。
 */
function toggleSourceMode(_event: Event, inner: IVditor) {
  const targetMode = inner.currentMode === 'sv' ? 'ir' : 'sv'
  const builtin = inner.toolbar.elements['edit-mode']
  const target = builtin?.querySelector<HTMLButtonElement>(`button[data-mode="${targetMode}"]`)
  if (!target) return
  target.click()

  // 进源码模式时从文档开头看起，避免停在滚动容器底部
  if (targetMode === 'sv') {
    const svEl = inner.sv?.element
    if (svEl) {
      svEl.setSelectionRange(0, 0)
      svEl.scrollTop = 0
    }
  }

  const item = inner.toolbar.elements['source-mode']
  item?.classList.toggle('is-source', inner.currentMode === 'sv')
  item?.querySelector('button')?.setAttribute(
    'aria-label',
    inner.currentMode === 'sv' ? '返回即时渲染' : '查看源代码',
  )
  syncTableOps()
}

const TOOLBAR: Array<string | IMenuItem> = [
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
  // 隐藏的内建项，只作为模式切换逻辑的入口
  'edit-mode',
  {
    name: 'source-mode',
    tip: '查看源代码',
    tipPosition: 'nw',
    icon: '<svg><use xlink:href="#vditor-icon-edit"></use></svg>',
    click: toggleSourceMode,
  },
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
    icon: 'ant',
    cache: { enable: false },
    outline: { enable: false, position: 'left' },
    counter: { enable: false },
    // 源码模式下只显示可编辑的 Markdown，不再出右侧预览
    preview: { mode: 'editor' },
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
  document.addEventListener('selectionchange', syncTableOps)
  document.addEventListener('scroll', syncTableOps, true)
  window.addEventListener('resize', syncTableOps)
  host.addEventListener('mousedown', rememberCodeClick, true)
  host.addEventListener('click', restoreCodeCaret, false)
})

function onReady() {
  ready = true
  const editor = vditor.value
  if (!editor) return
  // 隐藏 Vditor 自带的「切换编辑模式」按钮，只保留工具栏末尾那支铅笔
  const builtinEditMode = editor.vditor?.toolbar?.elements?.['edit-mode']
  if (builtinEditMode) builtinEditMode.style.display = 'none'
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
  document.removeEventListener('selectionchange', syncTableOps)
  document.removeEventListener('scroll', syncTableOps, true)
  window.removeEventListener('resize', syncTableOps)
  hostRef.value?.removeEventListener('mousedown', rememberCodeClick, true)
  hostRef.value?.removeEventListener('click', restoreCodeCaret, false)
  pendingCodeClick = null
  tableOps.value = null
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
  syncTableOps()
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

// ========== 代码块：点击定位到鼠标位置 ==========
// Vditor 的 IR 点击处理里有一条「点击后光标落于预览区」的逻辑：只要点击命中
// .vditor-ir__preview，就把光标挪到代码块开头。代码块的预览层和可编辑层是叠在
// 一起的，个别 WebView 里点击会命中预览层，导致点哪都跳回第一行。这里在事件冒泡
// 结束后按点击坐标重新定位光标，把位置映射回可编辑的原始代码。
let pendingCodeClick: { block: HTMLElement; x: number; y: number } | null = null

function rememberCodeClick(event: MouseEvent) {
  pendingCodeClick = null
  if (event.button !== 0) return
  const host = hostRef.value
  const editor = vditor.value
  if (!host || !editor || !ready || editor.getCurrentMode() !== 'ir') return
  const target = event.target
  if (!(target instanceof Node) || !host.contains(target)) return
  const root = host.querySelector<HTMLElement>('.vditor-ir')
  if (!root || !root.contains(target)) return
  const element = target instanceof Element ? target : target.parentElement
  const block = element?.closest<HTMLElement>('[data-type="code-block"]')
  if (!block) return
  pendingCodeClick = { block, x: event.clientX, y: event.clientY }
}

function caretRangeAtPoint(x: number, y: number): Range | null {
  const doc = document as Document & {
    caretRangeFromPoint?: (x: number, y: number) => Range | null
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null
  }
  if (doc.caretRangeFromPoint) return doc.caretRangeFromPoint(x, y)
  if (doc.caretPositionFromPoint) {
    const position = doc.caretPositionFromPoint(x, y)
    if (!position) return null
    const range = document.createRange()
    range.setStart(position.offsetNode, position.offset)
    range.collapse(true)
    return range
  }
  return null
}

/** 节点内某位置之前的字符数，用来把预览层的偏移换算到原始代码 */
function charOffsetWithin(root: HTMLElement, node: Node, offset: number): number {
  if (node !== root && !root.contains(node)) return -1
  try {
    const range = document.createRange()
    range.setStart(root, 0)
    range.setEnd(node, offset)
    return range.toString().length
  } catch {
    return -1
  }
}

function textNodeAtOffset(root: HTMLElement, offset: number): { node: Text; offset: number } | null {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  let remaining = offset
  let node = walker.nextNode()
  while (node) {
    const length = node.textContent?.length ?? 0
    if (remaining <= length) return { node: node as Text, offset: remaining }
    remaining -= length
    node = walker.nextNode()
  }
  return null
}

function restoreCodeCaret() {
  const pending = pendingCodeClick
  pendingCodeClick = null
  if (!pending) return
  const editor = vditor.value
  if (!editor || !ready || editor.getCurrentMode() !== 'ir') return
  // 拖选代码时不抢选区
  const selection = window.getSelection()
  if (selection && !selection.isCollapsed) return
  const { block, x, y } = pending
  if (!block.isConnected) return
  // Vditor 的点击处理已经把代码块展开，可编辑层此时才可见
  const rawPre = block.querySelector<HTMLElement>('pre.vditor-ir__marker--pre')
  const rawCode = rawPre?.querySelector<HTMLElement>('code') ?? rawPre
  const previewCode = block.querySelector<HTMLElement>('pre.vditor-ir__preview code')
  if (!rawPre || !rawCode || !previewCode || getComputedStyle(rawPre).display === 'none') return

  const pointRange = caretRangeAtPoint(x, y)
  if (!pointRange) return
  const pointNode = pointRange.startContainer

  let offset = -1
  if (rawPre.contains(pointNode)) {
    // 可编辑层在光标下面（Chromium），浏览器自己已经算好了位置
    offset = charOffsetWithin(rawCode, pointNode, pointRange.startOffset)
  } else {
    // 命中渲染层（部分 WebView）：按同一段文本映射回原始代码
    offset = charOffsetWithin(previewCode, pointNode, pointRange.startOffset)
  }
  const rawText = rawCode.textContent ?? ''
  if (offset < 0 || offset > rawText.length) return
  const target = textNodeAtOffset(rawCode, offset)
  if (!target) return
  const range = document.createRange()
  range.setStart(target.node, Math.min(target.offset, target.node.textContent?.length ?? 0))
  range.collapse(true)
  selection?.removeAllRanges()
  selection?.addRange(range)
}

// ========== 表格：光标在表格里时提供加行 / 加列 ==========
const tableOps = ref<{ left: number; top: number; transform: string } | null>(null)

function editableRoot(): { host: HTMLElement; root: HTMLElement } | null {
  const host = hostRef.value
  const editor = vditor.value
  if (!host || !editor || !ready) return null
  if (editor.getCurrentMode() !== 'ir') return null
  const root = contentRoot(host, editor)
  if (!root) return null
  return { host, root }
}

function selectedTableCell(root: HTMLElement): HTMLTableCellElement | null {
  const selection = window.getSelection()
  if (!selection || selection.rangeCount === 0) return null
  const node = selection.getRangeAt(0).startContainer
  const element = node instanceof Element ? node : node.parentElement
  const cell = element?.closest<HTMLTableCellElement>('td, th')
  if (!cell || !root.contains(cell) || !cell.closest('table')) return null
  return cell
}

/** 根据当前光标所在的表格刷新浮动工具条的位置；不在表格里就隐藏 */
function syncTableOps() {
  const ctx = editableRoot()
  const cell = ctx ? selectedTableCell(ctx.root) : null
  if (!ctx || !cell) {
    tableOps.value = null
    return
  }
  const table = cell.closest('table') as HTMLTableElement
  const hostRect = ctx.host.getBoundingClientRect()
  const rect = table.getBoundingClientRect()
  // table 元素可能被撑满整列，实际网格只占左侧一部分，用行内最后一个单元格对齐
  const row = cell.parentElement as HTMLTableRowElement | null
  const lastCell = row?.cells[row.cells.length - 1]
  const visualRight = lastCell?.getBoundingClientRect().right ?? rect.right
  const rowRect = row?.getBoundingClientRect() ?? rect

  // 优先贴在表格右侧空白处，与当前行垂直居中，不遮挡表格和上方正文
  const panelWidth = 176
  const sideLeft = visualRight - hostRect.left + 10
  if (sideLeft + panelWidth <= hostRect.width - 8) {
    tableOps.value = {
      left: sideLeft,
      top: rowRect.top + rowRect.height / 2 - hostRect.top,
      transform: 'translate(0, -50%)',
    }
    return
  }

  const left = Math.min(visualRight - hostRect.left - 2, hostRect.width - 8)
  const above = rect.top - hostRect.top - 6
  // 表格太宽没地方贴，就放表格上方；贴近顶部时改放下方
  tableOps.value =
    above >= 92
      ? { left, top: above, transform: 'translate(-100%, -100%)' }
      : { left, top: rect.bottom - hostRect.top + 6, transform: 'translate(-100%, 0)' }
}

function activeTableContext(): {
  root: HTMLElement
  cell: HTMLTableCellElement
  table: HTMLTableElement
} | null {
  const ctx = editableRoot()
  if (!ctx) return null
  const cell = selectedTableCell(ctx.root)
  const table = cell?.closest('table')
  if (!cell || !table) return null
  return { root: ctx.root, cell, table: table as HTMLTableElement }
}

/** 表格是直接改 DOM 的，改完让 Vditor 从 DOM 重新生成 markdown，保持光标与滚动位置 */
function commitTableChange(root: HTMLElement, focusCell: HTMLTableCellElement | null) {
  const editor = vditor.value
  if (!editor) return
  applyLocalValue(editor.getValue())
  const editable = root.querySelector<HTMLElement>('pre.vditor-reset') ?? root
  editable.focus()
  if (focusCell) {
    try {
      const range = document.createRange()
      range.selectNodeContents(focusCell)
      range.collapse(false)
      const selection = window.getSelection()
      selection?.removeAllRanges()
      selection?.addRange(range)
    } catch {
      // 光标恢复失败不影响表格内容
    }
  }
  syncTableOps()
}

function addTableRow(position: 'above' | 'below') {
  const ctx = activeTableContext()
  const row = ctx?.cell.parentElement as HTMLTableRowElement | null
  if (!ctx || !row) return
  const columnCount = ctx.table.rows[0]?.cells.length ?? 1
  const newRow = document.createElement('tr')
  for (let i = 0; i < columnCount; i += 1) newRow.appendChild(document.createElement('td'))
  const tbody = ctx.table.tBodies[0] ?? ctx.table.appendChild(document.createElement('tbody'))
  const isHeader = row.parentElement?.tagName === 'THEAD'
  if (isHeader) {
    // markdown 表格只有一行表头，表头里的「上/下」统一插到表体开头
    tbody.insertBefore(newRow, tbody.firstChild)
  } else if (position === 'above') {
    row.before(newRow)
  } else {
    row.after(newRow)
  }
  commitTableChange(ctx.root, newRow.cells[0] ?? null)
}

function addTableColumn(position: 'left' | 'right') {
  const ctx = activeTableContext()
  if (!ctx) return
  const currentRow = ctx.cell.parentElement as HTMLTableRowElement | null
  if (!currentRow) return
  const targetIndex = position === 'right' ? ctx.cell.cellIndex + 1 : ctx.cell.cellIndex
  for (const row of Array.from(ctx.table.rows)) {
    const inHead = row.parentElement?.tagName === 'THEAD'
    const newCell = document.createElement(inHead ? 'th' : 'td')
    const reference = row.cells[targetIndex] ?? null
    if (reference) row.insertBefore(newCell, reference)
    else row.appendChild(newCell)
  }
  commitTableChange(ctx.root, currentRow.cells[targetIndex] ?? null)
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
    <div
      v-if="tableOps"
      class="note-table-ops"
      :style="{ left: `${tableOps.left}px`, top: `${tableOps.top}px`, transform: tableOps.transform }"
      @mousedown.prevent
    >
      <button type="button" title="在上方插入一行" @click="addTableRow('above')">↑ 行</button>
      <button type="button" title="在下方插入一行" @click="addTableRow('below')">↓ 行</button>
      <button type="button" title="在左侧插入一列" @click="addTableColumn('left')">← 列</button>
      <button type="button" title="在右侧插入一列" @click="addTableColumn('right')">→ 列</button>
    </div>
    <div class="note-editor-statusbar">
      <span class="note-editor-words">{{ wordCount }} 字</span>
      <span class="note-editor-state" :class="{ 'is-editing': editing }">
        {{ editing ? '编辑中…' : '已保存' }}
      </span>
    </div>
  </div>
</template>

<style scoped>
/* 编辑器外壳：不再画卡片边框，笔记区与页面融为一体（参考 Typora 的整页编辑） */
.note-editor {
  --note-border-color: var(--border-color);
  --note-bg: var(--bg-primary);
  --note-bg-soft: var(--bg-secondary);
  --note-text: var(--text-primary);
  --note-text-secondary: var(--text-secondary);
  --note-text-muted: var(--text-muted);
  --note-primary: var(--primary);

  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
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
  height: 30px;
  padding: 0 4px 6px;
  background: transparent;
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
  --toolbar-height: 32px;

  /* Vditor 有自己的同名变量，这里用外壳变量重新映射一遍 */
  --border-color: var(--note-border-color);
  --second-color: var(--note-text-muted);
  --panel-background-color: var(--note-bg);
  --panel-shadow: 0 6px 20px rgba(15, 23, 42, 0.12);
  --toolbar-background-color: var(--note-bg);
  --toolbar-icon-color: var(--note-text-secondary);
  --toolbar-icon-hover-color: var(--note-text);
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

/* ========== 工具栏：扁平、悬停高亮、tooltip 放在下方 ========== */
.note-editor :deep(.vditor-toolbar) {
  padding-top: 6px;
  padding-bottom: 4px;
  background: var(--note-bg);
  border-bottom: 0;
}
.note-editor :deep(.vditor-toolbar--pin) {
  z-index: 3;
}
.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 32px;
  padding: 0;
  margin: 0 1px;
  border-radius: 6px;
  color: var(--note-text-secondary);
  transition: background-color 0.15s ease, color 0.15s ease;
}
.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped svg) {
  width: 17px;
  height: 17px;
}
.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped:hover),
.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped:focus) {
  color: var(--note-text);
  background: var(--note-bg-soft);
}
.note-editor :deep(.vditor-toolbar__item .vditor-tooltipped:active) {
  background: var(--note-border-color);
}
.note-editor :deep(.vditor-toolbar__divider) {
  height: 18px;
  margin: 7px 7px;
  border-left-color: var(--note-border-color);
}

/* Vditor 默认把 tooltip 放在按钮上方，而工具栏贴着编辑器顶部会被 overflow 裁掉；
   这里改到下方，并换成更现代的深色气泡。 */
.note-editor :deep(.vditor-toolbar .vditor-tooltipped::after) {
  top: calc(100% + 8px);
  bottom: auto;
  right: auto;
  left: 50%;
  transform: translateX(-50%);
  margin: 0;
  padding: 5px 9px;
  border-radius: 6px;
  background: #1f2329;
  color: #fff;
  font-size: 12px;
  line-height: 1.5;
  font-weight: 400;
  letter-spacing: 0.2px;
  white-space: nowrap;
  box-shadow: 0 6px 18px rgba(15, 23, 42, 0.18);
  z-index: 20;
  opacity: 1;
  animation: none;
}
.note-editor :deep(.vditor-toolbar .vditor-tooltipped::before) {
  top: calc(100% + 3px);
  bottom: auto;
  right: auto;
  left: 50%;
  margin: 0 0 0 -5px;
  border: 5px solid transparent;
  border-top-color: transparent;
  border-bottom-color: #1f2329;
  z-index: 21;
  opacity: 1;
  animation: none;
}

/* ========== 正文 ========== */
.note-editor :deep(.vditor-content) {
  background: var(--note-bg);
}

.note-editor :deep(.vditor-ir pre.vditor-reset),
.note-editor :deep(.vditor-wysiwyg pre.vditor-reset),
.note-editor :deep(.vditor-sv),
.note-editor :deep(.vditor-preview) {
  font-family: inherit;
  font-size: 15px;
  line-height: 1.75;
  color: var(--note-text);
  background: var(--note-bg);
}

.note-editor :deep(.vditor-reset) {
  color: var(--note-text);
  --blockquote-border-color: var(--note-border-color);
  --blockquote-text-color: var(--note-text-secondary);
}

/* IR 左侧的 H1~H6 等级标签在 Typora 里没有，去掉更接近纯写作视图 */
.note-editor :deep(.vditor-ir .vditor-reset > h1::before),
.note-editor :deep(.vditor-ir .vditor-reset > h2::before),
.note-editor :deep(.vditor-ir .vditor-reset > h3::before),
.note-editor :deep(.vditor-ir .vditor-reset > h4::before),
.note-editor :deep(.vditor-ir .vditor-reset > h5::before),
.note-editor :deep(.vditor-ir .vditor-reset > h6::before) {
  display: none;
}

/* 所见即所得模式套用同样的克制风格 */
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h1::before),
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h2::before),
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h3::before),
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h4::before),
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h5::before),
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h6::before),
.note-editor :deep(.vditor-wysiwyg div.vditor-wysiwyg__block::before) {
  content: none !important;
  display: none !important;
}
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h1),
.note-editor :deep(.vditor-wysiwyg .vditor-reset > h2) {
  padding-bottom: 0;
  border-bottom: 0;
}
.note-editor :deep(.vditor-wysiwyg .vditor-copy) {
  display: none !important;
}

/* 标题不带下划线，层级靠字号和字重区分 */
.note-editor :deep(.vditor-ir .vditor-reset > h1),
.note-editor :deep(.vditor-ir .vditor-reset > h2),
.note-editor :deep(.vditor-ir .vditor-reset > h3),
.note-editor :deep(.vditor-ir .vditor-reset > h4),
.note-editor :deep(.vditor-ir .vditor-reset > h5),
.note-editor :deep(.vditor-ir .vditor-reset > h6) {
  padding-bottom: 0;
  border-bottom: 0;
}

.note-editor :deep(.vditor-reset blockquote:not(.callout)) {
  color: var(--note-text-secondary);
}
.note-editor :deep(.vditor-reset blockquote:not(.callout)::before) {
  background-color: var(--note-border-color);
}

/* 行内代码：细边框小胶囊；只作用于非 pre 内的 code */
.note-editor :deep(.vditor-reset :not(pre) > code) {
  padding: 2px 6px;
  border: 1px solid var(--note-border-color);
  border-radius: 4px;
  background: var(--note-bg-soft);
  color: var(--note-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.86em;
}

/* ========== 代码块：圆角面板，编辑时也保留语法高亮 ========== */
.note-editor :deep(.vditor-ir div[data-type='code-block']),
.note-editor :deep(.vditor-wysiwyg div[data-type='code-block']) {
  position: relative;
  margin: 14px 0;
  border: 1px solid var(--note-border-color);
  border-radius: 8px;
  background: var(--note-bg-soft);
  overflow: hidden;
}

/* 收起态会留下不可见的 marker 占位，会把面板撑出一大块空白，这里直接隐藏 */
.note-editor :deep(.vditor-ir div[data-type='code-block']) {
  white-space: normal;
}
.note-editor :deep(.vditor-ir div[data-type='code-block']:not(.vditor-ir__node--expand) .vditor-ir__marker--info),
.note-editor :deep(.vditor-ir div[data-type='code-block']:not(.vditor-ir__node--expand) > pre.vditor-ir__marker--pre) {
  display: none;
}

/* 展开态不再用伪元素画出 ``` 围栏 */
.note-editor :deep(.vditor-ir__node--expand[data-type='code-block']::before),
.note-editor :deep(.vditor-ir__node--expand[data-type='code-block']::after) {
  content: none !important;
  display: none !important;
}

/* 语言标签绝对定位到右上角，不占文档流，避免把面板撑高 */
.note-editor :deep(.vditor-ir div[data-type='code-block'] .vditor-ir__marker--info) {
  position: absolute;
  top: 7px;
  right: 12px;
  z-index: 2;
  display: block;
  padding: 0;
  font-size: 11px;
  line-height: 1;
  text-align: right;
  color: var(--note-text-muted);
  pointer-events: none;
  user-select: none;
}

.note-editor :deep(.vditor-ir div[data-type='code-block'] pre),
.note-editor :deep(.vditor-wysiwyg div[data-type='code-block'] pre) {
  margin: 0;
  padding: 12px 16px;
  border: 0;
  border-radius: 0;
  background: transparent;
  overflow-x: auto;
}

.note-editor :deep(.vditor-ir div[data-type='code-block'] pre code),
.note-editor :deep(.vditor-wysiwyg div[data-type='code-block'] pre code) {
  padding: 0;
  border: 0;
  background: transparent;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.7;
}

/* 展开态的可编辑 pre 必须是块级：Vditor 默认把它当 inline，
   行内元素里放块级 code 会算出巨大的行高，表现就是点一下代码块突然变得很高。 */
.note-editor :deep(.vditor-ir .vditor-ir__node--expand[data-type='code-block'] > pre.vditor-ir__marker--pre) {
  display: block;
  position: relative;
  z-index: 1;
  white-space: pre-wrap;
  word-break: break-word;
  color: transparent;
  caret-color: var(--note-text);
}
.note-editor :deep(.vditor-ir .vditor-ir__node--expand[data-type='code-block'] > pre.vditor-ir__marker--pre code) {
  color: transparent !important;
}

/* 把渲染好的高亮层垫在编辑层下面：编辑时既能看见语法颜色，尺寸也保持不变 */
.note-editor :deep(.vditor-ir .vditor-ir__node--expand[data-type='code-block'] > pre.vditor-ir__preview) {
  display: block;
  position: absolute;
  inset: 0;
  z-index: 0;
  margin: 0;
  padding: 12px 16px;
  overflow: hidden;
  background: transparent;
  pointer-events: none;
  user-select: none;
}
.note-editor :deep(.vditor-ir .vditor-ir__node--expand[data-type='code-block'] > pre.vditor-ir__preview code) {
  display: block;
  padding: 0 !important;
  white-space: pre-wrap;
  word-break: break-word;
  overflow: hidden;
  max-height: none !important;
}

/* 所见即所得模式下的代码块同样是「可编辑层 + 高亮层」叠放，
   通过 :has() 判断 Vditor 何时把 __pre 切到 display:block。 */
.note-editor :deep(.vditor-wysiwyg div[data-type='code-block']:has(> pre.vditor-wysiwyg__pre[style*='block'])) > pre.vditor-wysiwyg__pre {
  position: relative;
  z-index: 1;
  margin: 0;
  padding: 12px 16px;
  color: transparent;
  caret-color: var(--note-text);
  white-space: pre-wrap;
  word-break: break-word;
  overflow: hidden;
}
.note-editor :deep(.vditor-wysiwyg div[data-type='code-block']:has(> pre.vditor-wysiwyg__pre[style*='block'])) > pre.vditor-wysiwyg__pre code {
  color: transparent !important;
}
.note-editor :deep(.vditor-wysiwyg div[data-type='code-block']:has(> pre.vditor-wysiwyg__pre[style*='block'])) > pre.vditor-wysiwyg__preview {
  position: absolute;
  inset: 0;
  z-index: 0;
  margin: 0;
  padding: 12px 16px;
  overflow: hidden;
  background: transparent;
  pointer-events: none;
  user-select: none;
}
.note-editor :deep(.vditor-wysiwyg div[data-type='code-block']:has(> pre.vditor-wysiwyg__pre[style*='block'])) > pre.vditor-wysiwyg__preview code {
  display: block;
  padding: 0 !important;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: none !important;
  overflow: hidden;
}

/* IR 编辑时用不到代码块右上角的复制按钮 */
.note-editor :deep(.vditor-ir .vditor-copy) {
  display: none !important;
}

/* ========== 表格浮动工具条：加行 / 加列 ========== */
.note-table-ops {
  position: absolute;
  z-index: 20;
  display: flex;
  gap: 2px;
  padding: 3px;
  border: 1px solid var(--note-border-color);
  border-radius: 8px;
  background: var(--note-bg);
  box-shadow: 0 6px 18px rgba(15, 23, 42, 0.12);
}
.note-table-ops button {
  height: 22px;
  padding: 0 7px;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: var(--note-text-muted);
  font-size: 11px;
  line-height: 22px;
  white-space: nowrap;
  cursor: pointer;
}
.note-table-ops button:hover {
  background: var(--note-bg-soft);
  color: var(--note-text);
}

/* ========== 源码模式：只显示可编辑的 Markdown ========== */
.note-editor :deep(.vditor-toolbar__item.is-source > button) {
  color: var(--note-primary);
  background: var(--note-bg-soft);
}
.note-editor :deep(textarea.vditor-sv) {
  max-width: 800px;
  margin: 0 auto;
  padding: 12px 0 32px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.9;
  color: var(--note-text);
  background: transparent;
  caret-color: var(--note-text);
  border: 0;
  outline: none;
  resize: none;
}

/* ========== 任务列表：自定义方形复选框（更大、可点击） ========== */
.note-editor :deep(.vditor-task) {
  position: relative;
}
.note-editor :deep(.vditor-task > input[type='checkbox']) {
  appearance: none;
  -webkit-appearance: none;
  position: absolute;
  left: -1.6em;
  top: calc((1.75em - 16px) / 2);
  width: 16px;
  height: 16px;
  margin: 0;
  font-size: inherit;
  border: 1.5px solid var(--note-border-color);
  border-radius: 4px;
  background: var(--note-bg) center / 10px 10px no-repeat;
  cursor: pointer;
  transition: background-color 0.15s ease, border-color 0.15s ease;
}
.note-editor :deep(.vditor-task input[type='checkbox']:hover) {
  border-color: var(--note-primary);
}
.note-editor :deep(.vditor-task input[type='checkbox']:checked) {
  border-color: var(--note-primary);
  background-color: var(--note-primary);
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 12'%3E%3Cpath d='M2.4 6.1 5 8.7l4.6-5.1' fill='none' stroke='%23fff' stroke-width='1.9' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
}
.note-editor :deep(.vditor-task input[type='checkbox']:focus-visible) {
  outline: none;
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.18);
}

/* ========== 表格 ========== */
.note-editor :deep(.vditor-reset table tr) {
  background-color: var(--note-bg);
  border-top-color: var(--note-border-color);
}
.note-editor :deep(.vditor-reset table tr:nth-child(2n)) {
  background-color: var(--note-bg-soft);
}
.note-editor :deep(.vditor-reset table td),
.note-editor :deep(.vditor-reset table th) {
  border-color: var(--note-border-color);
}
</style>
