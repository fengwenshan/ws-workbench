<script setup lang="ts">
import { computed } from 'vue'
import { message as antMessage } from 'ant-design-vue'
import {
  openLanShareFileDir,
  type ChatMessage,
  type SendProgress,
} from '@/api/lanshare'
import { openFile } from '@/api/system'
import { formatBytes } from '@/components/cleaner/format'

/**
 * 聊天里的文件消息卡片（设计 §2.2，钉钉观感）。
 *
 * 头部是大图标 + 文件名 + 大小；底部分隔线左边是状态（发送中 / 已发送 / 已保存 /
 * 发送失败 / 已删除），右边是胶囊按钮（打开 / 打开文件夹 / 重试）。
 * 局域网直传没有服务器，所以这里没有「查收 / 下载」这类云端语义：
 * 文件要么还在传（发送中），要么已经落盘（已发送 / 已保存）。
 *
 * 进度用 `transfer_id + file_index` 从父组件的 map 里取，
 * 消息状态（sending / sent / failed / received）仍以落库消息为准。
 */
const props = defineProps<{
  message: ChatMessage
  /** 对应的实时发送进度；还没收到进度事件时为 null */
  progress: SendProgress | null
}>()

const emit = defineEmits<{
  retry: [message: ChatMessage]
}>()

function errText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

const isSending = computed(
  () => props.message.direction === 'out' && props.message.status === 'sending',
)
const isFailed = computed(() => props.message.status === 'failed')
/** 本机收到的文件被「删除附件」清掉：文件卡只显示「已删除」，不给任何打开动作 */
const isDeleted = computed(() => props.message.status === 'deleted')
/** 已落盘的文件才给打开动作：发出成功的是本机源文件，接收成功的是保存结果 */
const hasPath = computed(() => !!props.message.file_path)
const canOpen = computed(
  () => hasPath.value && (props.message.status === 'sent' || props.message.status === 'received'),
)

const percent = computed(() => {
  const p = props.progress
  if (!p) return 0
  if (p.total_bytes <= 0) return p.done && !p.error ? 100 : 0
  return Math.min(100, Math.max(0, Math.floor((p.sent_bytes / p.total_bytes) * 100)))
})

const stateText = computed(() => {
  switch (props.message.status) {
    case 'deleted':
      return '已删除'
    case 'failed':
      // 接收失败的 in 消息后端目前不会写库；这里按方向兜底，别把接收失败写成「发送失败」
      return props.message.direction === 'in' ? '接收失败' : '发送失败'
    case 'received':
      return '已保存'
    case 'sending':
      // 有进度事件时带上百分比；文件刚建出来、还没开始传时只显示「发送中」
      return props.progress ? `发送中 ${percent.value}%` : '发送中'
    default:
      return '已发送'
  }
})

async function openFileAction() {
  const path = props.message.file_path
  if (!path) return
  try {
    await openFile(path)
  } catch (err) {
    antMessage.error(`打开文件失败：${errText(err)}`)
  }
}

async function openFolderAction() {
  const path = props.message.file_path
  if (!path) return
  try {
    await openLanShareFileDir(path)
  } catch (err) {
    antMessage.error(`打开文件夹失败：${errText(err)}`)
  }
}
</script>

<template>
  <div class="file-card" :class="{ 'is-failed': isFailed, 'is-deleted': isDeleted }">
    <div class="file-card-main">
      <span class="file-icon">
        <!-- 蓝色文档图标（大号） -->
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M14 3H8a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V9z" />
          <path d="M14 3v6h6" />
        </svg>
      </span>
      <div class="file-meta">
        <span class="file-name" :title="message.file_name || ''">
          {{ message.file_name || '未命名文件' }}
        </span>
        <span class="file-sub">
          {{ formatBytes(message.file_size ?? 0) }}
          <template v-if="(message.file_total ?? 1) > 1">
            · 第 {{ (message.file_index ?? 0) + 1 }}/{{ message.file_total }} 个
          </template>
        </span>
      </div>
    </div>

    <!-- 底部分隔线：左侧状态，右侧胶囊按钮 -->
    <div class="file-foot">
      <div class="file-state">
        <span
          class="state-text"
          :class="{ 'is-primary': isSending, 'is-danger': isFailed, 'is-muted': isDeleted }"
        >
          {{ stateText }}
        </span>
        <!-- 发送中：细进度条跟在状态下面；失败时把原因压成一行，完整内容在 title 里 -->
        <div v-if="isSending" class="progress">
          <div class="progress-fill" :style="{ width: percent + '%' }"></div>
        </div>
        <span v-else-if="isFailed" class="state-error" :title="message.error || ''">
          {{ message.error || '传输中断，请重试' }}
        </span>
      </div>
      <div class="file-actions">
        <template v-if="canOpen">
          <button class="pill-btn" type="button" @click="openFileAction">打开</button>
          <button class="pill-btn" type="button" @click="openFolderAction">打开文件夹</button>
        </template>
        <!-- 接收失败没有可重试的本地源文件；只有 out 失败才给重试 -->
        <button
          v-if="isFailed && message.direction === 'out'"
          class="pill-btn is-primary"
          type="button"
          title="重新发送"
          @click="emit('retry', message)"
        >
          重试
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 文件卡：白底 12px 圆角 + 阴影分层（与消息气泡同一套 elevation），小窗收到容器宽度内 */
.file-card {
  width: 320px;
  max-width: 100%;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  overflow: hidden;
}
.file-card.is-failed {
  border-color: rgba(239, 68, 68, 0.35);
}
/* 已删除：整卡降饱和，文件内容已经不在了，只留历史记录 */
.file-card.is-deleted {
  background: var(--bg-readonly);
}
.file-card.is-deleted .file-icon {
  background: var(--bg-tag);
  color: var(--text-muted);
}

.file-card-main {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 12px 12px;
}
.file-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  flex-shrink: 0;
  border-radius: var(--radius-md);
  background: rgba(59, 130, 246, 0.1);
  color: var(--primary);
}
.file-icon svg {
  width: 22px;
  height: 22px;
}
.file-meta {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.file-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.file-sub {
  font-size: 12px;
  color: var(--text-muted);
}

.file-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: var(--bg-secondary);
}
.file-state {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.state-text {
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.state-text.is-primary {
  color: var(--primary);
}
.state-text.is-danger {
  color: var(--danger);
}
.state-text.is-muted {
  color: var(--text-muted);
}
.state-error {
  font-size: 12px;
  color: var(--danger);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.progress {
  width: 120px;
  max-width: 100%;
  height: 3px;
  border-radius: var(--radius-sm);
  background: var(--bg-readonly);
  overflow: hidden;
}
.progress-fill {
  height: 100%;
  border-radius: var(--radius-sm);
  background: var(--primary);
  transition: width 0.25s ease;
}

.file-actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 8px;
}
.pill-btn {
  padding: 4px 12px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.6;
  white-space: nowrap;
  cursor: pointer;
  transition: color 0.15s, border-color 0.15s, background 0.15s;
}
.pill-btn:hover {
  border-color: var(--primary);
  color: var(--primary);
}
.pill-btn.is-primary {
  border-color: var(--primary);
  color: var(--primary);
}
</style>
