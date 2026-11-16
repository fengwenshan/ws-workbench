<script setup lang="ts">
import { computed } from 'vue'
import type { ChatMessage, DroppedFileInput, PendingOffer, SendProgress } from '@/api/lanshare'
import type { MergedPeer } from './types'
import LanAvatar from './LanAvatar.vue'
import LanChatMessageList from './LanChatMessageList.vue'
import LanRemarkEditor from './LanRemarkEditor.vue'
import { useLanShareFileDrop } from './useLanShareFileDrop'

/**
 * 右栏聊天壳：头部设备信息 + 消息流 + 输入区（Glass Deck 视觉，刷新 §2）。
 * 头部无下边框，输入区是浮在底部的白色圆角卡；消息流拆在 LanChatMessageList。
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
  /** 粘贴导入的剪贴板文件路径（父组件走 send_files） */
  'send-paths': [paths: string[]]
  /** 拖拽进来的文件内容（父组件走 send_dropped_files） */
  'send-dropped-files': [files: DroppedFileInput[]]
}>()

/** 头部第二行：备注时 `原名 · IP`，无备注只 IP；不显示在线文字，无 IP 时整行隐藏。 */
const peerSubtitle = computed(() => {
  const target = props.peer
  if (!target) return ''
  const ip = target.peer?.ip ?? ''
  if (!ip) return target.remark ? target.alias : ''
  return target.remark ? `${target.alias} · ${ip}` : ip
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

// 粘贴 / 拖拽发送（剪贴板文件、截图、拖拽文件）拆在 useLanShareFileDrop 里，
// 组件只负责把事件接上并转成事件抛给父组件（发送动作统一在 useLanShareActions）。
const { dragging, onPaste, onDragEnter, onDragOver, onDragLeave, onDrop } = useLanShareFileDrop({
  peer: () => props.peer,
  sendingFiles: () => props.sendingFiles,
  onSendPaths: (paths) => emit('send-paths', paths),
  onSendDroppedFiles: (files) => emit('send-dropped-files', files),
})
</script>

<template>
  <section
    class="chat-pane"
    :class="{ 'is-dragging': dragging }"
    @dragenter.prevent="onDragEnter"
    @dragover.prevent="onDragOver"
    @dragleave="onDragLeave"
    @drop.prevent="onDrop"
  >
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
            <!-- 第二行：有备注带原名，没备注只显示 IP（不显示在线状态，左栏已有） -->
            <span v-if="peerSubtitle" class="head-sub" :class="{ 'is-offline': !peer.online }">{{ peerSubtitle }}</span>
          </div>
        </div>
        <div class="head-actions">
          <button
            class="head-action"
            type="button"
            :disabled="!peer.online || sendingFiles"
            @click="emit('send-files')"
          >
            {{ sendingFiles ? '选择文件…' : '发送文件' }}
          </button>
          <button
            v-if="peer.hasConversation"
            class="head-action is-danger"
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
        <div class="input-card">
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
            @paste="onPaste"
          ></textarea>
          <!-- 发送中禁用「发送」按钮 / Enter，但输入框保留可编辑，方便连续起草下一条 -->
            <button class="btn btn-primary send-btn" type="button" :disabled="!canSend" @click="submit">
              {{ sending ? '发送中…' : '发送' }}
            </button>
          </div>
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

    <!-- 拖拽文件进聊天区：高亮提示松开即发送（目录 / 超限文件由 onDrop 跳过并提示） -->
    <div v-if="dragging && peer" class="drop-mask">
      <div class="drop-mask-inner">
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M12 16V4" />
          <path d="M6 10l6-6 6 6" />
          <path d="M4 20h16" />
        </svg>
        <span>松开即可发送到当前设备</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.chat-pane {
  flex: 1;
  min-height: 0;
  position: relative;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.drop-mask {
  position: absolute;
  inset: 10px;
  z-index: 20;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 2px dashed var(--primary);
  border-radius: 14px;
  background: rgba(59, 130, 246, 0.08);
  pointer-events: none;
}
.drop-mask-inner {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 10px 16px;
  border-radius: 999px;
  background: var(--bg-primary);
  box-shadow: 0 18px 45px -24px rgba(15, 23, 42, 0.35);
  color: var(--primary);
  font-size: 13px;
  font-weight: 600;
}
.drop-mask-inner svg {
  width: 16px;
  height: 16px;
}

.chat-head {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 20px 10px;
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
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  min-width: 0;
}
.head-sub {
  max-width: 100%;
  padding-left: 2px;
  font-size: 12px;
  line-height: 1.4;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.head-sub.is-offline {
  opacity: 0.8;
}
.head-actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 4px;
}

.head-action {
  padding: 6px 9px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.5;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.16s, color 0.16s;
}
.head-action:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--primary);
}
.head-action.is-danger:hover:not(:disabled) {
  background: rgba(239, 68, 68, 0.08);
  color: var(--danger);
}
.head-action:disabled {
  color: var(--text-muted);
  opacity: 0.6;
  cursor: not-allowed;
}

.chat-input {
  flex-shrink: 0;
  padding: 8px 20px 18px;
  background: transparent;
}
.input-offline {
  margin: 0 0 8px;
  padding: 6px 12px;
  border-radius: 10px;
  background: rgba(255, 255, 255, 0.78);
  color: var(--text-muted);
  font-size: 12px;
}
.input-card {
  padding: 10px;
  border-radius: 14px;
  background: #fff;
  box-shadow:
    0 1px 2px rgba(15, 23, 42, 0.04),
    0 18px 45px -24px rgba(15, 23, 42, 0.35);
}
.input-row {
  display: flex;
  align-items: flex-end;
  gap: 8px;
}
.attach-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  flex-shrink: 0;
  padding: 0;
  border: none;
  border-radius: 9px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: background 0.16s, color 0.16s;
}
.attach-btn:hover:not(:disabled) {
  background: var(--bg-hover);
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
  padding: 7px 8px;
  border: 1px solid transparent;
  border-radius: 10px;
  background: transparent;
  color: var(--text-primary);
  font-size: 13px;
  font-family: inherit;
  line-height: 1.5;
  resize: none;
  outline: none;
  transition: background 0.16s;
}
.chat-textarea:focus {
  background: var(--bg-secondary);
}
.chat-textarea:disabled {
  color: var(--text-muted);
  cursor: not-allowed;
}
.send-btn {
  flex-shrink: 0;
  height: 34px;
  padding: 0 18px;
  font-size: 13px;
  border-radius: 10px;
}

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
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-secondary);
}
.empty-hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}
@media (max-width: 760px) {
  .chat-head {
    padding: 10px 14px 6px;
    flex-wrap: wrap;
  }
  .head-actions {
    margin-left: auto;
  }
  .chat-input {
    padding: 6px 14px 14px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .head-action,
  .attach-btn,
  .chat-textarea {
    transition: none;
  }
}
</style>
