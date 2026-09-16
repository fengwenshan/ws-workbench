import { ref } from 'vue'
import type { LogEntry, LogLevel } from '@/types'

export function useLog() {
  const logs = ref<LogEntry[]>([])
  let logId = 0

  function addLog(level: LogLevel, message: string) {
    logs.value.push({
      id: logId++,
      timestamp: new Date().toLocaleTimeString('zh-CN'),
      level,
      message,
    })
  }

  function clearLogs() {
    logs.value = []
  }

  return { logs, addLog, clearLogs }
}
