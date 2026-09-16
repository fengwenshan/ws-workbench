<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'
import type { LogEntry } from '@/types'

const props = defineProps<{
  logs: LogEntry[]
}>()

const emit = defineEmits<{
  clear: []
}>()

const logContainer = ref<HTMLElement | null>(null)

watch(() => props.logs.length, async () => {
  await nextTick()
  if (logContainer.value) {
    logContainer.value.scrollTop = logContainer.value.scrollHeight
  }
})

const levelLabels: Record<string, string> = {
  info: 'INFO',
  warn: 'WARN',
  error: 'ERR',
  success: 'OK',
}
</script>

<template>
  <div class="log-panel">
    <div class="log-header">
      <span>日志</span>
      <button class="clear-btn" @click="emit('clear')">清空</button>
    </div>
    <div class="log-body" ref="logContainer">
      <div v-for="log in logs" :key="log.id" class="log-entry" :class="log.level">
        <span class="log-time">{{ log.timestamp }}</span>
        <span class="log-level">{{ levelLabels[log.level] || log.level }}</span>
        <span class="log-message">{{ log.message }}</span>
      </div>
      <div v-if="logs.length === 0" class="log-empty">暂无日志</div>
    </div>
  </div>
</template>

<style scoped>
.log-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.log-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 20px;
  border-bottom: 1px solid var(--border-color);
  font-size: 13px;
  color: var(--text-secondary);
}
.clear-btn {
  border: 1px solid var(--border-color);
  background: transparent;
  border-radius: 4px;
  padding: 2px 8px;
  font-size: 12px;
  cursor: pointer;
  color: var(--text-secondary);
}
.clear-btn:hover {
  border-color: var(--danger);
  color: var(--danger);
}
.log-body {
  flex: 1;
  overflow-y: auto;
  padding: 8px 20px;
  font-family: 'SF Mono', 'Monaco', 'Consolas', monospace;
  font-size: 13px;
}
.log-entry {
  display: flex;
  gap: 8px;
  padding: 2px 0;
}
.log-time {
  color: var(--text-muted);
  flex-shrink: 0;
}
.log-level {
  flex-shrink: 0;
  font-weight: 600;
  width: 40px;
}
.log-entry.info .log-level { color: var(--text-secondary); }
.log-entry.success .log-level { color: var(--success); }
.log-entry.warn .log-level { color: var(--warning); }
.log-entry.error .log-level { color: var(--danger); }
.log-message {
  color: var(--text-primary);
  word-break: break-all;
}
.log-empty {
  color: var(--text-muted);
  text-align: center;
  padding: 20px;
}
</style>
