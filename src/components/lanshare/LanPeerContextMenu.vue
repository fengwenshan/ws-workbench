<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import type { MergedPeer } from './types'

/**
 * 左栏列表项的右键菜单（设计 §3.2）。
 *
 * 用 fixed 定位跟随鼠标，Teleport 到 body 避免被左栏 overflow 裁掉；
 * 打开后先按可视区域收一次边，再监听「点外部 / Esc / 滚动 / 改变窗口大小」关闭。
 * 三个入口：删除聊天记录（有会话才显示）、删除附件（查数量期间置灰、数量 0 置灰）、
 * 免打扰模式（勾选态 = 当前免打扰开启）。
 */
const props = defineProps<{
  /** 右键命中的设备 */
  item: MergedPeer
  /** 鼠标位置（clientX / clientY） */
  x: number
  y: number
  /** 会话里本机收到的文件数量；null = 还在查询 */
  attachmentCount: number | null
  /** 是否正在查询附件数量（「删除附件」置灰） */
  attachmentLoading: boolean
  /** 当前是否开启免打扰（勾选态，与设置页开关同步） */
  dnd: boolean
}>()

const emit = defineEmits<{
  'delete-chat': []
  'delete-attachments': []
  'toggle-dnd': []
  close: []
}>()

const menuRef = ref<HTMLElement | null>(null)
const left = ref(props.x)
const top = ref(props.y)

const menuStyle = computed(() => ({ left: `${left.value}px`, top: `${top.value}px` }))

const displayName = computed(() => props.item.remark || props.item.alias)
const attachmentText = computed(() => {
  if (props.attachmentLoading) return '…'
  return String(props.attachmentCount ?? 0)
})
const attachmentsDisabled = computed(
  () => props.attachmentLoading || (props.attachmentCount ?? 0) <= 0,
)

/** 菜单贴边时往回收，避免超出窗口 */
async function clampToViewport() {
  await nextTick()
  const el = menuRef.value
  if (!el) return
  const margin = 8
  const rect = el.getBoundingClientRect()
  const maxLeft = window.innerWidth - rect.width - margin
  const maxTop = window.innerHeight - rect.height - margin
  left.value = Math.max(margin, Math.min(props.x, maxLeft))
  top.value = Math.max(margin, Math.min(props.y, maxTop))
}

function close() {
  emit('close')
}

function onDeleteChat() {
  emit('delete-chat')
  close()
}

function onDeleteAttachments() {
  if (attachmentsDisabled.value) return
  emit('delete-attachments')
  close()
}

function onToggleDnd() {
  emit('toggle-dnd')
  close()
}

function onPointerDown(event: PointerEvent) {
  if (menuRef.value?.contains(event.target as Node)) return
  close()
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') close()
}

// 同一个实例会被复用来打开另一台设备的菜单：位置 / 贴边都要按新坐标重算
watch(
  () => [props.x, props.y],
  () => {
    left.value = props.x
    top.value = props.y
    void clampToViewport()
  },
)

onMounted(() => {
  void clampToViewport()
  document.addEventListener('pointerdown', onPointerDown, true)
  document.addEventListener('keydown', onKeydown, true)
  window.addEventListener('scroll', close, true)
  window.addEventListener('resize', close)
  window.addEventListener('blur', close)
})

onUnmounted(() => {
  document.removeEventListener('pointerdown', onPointerDown, true)
  document.removeEventListener('keydown', onKeydown, true)
  window.removeEventListener('scroll', close, true)
  window.removeEventListener('resize', close)
  window.removeEventListener('blur', close)
})
</script>

<template>
  <Teleport to="body">
    <div ref="menuRef" class="ctx-menu" :style="menuStyle" role="menu" @contextmenu.prevent>
      <p class="ctx-title">{{ displayName }}</p>

      <button
        v-if="item.hasConversation"
        class="ctx-item"
        type="button"
        role="menuitem"
        @click="onDeleteChat"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M3 6h18" />
          <path d="M8 6V4h8v2" />
          <path d="M19 6l-1 14H6L5 6" />
          <path d="M10 11v5M14 11v5" />
        </svg>
        <span class="ctx-label">删除聊天记录</span>
      </button>

      <button
        v-if="item.hasConversation"
        class="ctx-item"
        type="button"
        role="menuitem"
        :disabled="attachmentsDisabled"
        @click="onDeleteAttachments"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M21 8.5l-8.4 8.4a5 5 0 0 1-7-7L14 1.4a3.3 3.3 0 0 1 4.7 4.7l-8.4 8.4a1.7 1.7 0 0 1-2.3-2.3l7.7-7.7" />
        </svg>
        <span class="ctx-label">删除附件</span>
        <span class="ctx-hint">{{ attachmentText }}</span>
      </button>

      <div v-if="item.hasConversation" class="ctx-divider"></div>

      <button class="ctx-item" type="button" role="menuitem" @click="onToggleDnd">
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M11 5L6 9H2v6h4l5 4V5z" />
          <path d="M22 9l-6 6M16 9l6 6" />
        </svg>
        <span class="ctx-label">免打扰模式</span>
        <svg
          v-if="dnd"
          class="ctx-check"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M20 6L9 17l-5-5" />
        </svg>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.ctx-menu {
  position: fixed;
  z-index: 1050;
  min-width: 180px;
  padding: 4px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  box-shadow: 0 8px 24px rgba(15, 23, 42, 0.16);
  user-select: none;
}

.ctx-title {
  margin: 2px 8px 4px;
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ctx-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 8px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary);
  font-size: 13px;
  line-height: 20px;
  text-align: left;
  cursor: pointer;
}

.ctx-item:hover:not(:disabled) {
  background: var(--bg-hover);
}

.ctx-item:disabled {
  color: var(--text-muted);
  cursor: default;
}

.ctx-item svg {
  width: 15px;
  height: 15px;
  flex-shrink: 0;
  color: var(--text-secondary);
}

.ctx-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ctx-hint {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-muted);
}

.ctx-item .ctx-check {
  flex-shrink: 0;
  color: var(--primary);
}

.ctx-divider {
  height: 1px;
  margin: 4px 2px;
  background: var(--border-color);
}
</style>
