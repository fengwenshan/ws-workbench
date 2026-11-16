<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import type { ChatMessage } from '@/api/lanshare'

/**
 * 消息右键菜单（补充需求 14 / 设计 §4）。
 *
 * 和左栏 LanPeerContextMenu 同一套交互：fixed 跟随鼠标、Teleport 到 body 避免被
 * 消息流 overflow 裁掉，打开先按可视区域收边，再监听「点外部 / Esc / 滚动 /
 * 改变窗口大小 / 失焦」关闭。只有「删除消息」一项，危险色。
 */
const props = defineProps<{
  /** 右键命中的消息 */
  message: ChatMessage
  /** 鼠标位置（clientX / clientY） */
  x: number
  y: number
}>()

const emit = defineEmits<{
  delete: []
  close: []
}>()

const menuRef = ref<HTMLElement | null>(null)
const left = ref(props.x)
const top = ref(props.y)

const menuStyle = computed(() => ({ left: `${left.value}px`, top: `${top.value}px` }))

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

function onDelete() {
  emit('delete')
  close()
}

function onPointerDown(event: PointerEvent) {
  if (menuRef.value?.contains(event.target as Node)) return
  close()
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') close()
}

// 同一个实例会被复用来打开另一条消息：位置 / 贴边按新坐标重算
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
      <button class="ctx-item is-danger" type="button" role="menuitem" @click="onDelete">
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
        <span class="ctx-label">删除消息</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.ctx-menu {
  position: fixed;
  z-index: 1050;
  min-width: 140px;
  padding: 4px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  box-shadow: 0 8px 24px rgba(15, 23, 42, 0.16);
  user-select: none;
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

.ctx-item:hover {
  background: rgba(239, 68, 68, 0.08);
}

/* 删除是破坏性操作：图标和文字都用危险色 */
.ctx-item.is-danger {
  color: var(--danger);
}

.ctx-item svg {
  width: 15px;
  height: 15px;
  flex-shrink: 0;
  color: inherit;
}

.ctx-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
