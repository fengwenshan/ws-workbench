<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import type { ChatMessage, PendingOffer, SendProgress } from '@/api/lanshare'
import { formatClock, formatDayLabel, startOfDay } from './format'
import LanFileMessage from './LanFileMessage.vue'
import LanOfferCard from './LanOfferCard.vue'

/**
 * 聊天消息流：日期分隔、文本气泡、文件消息卡片、待接收请求卡片。
 *
 * 消息按 created_at_ms 升序渲染（后端已排序，这里再按顺序分日期段），
 * 切会话 / 新消息 / 新请求时自动滚到底部，保证打开就在最新一条。
 */
const props = defineProps<{
  /** 当前会话消息（时间正序） */
  messages: ChatMessage[]
  /** 发给当前设备的待接收请求 */
  offers: PendingOffer[]
  /** 文件发送进度：key = ${transfer_id}:${file_index} */
  progress: Record<string, SendProgress>
  /** 正在处理中的 offer id */
  offerBusy: string | null
  /** 1 秒一跳的时间基准（日期分隔 / 待接收倒计时） */
  now: number
  /** 当前会话 id：切会话要滚到底部 */
  peerId: string | null
}>()

const emit = defineEmits<{
  retry: [message: ChatMessage]
  'accept-offer': [offer: PendingOffer]
  'reject-offer': [offer: PendingOffer]
}>()

const scrollRef = ref<HTMLElement | null>(null)

interface ChatRow {
  key: string
  /** 非空表示这是一行日期分隔 */
  dateLabel: string | null
  message: ChatMessage | null
  /** 文件消息对应的实时进度（没有则为 null） */
  progress: SendProgress | null
}

function progressFor(msg: ChatMessage): SendProgress | null {
  if (!msg.transfer_id || msg.file_index === null || msg.file_index === undefined) return null
  return props.progress[`${msg.transfer_id}:${msg.file_index}`] ?? null
}

/** 把消息与日期分隔合并成渲染行；进度在这里一并查好，模板里不再重复找 */
const rows = computed<ChatRow[]>(() => {
  const result: ChatRow[] = []
  let lastDay = ''
  for (const msg of props.messages) {
    const day = String(startOfDay(msg.created_at_ms))
    if (day !== lastDay) {
      result.push({
        key: `date-${day}`,
        dateLabel: formatDayLabel(msg.created_at_ms, props.now),
        message: null,
        progress: null,
      })
      lastDay = day
    }
    result.push({ key: msg.id, dateLabel: null, message: msg, progress: progressFor(msg) })
  }
  return result
})

function scrollToBottom() {
  const el = scrollRef.value
  if (!el) return
  el.scrollTop = el.scrollHeight
}

// 切会话 / 来新消息 / 出现待接收卡片：滚到底部
watch(
  () => [props.peerId, props.messages.length, props.offers.length],
  () => {
    void nextTick(scrollToBottom)
  },
)
</script>

<template>
  <div ref="scrollRef" class="chat-scroll">
    <div v-if="!rows.length && !offers.length" class="chat-hint">
      <p class="chat-hint-title">还没有消息</p>
      <p class="chat-hint-text">发条消息或传个文件，记录会留在这个会话里。</p>
    </div>

    <template v-for="row in rows" :key="row.key">
      <!-- 日期分隔 -->
      <div v-if="row.dateLabel" class="date-sep">
        <span>{{ row.dateLabel }}</span>
      </div>

      <div
        v-else-if="row.message"
        class="msg"
        :class="row.message.direction === 'out' ? 'is-out' : 'is-in'"
      >
        <!-- 文本气泡 -->
        <div v-if="row.message.kind === 'text'" class="bubble">{{ row.message.text }}</div>
        <LanFileMessage
          v-else
          :message="row.message"
          :progress="row.progress"
          @retry="emit('retry', $event)"
        />

        <div class="msg-meta">
          <span class="msg-time">{{ formatClock(row.message.created_at_ms) }}</span>
          <!-- 文本消息失败：气泡下面给原因与重试（文件卡自己有一份） -->
          <template v-if="row.message.kind === 'text' && row.message.status === 'failed'">
            <span class="msg-error" :title="row.message.error || ''">
              {{ row.message.error || '发送失败' }}
            </span>
            <button class="text-btn" type="button" @click="emit('retry', row.message)">
              重试
            </button>
          </template>
        </div>
      </div>
    </template>

    <!-- 待接收请求：同一个聊天里展示，接受后落盘并生成 received 文件消息 -->
    <LanOfferCard
      v-for="offer in offers"
      :key="offer.id"
      :offer="offer"
      :busy-id="offerBusy"
      :now="now"
      @accept="emit('accept-offer', $event)"
      @reject="emit('reject-offer', $event)"
    />
  </div>
</template>

<style scoped>
/* 消息流：可滚动区域，气泡靠两侧 */
.chat-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 14px 18px 18px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  background: var(--bg-secondary);
}

.date-sep {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 4px 0;
}
.date-sep span {
  padding: 2px 10px;
  border-radius: 10px;
  background: var(--bg-tag);
  color: var(--text-secondary);
  font-size: 11px;
}

.msg {
  display: flex;
  flex-direction: column;
  gap: 3px;
  max-width: 100%;
}
.msg.is-out {
  align-items: flex-end;
}
.msg.is-in {
  align-items: flex-start;
}
.bubble {
  max-width: 70%;
  padding: 8px 12px;
  border-radius: 10px;
  font-size: 13px;
  line-height: 1.6;
  word-break: break-word;
  white-space: pre-wrap;
}
.msg.is-in .bubble {
  background: var(--bg-primary);
  border: 1px solid var(--border-color);
  color: var(--text-primary);
  border-top-left-radius: 3px;
}
.msg.is-out .bubble {
  background: var(--primary);
  color: #fff;
  border-top-right-radius: 3px;
}

/* 文件卡片：自己发的用淡蓝底和气泡方向区分 */
.msg.is-out .file-card {
  border-color: rgba(59, 130, 246, 0.45);
  background: var(--bg-active);
}

.msg-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11px;
  color: var(--text-muted);
  padding: 0 4px;
}
.msg-error {
  max-width: 320px;
  color: var(--danger);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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

/* 会话内空态 */
.chat-hint {
  margin: auto;
  padding: 24px 0;
  text-align: center;
}
.chat-hint-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-secondary);
}
.chat-hint-text {
  margin-top: 6px;
  font-size: 12px;
  color: var(--text-muted);
}
</style>
