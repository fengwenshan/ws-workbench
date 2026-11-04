import { ref } from 'vue'
import type { LogEntry, LogLevel } from '@/types'
import { timeLabel } from '@/utils/time'

export function useLog() {
  const logs = ref<LogEntry[]>([])
  let logId = 0

  function addLog(level: LogLevel, message: string) {
    logs.value.push({
      id: logId++,
      timestamp: timeLabel(),
      level,
      message,
    })
  }

  function clearLogs() {
    logs.value = []
  }

  return { logs, addLog, clearLogs }
}
