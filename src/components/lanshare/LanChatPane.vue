<script setup lang="ts">
import { computed } from 'vue'
import type { ChatMessage, PendingOffer, SendProgress } from '@/api/lanshare'
import type { MergedPeer } from './types'
import LanAvatar from './LanAvatar.vue'
import LanChatMessageList from './LanChatMessageList.vue'
import LanRemarkEditor from './LanRemarkEditor.vue'

/**
 * 右栏聊天壳：头部设备信息 + 消息流 + 输入区。
 *
 * 头部按钉钉观感放头像 + 显示名（备注优先于设备名），名字旁的铅笔就地起备注
 * （LanRemarkEditor，保存仍走父组件统一调 API）。
 * 消息流拆在 LanChatMessageList 里；这里只做输入框交互与把事件转给 LanSharePanel。
 */
const props = defineProps<{
  /** 当前选中的设备（null 显示空态） */
  peer: MergedPeer | null
  /** 当前会话消息（时间正序） */
  messages: ChatMessage[]
  /** 发给当前设备的待接收请求 */
  offers: PendingOffer[]
  /** 文件发送进度：key = ${transfer_id}:${file_index} */
  progress: Record<string, SendProgress>
  /** 1 秒一跳的时间基准（日期分隔 / 待接收倒计时） */
  now: number
  /** 文本正在发送（发送按钮禁用，输入框仍可继续起草） */
  sending: boolean
  /** 正在选文件 / 发文件 */
  sendingFiles: boolean
  /** 正在处理中的 offer id */
  offerBusy: string | null
  /** 正在删除当前会话 */
  deleting: boolean
  /** 输入框草稿（壳里持有，发送成功才清空） */
  draft: string
}>()

const emit = defineEmits<{
  'update:draft': [value: string]
  'send-text': [text: string]
  'send-files': []
  retry: [message: ChatMessage]
  'accept-offer': [offer: PendingOffer]
  'reject-offer': [offer: PendingOffer]
  'delete-conversation': []
  /** 保存本机备注（空串 = 删除备注） */
  'save-remark': [peerId: string, remark: string]
}>()

const peerAddress = computed(() => {
  const runtime = props.peer?.peer
  if (!runtime) return '设备不在线'
  return `${runtime.ip}:${runtime.port}`
})

/** 显示名：本机备注优先，没备注才用设备名（设计 §2.3） */
const displayName = computed(() => {
  const target = props.peer
  if (!target) return ''
  return target.remark || target.alias
})

const textareaPlaceholder = computed(() => {
  if (!props.peer?.online) return '设备已离线'
  return '输入消息，Enter 发送，Shift+Enter 换行'
})

const canSend = computed(
  () => !!props.peer?.online && !props.sending && props.draft.trim().length > 0,
)

function submit() {
  if (!canSend.value) return
  emit('send-text', props.draft.trim())
}

function onInput(event: Event) {
  emit('update:draft', (event.target as HTMLTextAreaElement).value)
}

function onKeydown(event: KeyboardEvent) {
  if (event.key !== 'Enter' || event.shiftKey) return
  // 中文输入法用 Enter 确认候选词时也会触发 keydown，不能当成发送
  if (event.isComposing || event.keyCode === 229) return
  event.preventDefault()
  submit()
}
</script>

