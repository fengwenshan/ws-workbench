<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { open, save } from '@tauri-apps/plugin-dialog'
import { open as openPath } from '@tauri-apps/plugin-shell'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { Button, Input, Progress, Empty, Tag, Modal, message } from 'ant-design-vue'
import type { AttachmentMeta, EntityKind, ProjectConfig, ProjectGroup } from '@/types'

const props = defineProps<{
  kind: EntityKind
  entity: ProjectGroup | ProjectConfig
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  updateUploadDir: [dir: string | null]
  addAttachment: [meta: AttachmentMeta]
  removeAttachment: [id: string]
  renameAttachment: [id: string, name: string]
  log: [level: 'info' | 'warn' | 'error' | 'success', message: string]
}>()

const resolvedDir = ref('')
const uploading = ref(false)
const busyId = ref<string | null>(null)
const dragActive = ref(false)
const dropZoneEl = ref<HTMLElement | null>(null)

const progressId = ref<string | null>(null)
const progressDone = ref(0)
const progressTotal = ref(0)

const attachments = computed<AttachmentMeta[]>(() => props.entity.attachments ?? [])

const dirSource = computed(() => {
  if (props.entity.upload_dir) return '自定义'
  if (props.defaultUploadDir) return '全局默认'
  return 'App 私有'
})

const progressPercent = computed(() => {
  if (progressTotal.value <= 0) return 0
  return Math.min(100, Math.floor((progressDone.value / progressTotal.value) * 100))
})

const progressVisible = computed(
  () => uploading.value || busyId.value !== null || progressId.value !== null,
)

async function resolveDir() {
  try {
    resolvedDir.value = await invoke<string>('resolve_upload_dir', {
      entityDir: props.entity.upload_dir,
      defaultDir: props.defaultUploadDir,
    })
  } catch (err: any) {
    resolvedDir.value = ''
    emit('log', 'error', `解析上传目录失败: ${err}`)
  }
}

watch(
  () => [props.entity.upload_dir, props.defaultUploadDir],
  () => {
    resolveDir()
  },
)

function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const idx = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  return idx >= 0 ? trimmed.slice(idx + 1) : trimmed
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

function formatTime(ms: number): string {
  try {
    return new Date(ms).toLocaleString()
  } catch {
    return ''
  }
}

function resetProgress() {
  progressId.value = null
  progressDone.value = 0
  progressTotal.value = 0
}

// ========== 上传位置 ==========
async function changeDir() {
  const selected = await open({ directory: true, multiple: false })
  if (!selected) return
  emit('updateUploadDir', selected as string)
}

function restoreDefault() {
  emit('updateUploadDir', null)
}

// ========== 上传 ==========
async function pickAndUpload() {
  const picked = await open({ multiple: true, directory: false })
  if (!picked) return
  const paths = Array.isArray(picked) ? picked : [picked]
  await uploadPaths(paths as string[])
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
        const meta = await invoke<AttachmentMeta>('encrypt_upload_file', {
          srcPath: path,
          uploadDir: dir,
          name,
        })
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

// ========== 打开 / 下载 ==========
async function handleOpen(meta: AttachmentMeta) {
  busyId.value = meta.id
  try {
    const tempPath = await invoke<string>('decrypt_to_temp', {
      blobDir: meta.dir,
      id: meta.id,
      name: meta.name,
    })
    await openPath(tempPath)
    emit('log', 'info', `已解密预览: ${meta.name}`)
  } catch (err: any) {
    handleDecryptError(meta, err)
  } finally {
    busyId.value = null
    resetProgress()
  }
}

async function handleDownload(meta: AttachmentMeta) {
  const target = await save({ defaultPath: meta.name })
  if (!target) return
  busyId.value = meta.id
  try {
    await invoke('decrypt_export_file', {
      blobDir: meta.dir,
      id: meta.id,
      targetPath: target,
    })
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

function startRename(meta: AttachmentMeta) {
  editingId.value = meta.id
  editingName.value = meta.name
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
        await invoke('remove_attachment_file', { blobDir: meta.dir, id: meta.id })
        emit('removeAttachment', meta.id)
        emit('log', 'info', `已删除附件: ${meta.name}`)
      } catch (err: any) {
        message.error(`删除失败: ${err}`)
        throw err
      }
    },
  })
}

