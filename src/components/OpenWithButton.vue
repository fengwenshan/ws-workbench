<script setup lang="ts">
/**
 * 「用其它应用打开」分体按钮。
 *
 * 左半边：固定用默认应用打开目录（访达 / 资源管理器），不随菜单选择变化；
 * 右半边：菜单每一项都是「用这个应用打开当前目录」的动作，点了立刻用该应用
 * 打开，不是切换器，也不记住上次选了谁。图标由后端从 .icns（macOS）或
 * exe（Windows）转成 PNG。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { message } from 'ant-design-vue'
import { listOpenWithApps, openWith, type OpenWithApp } from '@/api/system'
import type { LogLevel } from '@/types'

const props = defineProps<{ path: string }>()
const emit = defineEmits<{
  log: [level: LogLevel, message: string]
}>()

const apps = ref<OpenWithApp[]>([])
const loading = ref(true)
const busy = ref(false)
const open = ref(false)
const root = ref<HTMLElement | null>(null)

const hasPath = computed(() => props.path.trim().length > 0)

/** 左半边固定用列表第一个（后端把默认应用排在第一位） */
const current = computed<OpenWithApp | null>(() => apps.value[0] ?? null)

const mainTitle = computed(() => {
  if (!hasPath.value) return '还没有选择目录'
  const name = current.value?.name ?? '默认应用'
  return `用 ${name} 打开`
})

onMounted(async () => {
  try {
    apps.value = await listOpenWithApps()
  } catch (err) {
    emit('log', 'warn', `读取打开方式失败: ${err}`)
  } finally {
    loading.value = false
  }
  document.addEventListener('mousedown', onDocumentClick)
  document.addEventListener('keydown', onDocumentKeydown)
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocumentClick)
  document.removeEventListener('keydown', onDocumentKeydown)
})

function onDocumentClick(event: MouseEvent) {
  if (!open.value) return
  const target = event.target as Node | null
  if (root.value && target && !root.value.contains(target)) {
    open.value = false
  }
}

function onDocumentKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') open.value = false
}

function toggle() {
  if (loading.value || apps.value.length === 0) return
  open.value = !open.value
}

/** 用指定应用打开当前目录：纯动作，不改变按钮上显示的应用 */
async function run(app: OpenWithApp) {
  if (!hasPath.value || busy.value) return
  busy.value = true
  open.value = false
  try {
    await openWith(props.path, app.id)
    emit('log', 'info', `用 ${app.name} 打开: ${props.path}`)
  } catch (err) {
    // 失败时给个可见反馈，否则用户只会觉得「点了没反应」
    message.error(`用 ${app.name} 打开失败: ${err}`)
    emit('log', 'error', `用 ${app.name} 打开失败: ${err}`)
  } finally {
    busy.value = false
  }
}

function openCurrent() {
  if (current.value) run(current.value)
}

/** 取不到图标时的兜底字形 */
function glyph(app: OpenWithApp | null) {
  return app && app.name ? app.name.slice(0, 1).toUpperCase() : '?'
}
</script>

<template>
  <div ref="root" class="open-with">
    <div class="ow-split" :class="{ 'is-open': open, 'is-busy': busy }">
      <button
        type="button"
        class="ow-main"
        :disabled="!hasPath || busy || !current"
        :title="mainTitle"
        @click="openCurrent"
      >
        <img v-if="current?.icon" class="ow-icon" :src="current.icon" alt="" />
        <span v-else class="ow-glyph">{{ glyph(current) }}</span>
      </button>
      <button
        type="button"
        class="ow-arrow"
        :disabled="loading || apps.length === 0"
        aria-haspopup="menu"
        :aria-expanded="open"
        title="用其它应用打开"
        @click="toggle"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path
            d="M2 3.5 5 6.5 8 3.5"
            fill="none"
            stroke="currentColor"
            stroke-width="1.4"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </button>
    </div>

    <div v-if="open" class="ow-menu" role="menu">
      <button
        v-for="app in apps"
        :key="app.id"
        type="button"
        role="menuitem"
        class="ow-item"
        @click="run(app)"
      >
        <img v-if="app.icon" class="ow-icon" :src="app.icon" alt="" />
        <span v-else class="ow-glyph">{{ glyph(app) }}</span>
        <span class="ow-name">{{ app.name }}</span>
        <span v-if="app.is_default" class="ow-tag">（默认）</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.open-with {
  position: relative;
  flex-shrink: 0;
}

/* 分体按钮：左半边用当前应用，右半边展开菜单 */
.ow-split {
  display: flex;
  align-items: stretch;
  height: 36px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  overflow: hidden;
  transition: border-color 0.15s;
}
.ow-split:hover,
.ow-split.is-open {
  border-color: var(--primary);
}

.ow-main,
.ow-arrow {
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.15s;
}
.ow-main {
  width: 38px;
  flex-shrink: 0;
}
.ow-arrow {
  width: 22px;
  border-left: 1px solid var(--border-color);
}
.ow-main:hover:not(:disabled),
.ow-arrow:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--primary);
}
.ow-main:disabled,
.ow-arrow:disabled {
  cursor: default;
  opacity: 0.45;
}
.ow-split.is-busy {
  opacity: 0.7;
}

.ow-icon {
  width: 18px;
  height: 18px;
  object-fit: contain;
  border-radius: 3px;
}
.ow-glyph {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border-radius: 4px;
  background: #e5e7eb;
  color: #4b5563;
  font-size: 11px;
  font-weight: 600;
  line-height: 1;
}

/* 下拉菜单：右对齐到按钮，避免挡住「选择」 */
.ow-menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: 40;
  min-width: 200px;
  padding: 4px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-input);
  box-shadow: 0 10px 24px rgba(17, 24, 39, 0.12);
}
.ow-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 7px 8px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  font-size: 13px;
  text-align: left;
  white-space: nowrap;
}
.ow-item:hover {
  background: var(--bg-hover);
  color: var(--primary);
}
.ow-name {
  flex: 1;
  min-width: 0;
}
.ow-tag {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-secondary);
}
</style>
