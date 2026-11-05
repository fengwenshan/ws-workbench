<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import type { VpnApi } from '@/composables/useVpn'

const props = defineProps<{
  vpn: VpnApi
}>()

const emit = defineEmits<{
  close: []
}>()

const vpn = props.vpn

const LEVEL_LABELS: Record<string, string> = {
  info: 'INFO',
  warn: 'WARN',
  error: 'ERR',
  success: 'OK',
}

const logBody = ref<HTMLElement | null>(null)
const paused = ref(false)

const logs = computed(() => vpn.logs)

const statusText = computed(() => {
  if (vpn.isConnected) return '已连接，日志实时刷新'
  if (vpn.phaseBusy || vpn.connecting) return '连接中…'
  return '未连接'
})

/** 粘在底部时才自动跟随：用户往上翻看历史时不抢滚动条 */
async function followBottom() {
  if (paused.value) return
  await nextTick()
  const el = logBody.value
  if (!el) return
  el.scrollTop = el.scrollHeight
}

watch(() => logs.value.length, followBottom)

function togglePause() {
  paused.value = !paused.value
  if (!paused.value) void followBottom()
}

function clearLogs() {
  vpn.clearLogs()
}
</script>

<template>
  <div class="log-drawer-mask" @click.self="emit('close')">
    <aside class="log-drawer" role="dialog" aria-label="实时日志">
      <header class="drawer-head">
        <div class="head-main">
          <h4>实时日志</h4>
          <span class="head-status">{{ statusText }}</span>
        </div>
        <div class="head-actions">
          <button class="btn btn-secondary btn-xs" @click="togglePause">
            {{ paused ? '继续跟随' : '暂停跟随' }}
          </button>
          <button class="btn btn-secondary btn-xs" @click="clearLogs">清空</button>
          <button class="btn btn-secondary btn-xs" @click="vpn.openSessionDir()">打开日志目录</button>
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
        <div v-if="logs.length === 0" class="log-empty">
          暂无日志。连接 VPN 后，隧道建立、DNS、规则命中这些过程会实时显示在这里。
        </div>
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
