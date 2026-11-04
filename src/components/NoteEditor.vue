<script setup lang="ts">
import { ref, watch, onBeforeUnmount } from 'vue'
import { MdEditor } from 'md-editor-v3'
import type { ToolbarNames } from 'md-editor-v3'
import 'md-editor-v3/lib/style.css'
import { isWindows } from '@/platform'
import type { AttachmentMeta, LogLevel } from '@/types'
import { encryptUploadBytes, resolveUploadDir } from '@/api/attachments'
import { errorText } from '@/utils/error'

const props = defineProps<{
  note: string
  uploadDir: string | null
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  update: [note: string]
  addAttachment: [meta: AttachmentMeta]
  log: [level: LogLevel, message: string]
}>()

const text = ref(props.note ?? '')

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

async function onUploadImg(files: File[], callback: (urls: string[]) => void) {
  let uploadDir: string
  try {
    uploadDir = await resolveUploadDir(props.uploadDir, props.defaultUploadDir)
  } catch (e) {
    emit('log', 'error', `解析上传目录失败: ${errorText(e)}`)
    callback([])
    return
  }

  const urls: string[] = []
  for (const file of files) {
    try {
      const dataB64 = await readFileAsBase64(file)
      if (!dataB64) throw new Error('文件内容为空')
      const meta = await encryptUploadBytes(
        uploadDir,
        file.name || 'image',
        extOf(file),
        dataB64,
      )
      emit('addAttachment', meta)
      emit('log', 'success', `图片已加密上传: ${meta.name}`)
      urls.push(imageUrl(meta.id, meta.ext, meta.dir))
    } catch (e) {
      emit('log', 'error', `图片上传失败: ${file.name} - ${errorText(e)}`)
    }
  }
  callback(urls)
}
</script>

<template>
  <div class="note-editor">
    <MdEditor
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
