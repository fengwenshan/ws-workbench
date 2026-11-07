<script setup lang="ts">
import { ref, computed, watch, nextTick, onMounted, onUnmounted } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { Button, Input, Progress, Empty, Modal, message } from 'ant-design-vue'
import type { AttachmentMeta, EntityKind, ProjectConfig, ProjectGroup } from '@/types'
import {
  decryptExportFile,
  encryptUploadBytes,
  encryptUploadFile,
  removeAttachmentFile,
  resolveUploadDir,
} from '@/api/attachments'
import { pickFiles, pickSavePath } from '@/utils/dialog'
import { checkDir } from '@/api/system'
import AttachmentPreview from '@/components/AttachmentPreview.vue'
import { basename } from '@/utils/path'
import { formatDateTime } from '@/utils/time'

const props = defineProps<{
  kind: EntityKind
  entity: ProjectGroup | ProjectConfig
  /** 设置里配置的全局上传目录（null 表示落到 App 私有目录） */
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  addAttachment: [meta: AttachmentMeta]
  removeAttachment: [id: string]
  renameAttachment: [id: string, name: string]
  openSettings: []
  log: [level: 'info' | 'warn' | 'error' | 'success', message: string]
}>()

const resolvedDir = ref('')
const uploading = ref(false)
const busyId = ref<string | null>(null)
const dragActive = ref(false)
// 设置里的上传目录是否已不存在（App 私有目录会按需创建，不算失效）
const dirMissing = ref(false)
// 目录检测是异步的，快速切换实体时只认最后一次结果
let dirCheckSeq = 0

const progressId = ref<string | null>(null)
const progressDone = ref(0)
const progressTotal = ref(0)

const attachments = computed<AttachmentMeta[]>(() => props.entity.attachments ?? [])

const progressPercent = computed(() => {
  if (progressTotal.value <= 0) return 0
  return Math.min(100, Math.floor((progressDone.value / progressTotal.value) * 100))
})

const progressVisible = computed(
  () => uploading.value || busyId.value !== null || progressId.value !== null,
)

async function resolveDir() {
  try {
    resolvedDir.value = await resolveUploadDir(props.defaultUploadDir)
  } catch (err: any) {
    resolvedDir.value = ''
    emit('log', 'error', `解析上传目录失败: ${err}`)
    return
  }
  await refreshDirMissing()
}

// 只提示、不阻断：设置里的目录没了就去设置里改，附件元信息仍跟着实体 id 保留
async function refreshDirMissing() {
  const dir = resolvedDir.value
  const seq = ++dirCheckSeq
  // App 私有目录首次上传时才创建，不存在属于正常状态
  if (!dir || !props.defaultUploadDir) {
    dirMissing.value = false
    return
  }
  try {
    const status = await checkDir(dir)
    if (seq === dirCheckSeq) dirMissing.value = !status.is_dir
  } catch {
    if (seq === dirCheckSeq) dirMissing.value = false
  }
}

watch(
  () => props.defaultUploadDir,
  () => {
    resolveDir()
  },
)

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

function resetProgress() {
  progressId.value = null
  progressDone.value = 0
  progressTotal.value = 0
}

// ========== 上传 ==========
async function pickAndUpload() {
  const paths = await pickFiles()
  if (!paths.length) return
  await uploadPaths(paths)
}

async function uploadPaths(paths: string[]) {
  if (!paths.length) return
  if (!resolvedDir.value) await resolveDir()
  const dir = resolvedDir.value
  if (!dir) {
    message.error('未解析到上传目录')
    return
  }
  uploading.value = true
  try {
    for (const path of paths) {
      const name = basename(path)
      try {
        const meta = await encryptUploadFile(path, dir, name)
        emit('addAttachment', meta)
        emit('log', 'success', `已加密上传: ${name}`)
      } catch (err: any) {
        emit('log', 'error', `上传失败（${name}）: ${err}`)
        message.error(`上传失败: ${err}`)
      }
    }
  } finally {
    uploading.value = false
    resetProgress()
  }
}