<template>
  <section class="chat-pane">
    <template v-if="peer">
      <header class="chat-head">
        <div class="head-main">
          <LanAvatar class="head-avatar" :self="false" />
          <div class="head-title">
            <!-- 显示名（备注 || 设备名）+ 铅笔就地编辑备注 -->
            <LanRemarkEditor
              :peer-id="peer.id"
              :alias="peer.alias"
              :remark="peer.remark"
              @save="emit('save-remark', peer.id, $event)"
            />
            <span class="online-label" :class="peer.online ? 'is-online' : 'is-offline'">
              <span class="online-dot"></span>
              {{ peer.online ? '在线' : '离线' }}
            </span>
          </div>
          <div class="head-addr mono" :title="peerAddress">{{ peerAddress }}</div>
        </div>
        <div class="head-actions">
          <button
            class="btn btn-secondary btn-xs"
            type="button"
            :disabled="!peer.online || sendingFiles"
            @click="emit('send-files')"
          >
            {{ sendingFiles ? '选择文件…' : '发送文件' }}
          </button>
          <button
            v-if="peer.hasConversation"
            class="btn btn-danger-ghost btn-xs"
            type="button"
            :disabled="deleting"
            @click="emit('delete-conversation')"
          >
            {{ deleting ? '删除中…' : '删除会话' }}
          </button>
        </div>
      </header>

      <LanChatMessageList
        :messages="messages"
        :offers="offers"
        :progress="progress"
        :offer-busy="offerBusy"
        :now="now"
        :peer-id="peer.id"
        :peer-name="displayName"
        @retry="emit('retry', $event)"
        @accept-offer="emit('accept-offer', $event)"
        @reject-offer="emit('reject-offer', $event)"
      />

      <footer class="chat-input">
        <p v-if="!peer.online" class="input-offline">设备已离线，无法发送消息或文件</p>
        <div class="input-row">
          <button
            class="attach-btn"
            type="button"
            title="选择文件发送"
            aria-label="选择文件发送"
            :disabled="!peer.online || sendingFiles"
            @click="emit('send-files')"
          >
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.7"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path
                d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"
              />
            </svg>
          </button>
          <textarea
            class="chat-textarea"
            :value="draft"
            :placeholder="textareaPlaceholder"
            :disabled="!peer.online"
            rows="2"
            @input="onInput"
            @keydown="onKeydown"
          ></textarea>
          <!-- 发送中禁用「发送」按钮 / Enter，但输入框保留可编辑，方便连续起草下一条 -->
          <button class="btn btn-primary send-btn" type="button" :disabled="!canSend" @click="submit">
            {{ sending ? '发送中…' : '发送' }}
          </button>
        </div>
      </footer>
    </template>

    <!-- 没选设备：右栏空态 -->
    <div v-else class="chat-empty-state">
      <svg
        class="empty-icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.4"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path
          d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8z"
        />
        <path d="M8 10h.01M12 10h.01M16 10h.01" />
      </svg>
      <p class="empty-title">从左侧选择一台设备开始聊天</p>
      <p class="empty-hint">可以发送文本消息和文件；在线设备会出现在列表前面。</p>
    </div>
  </section>
</template>

<style scoped>
.chat-pane {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* 头部：头像 / 设备名（可编辑备注）/ 在线状态 / IP:端口 + 操作按钮 */
.chat-head {
  flex-shrink: 0;
  height: var(--panel-header-height, 40px);
  padding: 0 16px;
  display: flex;
  align-items: center;
  gap: 12px;
  border-bottom: 1px solid var(--border-color);
}
.head-main {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 10px;
}
.head-avatar {
  flex-shrink: 0;
}
.head-title {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex-shrink: 1;
}
.online-label {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
  flex-shrink: 0;
}
.online-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-muted);
}
.online-label.is-online {
  color: var(--success);
}
.online-label.is-online .online-dot {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.16);
}
.head-addr {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.head-actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 8px;
}
.btn.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}

/* 输入区 */
.chat-input {
  flex-shrink: 0;
  padding: 10px 16px 14px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-primary);
}
.input-offline {
  margin-bottom: 8px;
  padding: 6px 10px;
  border-radius: 6px;
  background: var(--bg-readonly);
  color: var(--text-muted);
  font-size: 12px;
}
.input-row {
  display: flex;
  align-items: flex-end;
  gap: 10px;
}
.attach-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  flex-shrink: 0;
  padding: 0;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-input);
  color: var(--text-secondary);
  cursor: pointer;
  transition: border-color 0.15s, color 0.15s;
}
.attach-btn:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--primary);
}
.attach-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.attach-btn svg {
  width: 17px;
  height: 17px;
}
.chat-textarea {
  flex: 1;
  min-width: 0;
  min-height: 34px;
  max-height: 120px;
  padding: 7px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-input);
  color: var(--text-primary);
  font-size: 13px;
  font-family: inherit;
  line-height: 1.5;
  resize: none;
  outline: none;
}
.chat-textarea:focus {
  border-color: var(--primary);
}
.chat-textarea:disabled {
  background: var(--bg-readonly);
  color: var(--text-muted);
  cursor: not-allowed;
}
.send-btn {
  flex-shrink: 0;
  height: 34px;
  padding: 0 18px;
  font-size: 13px;
}

/* 无选中设备：右栏空态 */
.chat-empty-state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px;
  text-align: center;
}
.empty-icon {
  width: 64px;
  height: 64px;
  color: var(--text-muted);
  opacity: 0.55;
}
.empty-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-secondary);
}
.empty-hint {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}

/* 窄窗口：头部按钮只留图标会太挤，允许换行到名字下面 */
@media (max-width: 760px) {
  .chat-head {
    height: auto;
    min-height: var(--panel-header-height, 40px);
    padding: 6px 12px;
    flex-wrap: wrap;
  }
  .head-main {
    align-items: center;
  }
  .head-actions {
    margin-left: auto;
  }
  .head-addr {
    display: none;
  }
}
</style>
