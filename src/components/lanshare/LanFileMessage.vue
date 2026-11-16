<script setup lang="ts">
import { message as antMessage } from 'ant-design-vue'
import type { ChatMessage, ChatStatus, SendProgress } from '@/api/lanshare'
import { openFile } from '@/api/system'
import { formatBytes } from '@/components/cleaner/format'

/**
 * 聊天里的文件消息卡片：文件名 / 大小 / 状态 / 发送中进度 / 失败原因与重试。
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

const STATUS_TEXT: Record<ChatStatus, string> = {
  sending: '发送中',
  sent: '已发送',
  failed: '发送失败',
  received: '已接收',
}

function statusLabel(msg: ChatMessage): string {
  return STATUS_TEXT[msg.status] ?? msg.status
}

function percentOf(p: SendProgress | null): number {
  if (!p) return 0
  if (p.total_bytes <= 0) return p.done && !p.error ? 100 : 0
  return Math.min(100, Math.max(0, Math.floor((p.sent_bytes / p.total_bytes) * 100)))
}

function progressLabel(p: SendProgress | null): string {
  if (!p) return '等待开始…'
  if (p.error) return p.error
  if (p.done) return '已完成'
  return `${formatBytes(p.sent_bytes)} / ${formatBytes(p.total_bytes)} · ${percentOf(p)}%`
}

async function openReceivedFile() {
  const path = props.message.file_path
  if (!path) return
  try {
    await openFile(path)
  } catch (err) {
    const text = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err)
    antMessage.error(`打开文件失败：${text}`)
  }
}
</script>

<template>
  <div class="file-card" :class="{ 'is-failed': message.status === 'failed' }">
    <div class="file-card-head">
      <span class="file-icon">
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
          <path d="M14 3v5h5" />
        </svg>
      </span>
      <span class="file-meta">
        <span class="file-name" :title="message.file_name || ''">
          {{ message.file_name || '未命名文件' }}
        </span>
        <span class="file-sub">
          {{ formatBytes(message.file_size ?? 0) }}
          <template v-if="(message.file_total ?? 1) > 1">
            · 第 {{ (message.file_index ?? 0) + 1 }}/{{ message.file_total }} 个
          </template>
        </span>
      </span>
      <span class="file-status" :class="`status-${message.status}`">
        {{ statusLabel(message) }}
      </span>
    </div>

    <!-- 发送中的进度条：进度事件按 transfer_id + file_index 合并进来 -->
    <div v-if="message.direction === 'out' && message.status === 'sending'" class="file-progress">
      <div class="progress">
        <div class="progress-fill" :style="{ width: percentOf(progress) + '%' }"></div>
      </div>
      <span class="progress-label">{{ progressLabel(progress) }}</span>
    </div>

    <div v-if="message.status === 'failed'" class="file-error">
      <span class="error-text" :title="message.error || ''">
        {{ message.error || '发送失败' }}
      </span>
      <button class="text-btn" type="button" @click="emit('retry', message)">重试</button>
    </div>
    <div
      v-else-if="message.direction === 'in' && message.status === 'received' && message.file_path"
      class="file-actions"
    >
      <button class="text-btn" type="button" @click="openReceivedFile">打开文件</button>
    </div>
  </div>
</template>

<style scoped>
.file-card {
  width: 300px;
  max-width: 70%;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-primary);
}
.file-card.is-failed {
  border-color: rgba(239, 68, 68, 0.4);
}
.file-card-head {
  display: flex;
  align-items: flex-start;
  gap: 10px;
}
.file-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  background: var(--bg-active);
  color: var(--primary);
  flex-shrink: 0;
}
.file-icon svg {
  width: 18px;
  height: 18px;
}
.file-meta {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
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
  font-size: 11px;
  color: var(--text-muted);
}
.file-status {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-muted);
}
.file-status.status-sent,
.file-status.status-received {
  color: var(--success);
}
.file-status.status-failed {
  color: var(--danger);
}
.file-status.status-sending {
  color: var(--primary);
}

.file-progress {
  margin-top: 9px;
}
.progress {
  height: 6px;
  border-radius: 3px;
  background: var(--bg-readonly);
  overflow: hidden;
}
.progress-fill {
  height: 100%;
  border-radius: 3px;
  background: var(--primary);
  transition: width 0.25s ease;
}
.progress-label {
  display: block;
  margin-top: 4px;
  font-size: 11px;
  color: var(--text-secondary);
}
.file-error {
  margin-top: 8px;
  display: flex;
  align-items: center;
  gap: 8px;
}
.error-text {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  color: var(--danger);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.file-actions {
  margin-top: 8px;
  display: flex;
  justify-content: flex-end;
}
.text-btn {
  padding: 2px 6px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  line-height: 1.5;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.15s, color 0.15s;
}
.text-btn:hover:not(:disabled) {
  background: var(--bg-hover);
}
</style>