// ========== 预览 / 下载 ==========
// 预览由 AttachmentPreview 走 `wst-img` 协议按需解密渲染，不落临时文件；
// 旧的「打开」（解密到系统临时目录再交给默认程序）已移除。
const previewOpen = ref(false)
const previewMeta = ref<AttachmentMeta | null>(null)

function openPreview(meta: AttachmentMeta) {
  previewMeta.value = meta
  previewOpen.value = true
}

async function handleDownload(meta: AttachmentMeta) {
  const target = await pickSavePath(meta.name)
  if (!target) return
  busyId.value = meta.id
  try {
    await decryptExportFile(meta.dir, meta.id, target)
    emit('log', 'success', `已导出: ${meta.name}`)
  } catch (err: any) {
    handleDecryptError(meta, err)
  } finally {
    busyId.value = null
    resetProgress()
  }
}

function handleDecryptError(meta: AttachmentMeta, err: any) {
  const text = String(err ?? '')
  emit('log', 'error', `解密失败（${meta.name}）: ${text}`)
  if (text.includes('不存在')) {
    Modal.confirm({
      title: '文件不存在',
      content: `附件「${meta.name}」的密文文件已丢失，是否从列表移除该记录？`,
      okText: '从列表移除',
      cancelText: '保留',
      onOk() {
        emit('removeAttachment', meta.id)
      },
    })
  } else {
    message.error(`解密失败: ${text}`)
  }
}

// ========== 重命名 / 删除 ==========
const editingId = ref<string | null>(null)
const editingName = ref('')
const renameInputRef = ref<{ focus?: () => void; select?: () => void } | null>(null)

function setRenameInputEl(el: unknown) {
  renameInputRef.value = (el as { focus?: () => void; select?: () => void }) || null
}

async function startRename(meta: AttachmentMeta) {
  editingId.value = meta.id
  editingName.value = meta.name
  await nextTick()
  renameInputRef.value?.focus?.()
  renameInputRef.value?.select?.()
}

/** 失焦即保存：和回车一致，点到别处也把改好的名字存下来 */
function handleRenameBlur(meta: AttachmentMeta) {
  if (editingId.value !== meta.id) return
  commitRename(meta)
}

function commitRename(meta: AttachmentMeta) {
  if (editingId.value !== meta.id) return
  const next = editingName.value.trim()
  editingId.value = null
  if (next && next !== meta.name) emit('renameAttachment', meta.id, next)
}

function cancelRename() {
  editingId.value = null
  editingName.value = ''
}

function confirmDelete(meta: AttachmentMeta) {
  Modal.confirm({
    title: '删除附件',
    content: `确定要删除附件「${meta.name}」吗？此操作不可恢复。`,
    okText: '删除',
    okType: 'danger',
    cancelText: '取消',
    async onOk() {
      try {
        await removeAttachmentFile(meta.dir, meta.id)
        emit('removeAttachment', meta.id)
        emit('log', 'info', `已删除附件: ${meta.name}`)
      } catch (err: any) {
        message.error(`删除失败: ${err}`)
        throw err
      }
    },
  })
}

// ========== 拖拽上传（页面内 HTML5 拖放） ==========
// 窗口关掉了 Tauri 原生拖放（tauri.conf.json 的 dragDropEnabled: false，留给项目树排序），
// 所以这里走标准的 HTML5 drop：从 dataTransfer 里拿 File，按字节加密上传。
// 热点是整个「上传」页的内容区域（.attachment-panel 就是该页根节点，撑满 tab 内容区），
// 不限于虚线框；虚线框只作视觉提示。
let dragDepth = 0

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
  // 不 preventDefault 的话浏览器不会把 drop 交给我们
  event.preventDefault()
}

function onDragLeave(event: DragEvent) {
  if (!isFileDrag(event)) return
  dragDepth = Math.max(0, dragDepth - 1)
  if (dragDepth === 0) dragActive.value = false
}

