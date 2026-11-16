<script setup lang="ts">
import { ref } from 'vue'
import { message } from 'ant-design-vue'
import {
  getLanShareSnapshot,
  setLanShareRequireAccept,
  type LanShareSnapshot,
} from '@/api/lanshare'

/**
 * 「接收文件前需要确认」勾选（设计 §3.1）。
 *
 * 默认关闭 = 收到文件直接落盘；开启后接收端恢复 pending offer 流程
 * （左栏「待接收」+ 聊天内接受/拒绝卡）。切换后立即持久化并回抛最新快照；
 * 失败时重拉快照回滚勾选状态，避免 UI 与后端设置不一致。
 */
const props = defineProps<{
  snapshot: LanShareSnapshot | null
}>()

const emit = defineEmits<{
  updated: [snapshot: LanShareSnapshot]
}>()

const busy = ref(false)

function errorText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

async function onChange(event: Event) {
  if (!props.snapshot || busy.value) return
  const input = event.target as HTMLInputElement
  const next = input.checked
  busy.value = true
  try {
    emit('updated', await setLanShareRequireAccept(next))
    message.success(next ? '已开启接收前确认' : '已关闭接收前确认，文件会直接保存')
  } catch (err) {
    message.error(`保存失败：${errorText(err)}`)
    // 浏览器已经把勾选框切过去了；`:checked` 绑定的旧值没变时 Vue 不会重写 DOM，
    // 所以失败时手动把 DOM 纠正回快照里的值，再拉一次快照兜底。
    input.checked = props.snapshot.require_accept
    try {
      emit('updated', await getLanShareSnapshot())
    } catch {
      /* 拉不到快照就等下一次 snapshot 事件刷新 */
    }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div v-if="snapshot" class="settings-card">
    <label class="confirm-row">
      <input
        class="confirm-check"
        type="checkbox"
        :checked="snapshot.require_accept"
        :disabled="busy"
        @change="onChange"
      />
      <span class="confirm-body">
        <span class="confirm-title">接收文件前需要确认</span>
        <span class="confirm-desc">
          默认关闭：别人发来的文件直接保存到接收目录。开启后文件会先出现在「待接收」，
          需要手动接受才会接收。
        </span>
      </span>
    </label>
  </div>
</template>

<style scoped>
.settings-card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  padding: 14px;
}
.confirm-row {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  cursor: pointer;
}
.confirm-check {
  width: 14px;
  height: 14px;
  margin: 2px 0 0;
  flex-shrink: 0;
  accent-color: var(--primary);
  cursor: pointer;
}
.confirm-body {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}
.confirm-title {
  font-size: 13px;
  color: var(--text-primary);
}
.confirm-desc {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
</style>
