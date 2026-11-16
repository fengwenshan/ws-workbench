<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import type { ChatMessage, PendingOffer, SendProgress } from '@/api/lanshare'
import { formatClock, formatDayLabel, startOfDay } from './format'
import LanAvatar from './LanAvatar.vue'
import LanFileMessage from './LanFileMessage.vue'
import LanOfferCard from './LanOfferCard.vue'

/**
 * 聊天消息流（设计 §2.1，钉钉观感）：日期分隔 + 「头像 + 内容」消息行 + 待接收卡。
 *
 * 消息行：对方在左（头像左、内容右），自己在右（内容左、头像右）；
 * 内容上方是昵称行（对方 = 备注 || 设备名，自己 = 「我」）+ HH:mm。
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
  /** 对方显示名（备注 || 设备名），用于消息行昵称 */
  peerName: string
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
        class="msg-row"
        :class="row.message.direction === 'out' ? 'is-out' : 'is-in'"
      >
        <!-- 头像：自己人形、对方显示器；自己在右，对方在左 -->
        <LanAvatar class="msg-avatar" :self="row.message.direction === 'out'" />

        <div class="msg-body">
          <!-- 昵称行：对方备注||设备名 / 自己「我」 + HH:mm -->
          <div class="msg-head">
            <span class="msg-name">
              {{ row.message.direction === 'out' ? '我' : peerName }}
            </span>
            <span class="msg-time">{{ formatClock(row.message.created_at_ms) }}</span>
          </div>

          <!-- 文本气泡 -->
          <div v-if="row.message.kind === 'text'" class="bubble">{{ row.message.text }}</div>
          <LanFileMessage
            v-else
            :message="row.message"
            :progress="row.progress"
            @retry="emit('retry', $event)"
          />

          <!-- 文本消息失败：气泡下方红字原因 + 重试（文件卡自己有一份） -->
          <div
            v-if="row.message.kind === 'text' && row.message.status === 'failed'"
            class="msg-error-row"
          >
            <span class="msg-error" :title="row.message.error || ''">
              {{ row.message.error || '发送失败' }}
            </span>
            <button class="text-btn" type="button" @click="emit('retry', row.message)">
              重试
            </button>
          </div>
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
/* 消息流：可滚动区域，消息行靠两侧 */
.chat-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 14px 18px 18px;
  display: flex;
  flex-direction: column;
  gap: 14px;
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

/* 消息行：「头像 + 内容」两栏；自己在右用 row-reverse */
.msg-row {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  max-width: 100%;
}
.msg-row.is-out {
  flex-direction: row-reverse;
}
.msg-avatar {
  margin-top: 2px;
}
.msg-body {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  min-width: 0;
  max-width: min(70%, 560px);
}
.msg-row.is-out .msg-body {
  align-items: flex-end;
}

/* 昵称行：昵称在左、时间在右；自己整行靠右 */
.msg-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  max-width: 100%;
  padding: 0 2px;
}
.msg-row.is-out .msg-head {
  justify-content: flex-end;
}
.msg-name {
  max-width: 220px;
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.msg-time {
  font-size: 11px;
  color: var(--text-muted);
  white-space: nowrap;
}

/* 文本气泡：对方白底细边框、自己淡蓝底；靠头像一侧上角略小 */
.bubble {
  max-width: 100%;
  padding: 8px 12px;
  border-radius: 12px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-primary);
  word-break: break-word;
  white-space: pre-wrap;
}
.msg-row.is-in .bubble {
  background: var(--bg-primary);
  border: 1px solid var(--border-color);
  border-top-left-radius: 4px;
}
.msg-row.is-out .bubble {
  background: rgba(59, 130, 246, 0.08);
  border-top-right-radius: 4px;
}

/* 文本发送失败：气泡下方红字 + 重试 */
.msg-error-row {
  display: flex;
  align-items: center;
  gap: 8px;
  max-width: 100%;
  padding: 0 2px;
}
.msg-error {
  max-width: 320px;
  color: var(--danger);
  font-size: 11px;
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
