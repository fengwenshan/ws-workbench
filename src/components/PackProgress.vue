<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { PackProgress, PackResult } from '@/types'

const props = defineProps<{
  progress: PackProgress | null
  packing: boolean
  /** 打包结果：在模块内展示成功 / 失败明细，不再写全局日志 */
  result?: PackResult | null
  /** invoke 直接失败时的错误 */
  error?: string
}>()

/** 打包结束后的一行结论（模块内日志） */
const statusLine = computed(() => {
  if (props.error) return props.error
  const res = props.result
  if (!res) return ''
  if (res.success) {
    return `打包完成：${res.copied_files}/${res.total_files} 个文件，耗时 ${res.elapsed_ms}ms`
  }
  return `打包完成（有错误）：${res.copied_files}/${res.total_files}，跳过 ${res.skipped_files}`
})
const failed = computed(() => {
  if (props.error) return true
  return props.result ? !props.result.success : false
})
const errors = computed(() => {
  const res = props.result
  if (!res || res.success) return []
  return res.errors.slice(0, 5)
})

const phaseLabels: Record<string, string> = {
  scanning: '扫描文件中...',
  cleaning: '清空输出目录...',
  copying: '复制文件中...',
  zipping: '压缩文件中...',
  done: '完成',
}

// 是否显示进度条（打包完成后延迟隐藏）
const visible = ref(false)
let hideTimer: ReturnType<typeof setTimeout> | null = null

watch(() => props.packing, (isPacking, wasPacking) => {
  if (isPacking) {
    // 开始打包，立即显示
    if (hideTimer) {
      clearTimeout(hideTimer)
      hideTimer = null
    }
    visible.value = true
  } else if (wasPacking) {
    // 刚结束打包，延迟 2 秒后隐藏
    hideTimer = setTimeout(() => {
      visible.value = false
    }, 2000)
  }
}, { immediate: true })
</script>

<template>
  <div class="pack-progress" v-if="visible">
    <div class="progress-info">
      <span class="phase-label">
        {{ progress ? phaseLabels[progress.phase] || progress.phase : '准备中...' }}
      </span>
      <span class="progress-count" v-if="progress && progress.total > 0">
        {{ progress.current }} / {{ progress.total }}
      </span>
      <span class="progress-percent" v-if="progress">
        {{ progress.percentage.toFixed(1) }}%
      </span>
    </div>
    <div class="progress-bar">
      <div
        class="progress-bar-fill"
        :style="{ width: (progress ? progress.percentage : 0) + '%' }"
      ></div>
    </div>
    <div class="current-file" v-if="progress && progress.current_file">
      {{ progress.current_file }}
    </div>
    <div v-if="statusLine && !packing" class="pack-status" :class="{ failed }">
      {{ statusLine }}
    </div>
    <ul v-if="errors.length" class="pack-errors">
      <li v-for="(line, index) in errors" :key="index">{{ line }}</li>
    </ul>
  </div>
</template>

<style scoped>
.pack-progress {
  padding: 12px 20px;
  border-top: 1px solid var(--border-color);
}
.progress-info {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 6px;
  font-size: 14px;
}
.phase-label {
  color: var(--text-primary);
  font-weight: 500;
}
.progress-count {
  color: var(--text-secondary);
}
.progress-percent {
  color: var(--primary);
}
.progress-bar {
  height: 8px;
  background: var(--bg-readonly);
  border-radius: 4px;
  overflow: hidden;
}
.progress-bar-fill {
  height: 100%;
  background: var(--primary);
  border-radius: 4px;
  transition: width 0.3s ease;
}
.pack-status {
  margin-top: 6px;
  font-size: 12px;
  color: var(--text-secondary);
}
.pack-status.failed {
  color: var(--danger);
}
.pack-errors {
  margin: 4px 0 0;
  padding-left: 18px;
  font-size: 12px;
  color: var(--danger);
}
.current-file {
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