async function onDropFiles(event: DragEvent) {
  event.preventDefault()
  dragDepth = 0
  dragActive.value = false
  if (!isFileDrag(event)) return

  const files: File[] = []
  let skippedDir = false
  for (const item of Array.from(event.dataTransfer?.items ?? [])) {
    if (item.kind !== 'file') continue
    // 整目录拖进来暂不支持：没有递归遍历，直接跳过并提示
    if (item.webkitGetAsEntry?.()?.isDirectory) {
      skippedDir = true
      continue
    }
    const file = item.getAsFile()
    if (file) files.push(file)
  }
  if (skippedDir) message.warning('暂不支持拖入文件夹，请拖入具体文件')
  await uploadDroppedFiles(files)
}

/** File → base64（不含 data URL 前缀），后端 encrypt_upload_bytes 再解密成字节加密落盘 */
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

function fileExt(name: string): string {
  const dot = name.lastIndexOf('.')
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : ''
}

// 拖拽走 IPC 传字节，大文件会占用大量内存；超过这个大小建议按规定走「选择文件」的流式上传
const DROP_SIZE_LIMIT = 100 * 1024 * 1024

async function uploadDroppedFiles(files: File[]) {
  if (!files.length) return
  if (!resolvedDir.value) await resolveDir()
  const dir = resolvedDir.value
  if (!dir) {
    message.error('未解析到上传目录')
    return
  }
  uploading.value = true
  try {
    for (const file of files) {
      if (file.size > DROP_SIZE_LIMIT) {
        message.error(`「${file.name}」超过 100MB，请点「选择文件」上传大文件`)
        continue
      }
      try {
        const dataB64 = await fileToBase64(file)
        const meta = await encryptUploadBytes(dir, file.name, fileExt(file.name), dataB64)
        emit('addAttachment', meta)
        emit('log', 'success', `已加密上传: ${file.name}`)
      } catch (err: any) {
        emit('log', 'error', `上传失败（${file.name}）: ${err}`)
        message.error(`上传失败: ${err}`)
      }
    }
  } finally {
    uploading.value = false
    resetProgress()
  }
}

let unlistenProgress: UnlistenFn | null = null

onMounted(async () => {
  await resolveDir()
  unlistenProgress = await listen<{ id: string; done: number; total: number }>(
    'attachment-progress',
    (event) => {
      progressId.value = event.payload.id
      progressDone.value = event.payload.done
      progressTotal.value = event.payload.total
    },
  )
})

onUnmounted(() => {
  unlistenProgress?.()
  unlistenProgress = null
})
</script>

