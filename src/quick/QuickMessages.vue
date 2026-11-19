<script setup lang="ts">
import type { LanShareConversation } from '@/api/lanshare'
import { formatListTime } from '@/components/lanshare/format'

/**
 * 快捷面板「消息」页签的列表（设计 §7.3）。
 *
 * 只做展示：会话列表、备注、时间基准和空态 / 错误文案都由 QuickPanel 拉取后传入，
 * 点击某项抛 `open` 事件（父组件调 quick_panel_open_chat 让主窗口定位）。
 */
const props = defineProps<{
  conversations: LanShareConversation[]
  /** 本机备注：fingerprint -> 备注（备注优先于设备名） */
  remarks: Record<string, string>
  /** 时间基准（面板打开 / 刷新时更新，够 formatListTime 判断今天/昨天） */
  now: number
  loading: boolean
  error: string
}>()

const emit = defineEmits<{
  open: [peerId: string]
}>()

function displayName(item: LanShareConversation): string {
  const remark = props.remarks[item.id]?.trim()
  return remark || item.alias || '未知设备'
}
</script>

<template>
  <div class="qp-messages">
    <p v-if="error" class="msg-error">{{ error }}</p>

    <div v-if="conversations.length" class="msg-list">
      <button
        v-for="item in conversations"
        :key="item.id"
        class="msg-item"
        type="button"
        @click="emit('open', item.id)"
      >
        <span class="msg-main">
          <span class="msg-top">
            <span class="msg-name">{{ displayName(item) }}</span>
            <span class="msg-time">{{ formatListTime(item.last_activity_ms, now) }}</span>
          </span>
          <span class="msg-preview" :class="{ 'is-muted': !item.last_preview }">
            {{ item.last_preview || '暂无消息' }}
          </span>
        </span>
        <span v-if="item.unread > 0" class="msg-badge">
          {{ item.unread > 99 ? '99+' : item.unread }}
        </span>
      </button>
    </div>

    <p v-else class="msg-empty">
      {{ loading ? '正在读取…' : '暂无聊天记录' }}
    </p>
  </div>
</template>

<style scoped>
.qp-messages {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.msg-error {
  margin: 0;
  padding: 8px 12px;
  border-radius: var(--radius-md);
  background: rgba(239, 68, 68, 0.08);
  color: var(--danger);
  font-size: 12px;
  line-height: 1.5;
}

.msg-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  /* 会话多时内部滚动，面板高度按内容量到上限为止 */
  max-height: 320px;
  overflow-y: auto;
}

.msg-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 8px;
  border: 0;
  border-radius: var(--radius-md);
  background: transparent;
  text-align: left;
  cursor: pointer;
}

.msg-item:hover {
  background: var(--bg-hover);
}

.msg-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.msg-top {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.msg-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 600;
}

.msg-time {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 12px;
}

.msg-preview {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-secondary);
  font-size: 12px;
}

.msg-preview.is-muted {
  color: var(--text-muted);
}

.msg-badge {
  flex-shrink: 0;
  min-width: 17px;
  height: 17px;
  padding: 0 4px;
  border-radius: var(--radius-md);
  background: var(--danger);
  color: var(--on-primary);
  font-size: 12px;
  line-height: 17px;
  text-align: center;
}

.msg-empty {
  margin: 0;
  padding: 24px 12px;
  color: var(--text-muted);
  font-size: 12px;
  text-align: center;
}
</style>
