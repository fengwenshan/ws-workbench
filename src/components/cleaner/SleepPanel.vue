<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { Checkbox, message } from 'ant-design-vue'
import { useKeepAwake } from '@/composables/useKeepAwake'
import { platform } from '@/platform'
import type { KeepAwakeDurationMinutes } from '@/types/sleep'

/**
 * 屏幕休眠页签：开关「保持唤醒」+ 时长（15/30/60/120 分钟 / 无限）+
 * 「允许屏幕关闭」勾选 + 状态卡（已保持时长、剩余时间、是否阻止屏幕关闭）+ 停止。
 *
 * 轮询开关由父级传入的 `active` 决定（页签切走或整个工具页不可见就停），
 * 真正的 5 秒节拍与「页面隐藏暂停」在 useKeepAwake 里；剩余时间用本地 1 秒
 * 时钟推算，不依赖 5 秒轮询的精度（后端到点会自动释放断言，下一轮询即同步）。
 */
const props = defineProps<{
  /** 当前页签是否激活（CleanerPanel 的 tab === 'sleep' && 整个工具页可见） */
  active: boolean
}>()

const keepAwake = useKeepAwake()

// ============================ 表单状态 ============================

/** 时长选择（分钟）；0 = 无限。开始后锁定，停止后保留上次选择 */
const durationMinutes = ref<KeepAwakeDurationMinutes>(30)
/** 是否允许屏幕关闭（默认关 = 屏幕保持常亮，与设计 4.1 一致） */
const allowDisplaySleep = ref(false)

const DURATION_OPTIONS: { value: KeepAwakeDurationMinutes; label: string }[] = [
  { value: 15, label: '15 分钟' },
  { value: 30, label: '30 分钟' },
  { value: 60, label: '1 小时' },
  { value: 120, label: '2 小时' },
  { value: 0, label: '无限' },
]

/** 当前平台是否支持：后端在非 macOS / Windows 上返回「当前平台不支持」 */
const supported = computed(() => !platform.is_linux)

// ============================ 轮询生命周期 ============================

watch(
  () => props.active,
  (active) => {
    if (active) keepAwake.startPolling()
    else keepAwake.stopPolling()
  },
  { immediate: true },
)

onUnmounted(() => {
  keepAwake.stopPolling()
})

// ============================ 状态展示 ============================

const status = computed(() => keepAwake.status)
const active = computed(() => keepAwake.active)

/** 本地时钟：剩余时间/已保持时长逐秒走；不在激活状态时不跑 */
const nowMs = ref(Date.now())
let clockTimer: ReturnType<typeof setInterval> | null = null

function startClock(): void {
  if (clockTimer === null) {
    clockTimer = setInterval(() => {
      nowMs.value = Date.now()
    }, 1000)
  }
}

function stopClock(): void {
  if (clockTimer !== null) {
    clearInterval(clockTimer)
    clockTimer = null
  }
}

// 只在「页签激活 + 正在保持」时跑秒表，其它时候不占资源
watch(
  [() => props.active, active],
  ([isActive, isKeeping]) => {
    if (isActive && isKeeping) startClock()
    else stopClock()
  },
  { immediate: true },
)

onUnmounted(() => {
  stopClock()
})

/** 最近一次拿到后端状态的本地时刻：用它把 5 秒轮询的剩余秒数逐秒外推 */
const statusAtMs = ref(Date.now())
watch(
  () => keepAwake.status,
  () => {
    statusAtMs.value = Date.now()
  },
)

const startedLabel = computed(() => {
  const startedAt = status.value?.started_at_ms
  if (!active.value || !startedAt) return '—'
  const date = new Date(startedAt)
  if (Number.isNaN(date.getTime())) return '—'
  return date.toLocaleTimeString('zh-CN')
})

const elapsedLabel = computed(() => {
  const startedAt = status.value?.started_at_ms
  if (!active.value || !startedAt) return '—'
  const elapsedSecs = Math.max(0, Math.floor((nowMs.value - startedAt) / 1000))
  return formatDuration(elapsedSecs)
})

/** 剩余时间：无限时长为「无限」；否则用后端剩余秒数按本地时钟外推 */
const remainingLabel = computed(() => {
  const current = status.value
  if (!active.value || !current) return '—'
  if (current.remaining_secs == null) return '无限'
  const driftSecs = Math.floor((nowMs.value - statusAtMs.value) / 1000)
  const remainingSecs = Math.max(0, current.remaining_secs - driftSecs)
  return formatDuration(remainingSecs)
})

