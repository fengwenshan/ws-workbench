<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import type { LogEntry } from '@/types'

const props = defineProps<{
  title: string
  status?: string
  logs: LogEntry[]
  emptyText?: string
}>()

const emit = defineEmits<{
  close: []
  clear: []
}>()

const LEVEL_LABELS: Record<string, string> = {
  info: 'INFO',
  warn: 'WARN',
  error: 'ERR',
  success: 'OK',
}

const logBody = ref<HTMLElement | null>(null)
const paused = ref(false)

/** 复制按钮的短暂反馈：idle -> done/fail -> 1.5 秒后回 idle */
const copyState = ref<'idle' | 'done' | 'fail'>('idle')
let copyTimer: ReturnType<typeof setTimeout> | null = null

const copyLabel = computed(() => {
  if (copyState.value === 'done') return '已复制'
  if (copyState.value === 'fail') return '复制失败'
  return '复制'
})

/** 粘在底部时才自动跟随：用户往上翻看历史时不抢滚动条 */
async function followBottom() {
  if (paused.value) return
  await nextTick()
  const el = logBody.value
  if (!el) return
  el.scrollTop = el.scrollHeight
}

watch(() => props.logs.length, followBottom)
onMounted(followBottom)

function togglePause() {
  paused.value = !paused.value
  if (!paused.value) void followBottom()
}

/**
 * 写剪贴板。
 *
 * 先试 Clipboard API：Tauri 的 WebView 在生产环境下不是普通 http(s) 页面，
 * 有的平台会直接判成非安全上下文而拒掉；拒了就退回隐藏 textarea + execCommand，
 * 这条老路在 WebView 里更稳。
 */
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    // 继续走兜底
  }
  try {
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.style.position = 'fixed'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(textarea)
    return ok
  } catch {
    return false
  }
}

/**
 * 复制当前抽屉里的日志。
 *
 * 只复制这个窗口里已经在显示的那一份（`props.logs`），不是磁盘上的完整日志文件；
 * 格式与界面一致：一条一行「时间 级别 内容」，多行消息原样保留。
 */
async function copyLogs() {
  if (props.logs.length === 0) return
  const text = props.logs
    .map((log) => `${log.timestamp} ${LEVEL_LABELS[log.level] || log.level} ${log.message}`)
    .join('\n')
  const ok = await copyText(text)
  copyState.value = ok ? 'done' : 'fail'
  if (copyTimer) clearTimeout(copyTimer)
  copyTimer = setTimeout(() => {
    copyState.value = 'idle'
    copyTimer = null
  }, 1500)
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close')
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
  if (copyTimer) {
    clearTimeout(copyTimer)
    copyTimer = null
  }
})
</script>

<template>
  <div class="log-drawer-mask" @click.self="emit('close')">
    <aside class="log-drawer" role="dialog" :aria-label="title">
      <header class="drawer-head">
        <div class="head-main">
          <h4>{{ title }}</h4>
          <span class="head-status">{{ status }}</span>
        </div>
        <div class="head-actions">
          <slot name="actions" />
          <button
            class="btn btn-secondary btn-xs"
            type="button"
            :disabled="logs.length === 0"
            :title="logs.length === 0 ? '当前窗口里没有日志可复制' : '复制当前窗口内的全部日志'"
            @click="copyLogs"
          >
            {{ copyLabel }}
          </button>
          <button class="btn btn-secondary btn-xs" @click="togglePause">
            {{ paused ? '继续跟随' : '暂停跟随' }}
          </button>
          <button class="btn btn-secondary btn-xs" @click="emit('clear')">清空</button>
          <button class="drawer-close" type="button" title="关闭" aria-label="关闭" @click="emit('close')">
            ×
          </button>
        </div>
      </header>

      <div ref="logBody" class="drawer-body">
        <div v-for="log in logs" :key="log.id" class="log-entry" :class="log.level">
          <span class="log-time">{{ log.timestamp }}</span>
          <span class="log-level">{{ LEVEL_LABELS[log.level] || log.level }}</span>
          <span class="log-message">{{ log.message }}</span>
        </div>
        <div v-if="logs.length === 0" class="log-empty">{{ emptyText }}</div>
      </div>
    </aside>
  </div>
</template>

<style scoped>
.log-drawer-mask {
  position: fixed;
  inset: 0;
  z-index: 900;
  background: rgba(0, 0, 0, 0.18);
  display: flex;
  justify-content: flex-end;
}
.log-drawer {
  width: min(560px, 92vw);
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
  border-left: 1px solid var(--border-color);
  box-shadow: -8px 0 24px rgba(0, 0, 0, 0.12);
  animation: drawer-in 0.16s ease-out;
}
@keyframes drawer-in {
  from {
    transform: translateX(16px);
    opacity: 0.6;
  }
  to {
    transform: translateX(0);
    opacity: 1;
  }
}
.drawer-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 16px 10px;
  border-bottom: 1px solid var(--border-color);
}
.head-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.head-main h4 {
  margin: 0;
  font-size: 14px;
  color: var(--text-primary);
}
.head-status {
  font-size: 12px;
  color: var(--text-muted);
}
.head-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  justify-content: flex-end;
}
.drawer-close {
  width: 24px;
  height: 24px;
  border: none;
  background: transparent;
  color: var(--text-muted);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
  border-radius: 4px;
}
.drawer-close:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.drawer-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px 16px 16px;
  font-family: 'SF Mono', 'Monaco', 'Consolas', monospace;
  font-size: 12.5px;
  line-height: 1.6;
}
.log-entry {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 2px 0;
  border-bottom: 1px solid transparent;
  word-break: break-all;
}
.log-entry:hover {
  border-bottom-color: var(--border-color);
}
.log-time {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}
.log-level {
  flex-shrink: 0;
  width: 34px;
  font-size: 10px;
  color: var(--text-muted);
}
.log-entry.warn .log-level,
.log-entry.warn .log-message {
  color: #b45309;
}
.log-entry.error .log-level,
.log-entry.error .log-message {
  color: var(--danger);
}
.log-entry.success .log-level {
  color: #15803d;
}
.log-message {
  color: var(--text-secondary);
  white-space: pre-wrap;
}
.log-empty {
  padding: 32px 8px;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.7;
  text-align: center;
  font-family: inherit;
}
</style>