<template>
  <div
    class="attachment-panel"
    :class="{ 'drag-over': dragActive }"
    @dragenter="onDragEnter"
    @dragover="onDragOver"
    @dragleave="onDragLeave"
    @drop="onDropFiles"
  >
    <!-- 目录失效提示：附件记录还在，去设置里换个上传目录即可恢复 -->
    <div v-if="dirMissing" class="dir-missing-row">
      <span class="dir-missing-text">
        ⚠ 设置里的上传地址已不存在，已有附件的密文文件可能已丢失。到「设置 → 附件上传」换个目录后，附件记录仍会保留。
      </span>
      <Button size="small" type="primary" @click="emit('openSettings')">去设置</Button>
    </div>

    <!-- 空状态：没有附件时，这块就是上传提示（不再常驻占地方） -->
    <div
      v-if="attachments.length === 0"
      class="attachment-empty"
      :class="{ active: dragActive }"
    >
      <Empty :image="Empty.PRESENTED_IMAGE_SIMPLE" description="暂无附件" />
      <div class="attachment-empty-text">
        拖拽文件到本页任意位置，或
        <Button type="primary" size="small" :loading="uploading" @click="pickAndUpload">
          选择文件
        </Button>
      </div>
      <div class="attachment-empty-hint">
        上传后立即加密落盘（密文 blob），文件名与内容均加密
      </div>
    </div>

    <!-- 有附件时：只留一行紧凑工具条 -->
    <div v-else class="attachment-toolbar">
      <span class="attachment-toolbar-kicker">
        共 {{ attachments.length }} 个附件，也可直接把文件拖入本页
      </span>
      <Button size="small" :loading="uploading" @click="pickAndUpload">上传文件</Button>
    </div>

    <!-- 进度 -->
    <div v-if="progressVisible" class="progress-row">
      <Progress
        :percent="progressPercent"
        :status="busyId ? 'active' : 'normal'"
        size="small"
      />
    </div>

    <!-- 附件列表（没有附件时由上面的空状态占位） -->
    <div v-if="attachments.length > 0" class="attachment-list">
      <div
        v-for="meta in attachments"
        :key="meta.id"
        class="attachment-item"
        :class="{ busy: busyId === meta.id }"
      >
        <div class="attachment-main">
          <Input
            v-if="editingId === meta.id"
            :ref="setRenameInputEl"
            v-model:value="editingName"
            size="small"
            class="attachment-rename"
            @pressEnter="commitRename(meta)"
            @blur="handleRenameBlur(meta)"
            @keydown.esc="cancelRename"
          />
          <span
            v-else
            class="attachment-name"
            :title="`${meta.name}\n密文文件：${meta.id}.wst`"
          >{{ meta.name }}</span>
          <span class="attachment-meta">
            {{ formatSize(meta.size) }} · {{ formatDateTime(meta.created_at) }}
          </span>
        </div>
        <div class="attachment-actions">
          <Button size="small" :disabled="busyId === meta.id" @click="openPreview(meta)">
            预览
          </Button>
          <Button size="small" :disabled="busyId === meta.id" @click="handleDownload(meta)">
            下载
          </Button>
          <Button size="small" :disabled="busyId === meta.id" @click="startRename(meta)">
            重命名
          </Button>
          <Button size="small" danger :disabled="busyId === meta.id" @click="confirmDelete(meta)">
            删除
          </Button>
        </div>
      </div>
    </div>

    <!-- 预览：图像走 wst-img 协议按需解密，不落临时文件；其他类型给出下载提示 -->
    <AttachmentPreview v-model:open="previewOpen" :meta="previewMeta" />
  </div>
</template>

<style scoped>
.attachment-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
  height: 100%;
  min-height: 0;
  border-radius: 10px;
  transition: background 0.15s, outline-color 0.15s;
}
/* 拖文件进「上传」页：整块内容区都是热点，给个整体高亮 */
.attachment-panel.drag-over {
  outline: 1px dashed var(--primary);
  outline-offset: 4px;
  background: rgba(37, 99, 235, 0.04);
}
/* 上传位置失效提示 */
.dir-missing-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border: 1px solid var(--danger);
  border-radius: 8px;
  background: rgba(220, 38, 38, 0.06);
}
.dir-missing-text {
  flex: 1;
  font-size: 12px;
  line-height: 1.5;
  color: var(--danger);
}
/* 空状态：附件为空时这块就是上传提示，撑满内容区并居中 */
.attachment-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  border: 1px dashed var(--border-color);
  border-radius: 8px;
  padding: 20px 18px 22px;
  text-align: center;
  transition: all 0.15s;
  background: var(--bg-input);
}
.attachment-empty.active {
  border-color: var(--primary);
  background: rgba(37, 99, 235, 0.06);
}
.attachment-empty-text {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  margin-top: 6px;
  font-size: 14px;
  color: var(--text-secondary);
}
.attachment-empty-hint {
  margin-top: 8px;
  font-size: 12px;
  color: var(--text-muted);
}
/* 有附件时：只占一行，上传入口留着 */
.attachment-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.attachment-toolbar-kicker {
  font-size: 12px;
  color: var(--text-muted);
}
.progress-row {
  padding: 0 4px;
}
.attachment-list {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.attachment-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
}
.attachment-item.busy {
  opacity: 0.7;
}
.attachment-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  flex: 1;
}
.attachment-name {
  font-size: 14px;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.attachment-rename {
  max-width: 320px;
}
.attachment-meta {
  font-size: 12px;
  color: var(--text-muted);
}
.attachment-actions {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}
</style>