const displayLabel = computed(() => {
  const current = status.value
  if (!active.value || !current) return '—'
  return current.allow_display_sleep ? '允许屏幕关闭' : '屏幕保持常亮'
})

const platformHint = computed(() => {
  if (status.value?.platform_hint) return status.value.platform_hint
  if (platform.is_windows) {
    return '不拦屏保、合盖与主动睡眠；Modern Standby 下可能被系统挂起'
  }
  if (platform.is_macos) {
    return '只阻止空闲睡眠（默认同时阻止屏幕关闭）；合盖、主动睡眠与锁屏不在支持范围'
  }
  return '当前平台不支持保持唤醒'
})

/** 秒数转「x 小时 y 分 / x 分 y 秒 / x 秒」 */
function formatDuration(totalSecs: number): string {
  const secs = Math.max(0, Math.floor(totalSecs))
  const hours = Math.floor(secs / 3600)
  const minutes = Math.floor((secs % 3600) / 60)
  const seconds = secs % 60
  if (hours > 0) {
    return minutes > 0 ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`
  }
  if (minutes > 0) {
    return seconds > 0 ? `${minutes} 分 ${seconds} 秒` : `${minutes} 分`
  }
  return `${seconds} 秒`
}

// ============================ 动作 ============================

async function handleStart(): Promise<void> {
  if (active.value || keepAwake.starting || !supported.value) return
  const durationSeconds = durationMinutes.value === 0 ? null : durationMinutes.value * 60
  const ok = await keepAwake.start(durationSeconds, allowDisplaySleep.value)
  if (ok) {
    const durationText =
      durationMinutes.value === 0 ? '无限' : formatDuration(durationMinutes.value * 60)
    message.success(
      `已开始保持唤醒（${durationText}）${
        allowDisplaySleep.value ? '，允许屏幕关闭' : '，屏幕保持常亮'
      }`,
    )
  } else {
    message.error(keepAwake.actionError || '开启保持唤醒失败')
  }
}

async function handleStop(): Promise<void> {
  if (!active.value || keepAwake.stopping) return
  const ok = await keepAwake.stop()
  if (ok) message.success('已停止保持唤醒')
  else message.error(keepAwake.actionError || '停止保持唤醒失败')
}
</script>

<template>
  <div class="sleep-panel">
    <!-- 状态卡 -->
    <section class="sleep-card">
      <div class="card-head">
        <div class="card-titles">
          <span class="card-label">保持唤醒</span>
          <span class="card-sub">
            {{
              active
                ? '正在阻止系统空闲睡眠，定时到点会自动释放'
                : '开启后系统不会因空闲进入睡眠'
            }}
          </span>
        </div>
        <button
          type="button"
          class="btn btn-secondary btn-xs"
          :disabled="keepAwake.polling === false"
          @click="keepAwake.refresh()"
        >
          立即刷新
        </button>
      </div>

      <div class="status-main">
        <span class="status-badge" :class="{ 'is-active': active }">
          {{ active ? '保持唤醒中' : '未开启' }}
        </span>
        <span class="status-display">{{ displayLabel }}</span>
      </div>

      <div class="status-facts">
        <span class="fact">
          已保持 <strong>{{ elapsedLabel }}</strong>
        </span>
        <span class="fact">
          剩余 <strong>{{ remainingLabel }}</strong>
        </span>
        <span class="fact">
          开始于 <strong>{{ startedLabel }}</strong>
        </span>
      </div>

      <p class="platform-hint">{{ platformHint }}</p>

      <p v-if="!supported" class="warn-bar">
        当前平台不支持保持唤醒，仅 macOS / Windows 可用。
      </p>

      <p v-if="keepAwake.statusError" class="error-bar">
        <span>{{ keepAwake.statusError }}</span>
        <button type="button" class="btn btn-secondary btn-xs" @click="keepAwake.refresh()">
          重试
        </button>
      </p>
      <p v-if="keepAwake.actionError" class="error-bar">
        <span>{{ keepAwake.actionError }}</span>
      </p>
    </section>

    <!-- 控制卡 -->
    <section class="sleep-card">
      <div class="card-head">
        <div class="card-titles">
          <span class="card-label">开启设置</span>
          <span class="card-sub">开始后如需调整，先停止再重新开启</span>
        </div>
      </div>

      <div class="control-block">
        <span class="control-label">保持时长</span>
        <div class="duration-group" role="group" aria-label="保持唤醒时长">
          <button
            v-for="option in DURATION_OPTIONS"
            :key="option.value"
            type="button"
            class="duration-chip"
            :class="{ active: durationMinutes === option.value }"
            :disabled="active || !supported"
            :aria-pressed="durationMinutes === option.value"
            @click="durationMinutes = option.value"
          >
            {{ option.label }}
          </button>
        </div>
      </div>

      <label class="display-option" :class="{ disabled: active || !supported }">
        <Checkbox
          :checked="allowDisplaySleep"
          :disabled="active || !supported"
          @change="
            (event) => {
              allowDisplaySleep = event.target.checked ?? false
            }
          "
        />
        <span class="display-option-text">
          允许屏幕关闭
          <em>只阻止系统睡眠，屏幕仍按系统设置自动关闭</em>
        </span>
      </label>

      <div class="action-buttons">
        <button
          type="button"
          class="btn btn-primary"
          :disabled="active || keepAwake.starting || !supported"
          :title="active ? '已经在保持唤醒，先停止才能修改设置' : '立即阻止系统空闲睡眠'"
          @click="handleStart"
        >
          {{ keepAwake.starting ? '开启中…' : active ? '保持唤醒中' : '开始保持唤醒' }}
        </button>
        <button
          type="button"
          class="btn btn-danger-ghost"
          :disabled="!active || keepAwake.stopping"
          @click="handleStop"
        >
          {{ keepAwake.stopping ? '停止中…' : '停止' }}
        </button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.sleep-panel {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 10px 20px 16px;
  overflow-y: auto;
}

.sleep-card {
  flex-shrink: 0;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  padding: 14px;
}

.card-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}
.card-titles {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.card-label {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}
.card-sub {
  font-size: 11px;
  color: var(--text-muted);
}

/* ===== 状态 ===== */
.status-main {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 12px 0 8px;
}
.status-badge {
  display: inline-flex;
  align-items: center;
  height: 26px;
  padding: 0 12px;
  border-radius: 13px;
  background: var(--bg-tag);
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 600;
}
.status-badge.is-active {
  background: rgba(16, 185, 129, 0.12);
  color: var(--success);
}
.status-display {
  font-size: 12px;
  color: var(--text-secondary);
}

.status-facts {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 22px;
  font-size: 12px;
  color: var(--text-muted);
}
.status-facts strong {
  color: var(--text-secondary);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.platform-hint {
  margin-top: 10px;
  padding: 7px 12px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-secondary);
  font-size: 11px;
  color: var(--text-muted);
}

.warn-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 10px;
  padding: 7px 12px;
  border: 1px solid rgba(245, 158, 11, 0.3);
  border-radius: 6px;
  background: rgba(245, 158, 11, 0.08);
  color: var(--warning);
  font-size: 12px;
}

.error-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 10px;
  padding: 7px 12px;
  border: 1px solid rgba(239, 68, 68, 0.28);
  border-radius: 6px;
  background: #fef2f2;
  color: var(--danger);
  font-size: 12px;
}
.error-bar span {
  flex: 1;
  min-width: 0;
  word-break: break-all;
}

/* ===== 设置 ===== */
.control-block {
  margin-top: 14px;
}
.control-label {
  display: block;
  margin-bottom: 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
}
.duration-group {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.duration-chip {
  height: 28px;
  padding: 0 14px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
  transition: border-color 0.2s, color 0.2s, background 0.2s;
}
.duration-chip:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--text-primary);
}
.duration-chip.active {
  border-color: var(--primary);
  background: rgba(59, 130, 246, 0.1);
  color: var(--primary);
  font-weight: 600;
}
.duration-chip:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.duration-chip:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: 2px;
}

.display-option {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin-top: 14px;
  cursor: pointer;
}
.display-option.disabled {
  cursor: not-allowed;
  opacity: 0.7;
}
.display-option-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
  font-size: 12px;
  color: var(--text-secondary);
}
.display-option-text em {
  font-size: 11px;
  font-style: normal;
  color: var(--text-muted);
}

.action-buttons {
  display: flex;
  gap: 8px;
  margin-top: 16px;
}
.action-buttons .btn {
  height: 30px;
  padding: 0 14px;
  font-size: 12px;
}
.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}
</style>
