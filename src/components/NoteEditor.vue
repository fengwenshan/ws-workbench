<script setup lang="ts">
import { ref, watch, onBeforeUnmount } from 'vue'
import { MdEditor } from 'md-editor-v3'
import type { ExposeParam, ToolbarNames } from 'md-editor-v3'
import 'md-editor-v3/lib/style.css'
import { isWindows } from '@/platform'
import type { LogLevel } from '@/types'
import { encryptUploadBytes, resolveUploadDir } from '@/api/attachments'
import { errorText } from '@/utils/error'

const props = defineProps<{
  note: string
  /** 设置里配置的全局上传目录（null 表示 App 私有目录） */
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  update: [note: string]
  log: [level: LogLevel, message: string]
}>()

const text = ref(props.note ?? '')
const editorRef = ref<ExposeParam | null>(null)

// 外部（切换实体）传入新笔记时同步到本地
watch(
  () => props.note,
  (value) => {
    const next = value ?? ''
    if (next !== text.value) text.value = next
  },
)

// 本地编辑防抖回传，沿用 ProjectConfig 的保存模式
let timer: number | undefined
watch(text, (value) => {
  if (timer) window.clearTimeout(timer)
  timer = window.setTimeout(() => {
    timer = undefined
    emit('update', value)
  }, 500)
})

onBeforeUnmount(() => {
  if (timer) {
    window.clearTimeout(timer)
    timer = undefined
  }
  // 卸载前把未落盘的编辑补交，避免快速切换实体丢内容
  if (text.value !== (props.note ?? '')) emit('update', text.value)
})

const toolbars: ToolbarNames[] = [
  'bold',
  'italic',
  'strikeThrough',
  'title',
  'quote',
  'unorderedList',
  'orderedList',
  'task',
  'codeRow',
  'code',
  'link',
  'image',
  'table',
  '-',
  'revoke',
  'next',
  '-',
  'prettier',
  'catalog',
  'preview',
  'previewOnly',
]

// 自定义协议前缀：macOS/Linux 为 wst-img://，Windows 为 http://wst-img.localhost
function imageUrl(id: string, ext: string, dir: string): string {
  const prefix = isWindows() ? 'http://wst-img.localhost' : 'wst-img://localhost'
  const query = `id=${encodeURIComponent(id)}&ext=${encodeURIComponent(ext)}&dir=${encodeURIComponent(dir)}`
  return `${prefix}/img?${query}`
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

function extOf(file: File): string {
  const name = file.name || ''
  const idx = name.lastIndexOf('.')
  if (idx >= 0 && idx < name.length - 1) return name.slice(idx + 1).toLowerCase()
  const type = file.type || ''
  const slash = type.indexOf('/')
  return slash >= 0 ? type.slice(slash + 1).toLowerCase() : ''
}

async function uploadImage(
  file: File,
  uploadDir: string,
  name: string,
): Promise<string> {
  const dataB64 = await readFileAsBase64(file)
  if (!dataB64) throw new Error('文件内容为空')
  const meta = await encryptUploadBytes(uploadDir, name, extOf(file), dataB64)
  // 笔记里的图片只在笔记里用，不进「上传」页的附件列表
  emit('log', 'success', `笔记图片已加密保存: ${meta.name}`)
  return imageUrl(meta.id, meta.ext, meta.dir)
}

async function uploadImages(
  files: File[],
  nameOf: (file: File, index: number) => string = (file) => file.name || 'image',
): Promise<string[]> {
  const uploadDir = await resolveUploadDir(props.defaultUploadDir)
  const urls: string[] = []
  for (const [index, file] of files.entries()) {
    try {
      urls.push(await uploadImage(file, uploadDir, nameOf(file, index)))
    } catch (e) {
      emit('log', 'error', `图片上传失败: ${file.name || '剪贴板图片'} - ${errorText(e)}`)
    }
  }
  return urls
}

async function onUploadImg(files: File[], callback: (urls: string[]) => void) {
  try {
    callback(await uploadImages(files))
  } catch (e) {
    emit('log', 'error', `解析上传目录失败: ${errorText(e)}`)
    callback([])
  }
}

// ========== 粘贴图片 ==========
// md-editor 自带的粘贴上传只认 clipboardData.files，有些 WebView 里图片只出现在
// clipboardData.items 里，粘贴图片就悄无声息了。这里统一兜住：拿到图片就自己上传并插入。
function pasteImageFiles(data: DataTransfer): File[] {
  const files: File[] = []
  for (const item of Array.from(data.items ?? [])) {
    if (item.kind !== 'file' || !item.type.startsWith('image/')) continue
    const file = item.getAsFile()
    if (file) files.push(file)
  }
  if (files.length === 0) {
    for (const file of Array.from(data.files ?? [])) {
      if (file.type.startsWith('image/')) files.push(file)
    }
  }
  return files
}

function onPaste(event: ClipboardEvent) {
  if (!event.clipboardData) return
  const files = pasteImageFiles(event.clipboardData)
  if (files.length === 0) return // 普通文本粘贴，交给编辑器
  // 图片由自己处理，拦掉编辑器默认的粘贴行为
  event.preventDefault()
  event.stopPropagation()
  void pasteImages(files)
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

async function pasteImages(files: File[]) {
  let urls: string[]
  try {
    urls = await uploadImages(files, (_file, index) =>
      pastedImageName(index, files.length, extOf(files[index])),
    )
  } catch (e) {
    emit('log', 'error', `解析上传目录失败: ${errorText(e)}`)
    return
  }
  if (urls.length === 0) return
  const markdown = urls.map((url) => `![](${url})`).join('\n')
  const editor = editorRef.value
  if (editor) {
    editor.insert(() => ({ targetValue: `${markdown}\n` }))
  } else {
    text.value = text.value ? `${text.value}\n${markdown}\n` : `${markdown}\n`
  }
}
</script>

<template>
  <div class="note-editor" @paste.capture="onPaste">
    <MdEditor
      ref="editorRef"
      v-model="text"
      class="note-md-editor"
      language="zh-CN"
      theme="light"
      preview-theme="github"
      code-theme="github"
      :toolbars="toolbars"
      :show-code-row-number="true"
      :no-mermaid="true"
      :no-katex="true"
      :on-upload-img="onUploadImg"
    />
  </div>
</template>

<style scoped>
.note-editor {
  height: 100%;
  min-height: 0;
}
.note-md-editor {
  height: 100%;
}
.note-editor :deep(.md-editor) {
  height: 100%;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  --md-bk-color: var(--bg-primary);
  --md-color: var(--text-primary);
  --md-border-color: var(--border-color);
}
.note-editor :deep(.md-editor-toolbar-wrapper) {
  border-bottom: 1px solid var(--border-color);
}
</style>