// ========== 拖拽上传（Tauri 原生文件拖放） ==========
function isInsideZone(position: { x: number; y: number } | undefined): boolean {
  const el = dropZoneEl.value
  if (!el || !position) return false
  const rect = el.getBoundingClientRect()
  const dpr = window.devicePixelRatio || 1
  const x = position.x / dpr
  const y = position.y / dpr
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
}

let unlistenDrop: UnlistenFn | null = null
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
  try {
    unlistenDrop = await getCurrentWebview().onDragDropEvent((event: any) => {
      const payload = event.payload
      if (payload.type === 'enter' || payload.type === 'over') {
        dragActive.value = isInsideZone(payload.position)
      } else if (payload.type === 'drop') {
        const inside = isInsideZone(payload.position)
        dragActive.value = false
        if (inside && Array.isArray(payload.paths) && payload.paths.length) {
          uploadPaths(payload.paths)
        }
      } else {
        dragActive.value = false
      }
    })
  } catch (err: any) {
    emit('log', 'warn', `拖拽上传不可用: ${err}`)
  }
})

onUnmounted(() => {
  unlistenDrop?.()
  unlistenDrop = null
  unlistenProgress?.()
  unlistenProgress = null
})
</script>

<template>
  <div class="attachment-panel">
    <!-- 上传位置 -->
    <div class="upload-dir-row">
      <span class="upload-dir-label">上传位置</span>
      <Tag :color="dirSource === '自定义' ? 'blue' : dirSource === '全局默认' ? 'green' : 'default'">
        {{ dirSource }}
      </Tag>
      <span class="upload-dir-path" :title="resolvedDir">{{ resolvedDir || '—' }}</span>
      <div class="upload-dir-actions">
        <Button size="small" @click="changeDir">更改</Button>
        <Button
          size="small"
          :disabled="!entity.upload_dir"
          @click="restoreDefault"
        >
          恢复默认
        </Button>
      </div>
    </div>

    <!-- 上传区 -->
    <div
      ref="dropZoneEl"
      class="drop-zone"
      :class="{ active: dragActive }"
    >
      <div class="drop-zone-text">
        拖拽文件到此处，或
        <Button type="primary" size="small" :loading="uploading" @click="pickAndUpload">
          选择文件
        </Button>
      </div>
      <div class="drop-zone-hint">
        上传后立即加密落盘（密文 blob），文件名与内容均加密
      </div>
    </div>

    <!-- 进度 -->
    <div v-if="progressVisible" class="progress-row">
      <Progress
        :percent="progressPercent"
        :status="busyId ? 'active' : 'normal'"
        size="small"
      />
    </div>

    <!-- 附件列表 -->
    <div class="attachment-list">
      <div v-if="attachments.length === 0" class="attachment-empty">
        <Empty :image="Empty.PRESENTED_IMAGE_SIMPLE" description="暂无附件" />
      </div>
      <div
        v-for="meta in attachments"
        :key="meta.id"
        class="attachment-item"
        :class="{ busy: busyId === meta.id }"
      >
        <div class="attachment-main">
          <Input
            v-if="editingId === meta.id"
            v-model:value="editingName"
            size="small"
            class="attachment-rename"
            @pressEnter="commitRename(meta)"
            @blur="commitRename(meta)"
            @keydown.esc="cancelRename"
          />
          <span v-else class="attachment-name" :title="meta.name">{{ meta.name }}</span>
          <span class="attachment-meta">
            {{ formatSize(meta.size) }} · {{ formatTime(meta.created_at) }}
          </span>
        </div>
        <div class="attachment-actions">
          <Button size="small" :disabled="busyId === meta.id" @click="handleOpen(meta)">
            打开
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
  </div>
</template>

<style scoped>
.attachment-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
  height: 100%;
  min-height: 0;
}
.upload-dir-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  flex-wrap: wrap;
}
.upload-dir-label {
  font-size: 13px;
  color: var(--text-secondary);
  flex-shrink: 0;
}
.upload-dir-path {
  flex: 1;
  min-width: 120px;
  font-size: 12px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.upload-dir-actions {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}
.drop-zone {
  border: 1px dashed var(--border-color);
  border-radius: 8px;
  padding: 18px;
  text-align: center;
  transition: all 0.15s;
  background: var(--bg-input);
}
.drop-zone.active {
  border-color: var(--primary);
  background: rgba(37, 99, 235, 0.06);
}
.drop-zone-text {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  font-size: 13px;
  color: var(--text-secondary);
}
.drop-zone-hint {
  margin-top: 8px;
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
.attachment-empty {
  padding: 24px 0;
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
