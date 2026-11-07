<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { Input, InputNumber, Select, Switch, message } from 'ant-design-vue'
import { useKeepAwake } from '@/composables/useKeepAwake'
import { platform } from '@/platform'
import type { SleepTrigger, SleepTriggerKind } from '@/types/sleep'

/**
 * 屏幕休眠页签：开关「保持唤醒」+ 时长（15/30/60/120 分钟 / 无限）+
 * 「允许屏幕关闭」勾选 + 状态卡（已保持时长、剩余时间、是否阻止屏幕关闭）+ 停止，
 * 以及「自动保持唤醒」规则（App 运行 / 接通电源 / 外接显示器）。
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

/** 是否允许屏幕关闭（默认关 = 屏幕保持常亮，与设计 4.1 一致） */
const allowDisplaySleep = ref(false)

/**
 * 快捷时长分组，参考 Amphetamine「开启新会话 → 分钟 / 小时」的二级菜单：
 * 点一个时长立刻开始一个会话，不用再点「开始」。
 */
const MINUTE_PRESETS = [5, 10, 15, 20, 25, 30, 45]
const HOUR_PRESETS = [1, 2, 3, 4, 6, 8, 12]

/** 自定义时长（小时 + 分钟）；都填 0 时按「无限期」开始 */
const customHours = ref(1)
const customMinutes = ref(0)

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

// ============================ 自动保持规则 ============================

const KIND_LABEL: Record<SleepTriggerKind, string> = {
  app_running: 'App 运行中',
  ac_power: '接通电源',
  external_display: '外接显示器',
}

const KIND_ICON: Record<SleepTriggerKind, string> = {
  app_running: '▣',
  ac_power: '⚡',
  external_display: '▥',
}

const KIND_OPTIONS = (Object.keys(KIND_LABEL) as SleepTriggerKind[]).map((kind) => ({
  value: kind,
  label: KIND_LABEL[kind],
}))

/** 新增规则的表单 */
const newTriggerKind = ref<SleepTriggerKind>('app_running')
const newTriggerValue = ref('')

const triggersState = computed(() => keepAwake.triggers)

function triggerText(trigger: SleepTrigger): string {
  if (trigger.kind === 'app_running') return `运行「${trigger.value}」时`
  if (trigger.kind === 'ac_power') return '接通电源时'
  return '连接外接显示器时'
}

/** 当前环境：直接告诉用户「为什么现在（没）保持」 */
const detectedLabel = computed(() => {
  const detected = triggersState.value?.detected
  if (!detected) return '—'
  const parts = [
    detected.on_ac_power ? '已接电源' : '电池供电',
    detected.external_display ? '有外接显示器' : '单屏',
  ]
  return parts.join(' · ')
})

/** 当前生效规则的名字（用于状态卡） */
const activeTriggerLabel = computed(() => {
  const id = status.value?.active_trigger
  if (!id) return ''
  const trigger = triggersState.value?.triggers.find((item) => item.id === id)
  return trigger ? triggerText(trigger) : '自动规则'
})

async function handleArmed(next: boolean): Promise<void> {
  const ok = await keepAwake.setTriggersArmed(next)
  if (!ok) {
    message.error(keepAwake.triggersError || '切换自动保持失败')
    return
  }
  message.success(next ? '已开启自动保持' : '已关闭自动保持')
}

async function handleAddTrigger(): Promise<void> {
  const kind = newTriggerKind.value
  const value = kind === 'app_running' ? newTriggerValue.value.trim() : ''
  if (kind === 'app_running' && !value) {
    message.warning('请先填进程名，例如 Typora、Code')
    return
  }
  const ok = await keepAwake.addTrigger(kind, value)
  if (!ok) {
    message.error(keepAwake.triggersError || '新增自动规则失败')
    return
  }
  newTriggerValue.value = ''
  message.success('规则已添加，下个 5 秒轮询生效')
}

async function handleToggleTrigger(id: string, enabled: boolean): Promise<void> {
  const ok = await keepAwake.setTriggerEnabled(id, enabled)
  if (!ok) message.error(keepAwake.triggersError || '更新规则失败')
}

async function handleRemoveTrigger(id: string): Promise<void> {
  const ok = await keepAwake.removeTrigger(id)
  if (!ok) message.error(keepAwake.triggersError || '删除规则失败')
  else message.success('规则已删除')
}

// ============================ 动作 ============================

/** 开始一个新会话；`seconds = null` 表示无限期 */
async function startSession(seconds: number | null): Promise<void> {
  if (active.value || keepAwake.starting || !supported.value) return
  const ok = await keepAwake.start(seconds, allowDisplaySleep.value)
  if (ok) {
    const durationText = seconds == null ? '无限期' : formatDuration(seconds)
    message.success(
      `已开始保持唤醒（${durationText}）${
        allowDisplaySleep.value ? '，允许屏幕关闭' : '，屏幕保持常亮'
      }`,
    )
  } else {
    message.error(keepAwake.actionError || '开启保持唤醒失败')
  }
}

/** 自定义时长：小时/分钟都为 0 时等同于「无限期」 */
async function startCustom(): Promise<void> {
  const hours = Math.max(0, Math.round(customHours.value || 0))
  const minutes = Math.max(0, Math.round(customMinutes.value || 0))
  const total = hours * 3600 + minutes * 60
  await startSession(total === 0 ? null : total)
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
    <!-- 状态卡：当前会话一目了然 -->
    <section class="sleep-card hero-card">
      <div class="card-head">
        <div class="card-titles">
          <span class="card-label">保持唤醒</span>
          <span class="card-sub">
            {{
              active
                ? '正在阻止系统空闲睡眠，定时到点会自动释放'
                : '阻止系统因空闲进入睡眠（参考 Amphetamine 的会话机制）'
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

      <div class="hero-body">
        <div v-if="active" class="hero-left">
          <span class="status-badge is-active">保持中</span>
          <div class="hero-count">
            <strong>{{ remainingLabel }}</strong>
            <em>剩余</em>
          </div>
        </div>
        <p v-else class="hero-idle">
          当前没有保持唤醒会话：选一个时长立即开始，到点会自动释放。
        </p>
        <button
          v-if="active"
          type="button"
          class="btn btn-danger-ghost hero-stop"
          :disabled="keepAwake.stopping"
          @click="handleStop"
        >
          {{ keepAwake.stopping ? '停止中…' : '停止保持' }}
        </button>
      </div>

      <div v-if="active" class="status-facts">
        <span class="fact">已保持 <strong>{{ elapsedLabel }}</strong></span>
        <span class="fact">开始于 <strong>{{ startedLabel }}</strong></span>
        <span class="fact">{{ displayLabel }}</span>
        <span v-if="activeTriggerLabel" class="status-trigger">
          自动规则：{{ activeTriggerLabel }}
        </span>
      </div>
      <p v-else class="platform-hint">{{ platformHint }}</p>

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

    <!-- 开启新会话：点时长即开始，和 Amphetamine 的菜单一个手感 -->
    <section class="sleep-card">
      <div class="card-head">
        <div class="card-titles">
          <span class="card-label">开启新会话</span>
          <span class="card-sub">
            {{
              active
                ? '已在进行中，先停止才能开始新的会话'
                : '点一个时长立刻开始，也可以自定义小时 / 分钟'
            }}
          </span>
        </div>
        <div class="allow-sleep" :class="{ disabled: active || !supported }">
          <span>允许屏幕关闭</span>
          <Switch
            size="small"
            :checked="allowDisplaySleep"
            :disabled="active || !supported"
            @change="(checked) => (allowDisplaySleep = Boolean(checked))"
          />
        </div>
      </div>

      <div class="preset-block">
        <span class="preset-label">不限时</span>
        <div class="preset-row">
          <button
            type="button"
            class="duration-chip"
            :disabled="active || keepAwake.starting || !supported"
            @click="startSession(null)"
          >
            无限期
          </button>
        </div>
      </div>

      <div class="preset-block">
        <span class="preset-label">分钟</span>
        <div class="preset-row">
          <button
            v-for="minutes in MINUTE_PRESETS"
            :key="minutes"
            type="button"
            class="duration-chip"
            :disabled="active || keepAwake.starting || !supported"
            @click="startSession(minutes * 60)"
          >
            {{ minutes }} 分钟
          </button>
        </div>
      </div>

      <div class="preset-block">
        <span class="preset-label">小时</span>
        <div class="preset-row">
          <button
            v-for="hours in HOUR_PRESETS"
            :key="hours"
            type="button"
            class="duration-chip"
            :disabled="active || keepAwake.starting || !supported"
            @click="startSession(hours * 3600)"
          >
            {{ hours }} 小时
          </button>
        </div>
      </div>

      <div class="preset-block custom-block">
        <span class="preset-label">自定义</span>
        <div class="preset-row">
          <InputNumber
            v-model:value="customHours"
            :min="0"
            :max="23"
            size="small"
            class="custom-input"
            :disabled="active || !supported"
          />
          <span class="custom-unit">小时</span>
          <InputNumber
            v-model:value="customMinutes"
            :min="0"
            :max="59"
            size="small"
            class="custom-input"
            :disabled="active || !supported"
          />
          <span class="custom-unit">分钟</span>
          <button
            type="button"
            class="btn btn-primary btn-xs"
            :disabled="active || keepAwake.starting || !supported"
            @click="startCustom"
          >
            {{ keepAwake.starting ? '开启中…' : '开始' }}
          </button>
        </div>
      </div>

      <p class="section-hint">
        「允许屏幕关闭」按开始时的开关生效：关着时系统睡眠和屏幕关闭都会被阻止。
      </p>
    </section>

    <!-- 自动保持规则 -->
    <section class="sleep-card">
      <div class="card-head">
        <div class="card-titles">
          <span class="card-label">自动保持唤醒</span>
          <span class="card-sub">
            条件满足时自动保持、失效自动释放；手动开启优先，不会被规则停掉
          </span>
        </div>
        <Switch
          :checked="triggersState?.armed ?? false"
          :disabled="keepAwake.triggersBusy || !supported"
          @change="(checked) => handleArmed(Boolean(checked))"
        />
      </div>

      <p class="detected-line">当前环境：{{ detectedLabel }}</p>

      <div v-if="triggersState?.armed" class="trigger-body">
        <p v-if="!triggersState.triggers.length" class="empty-line">
          还没有规则，选一种条件加一条。
        </p>

        <div v-for="trigger in triggersState.triggers" :key="trigger.id" class="trigger-row">
          <span class="trigger-icon" aria-hidden="true">{{ KIND_ICON[trigger.kind] }}</span>
          <span class="trigger-text">{{ triggerText(trigger) }}</span>
          <Switch
            :checked="trigger.enabled"
            size="small"
            :disabled="keepAwake.triggersBusy"
            @change="(checked) => handleToggleTrigger(trigger.id, Boolean(checked))"
          />
          <button
            type="button"
            class="btn btn-secondary btn-xs"
            :disabled="keepAwake.triggersBusy"
            @click="handleRemoveTrigger(trigger.id)"
          >
            删除
          </button>
        </div>

        <div class="trigger-add">
          <Select
            v-model:value="newTriggerKind"
            :options="KIND_OPTIONS"
            size="small"
            class="trigger-kind"
            :disabled="keepAwake.triggersBusy"
          />
          <Input
            v-if="newTriggerKind === 'app_running'"
            v-model:value="newTriggerValue"
            size="small"
            placeholder="进程名，如 Typora / Code"
            class="trigger-input"
            :disabled="keepAwake.triggersBusy"
            @press-enter="handleAddTrigger"
          />
          <span v-else class="trigger-note">该条件由系统状态自动判断</span>
          <button
            type="button"
            class="btn btn-primary btn-xs"
            :disabled="
              keepAwake.triggersBusy ||
              (newTriggerKind === 'app_running' && !newTriggerValue.trim())
            "
            @click="handleAddTrigger"
          >
            添加
          </button>
        </div>
      </div>

      <p v-if="keepAwake.triggersError" class="error-bar">
        <span>{{ keepAwake.triggersError }}</span>
      </p>
    </section>
  </div>
</template>

<style scoped>
.fact {
  font-size: 12px;
  color: var(--text-muted);
}
.fact strong {
  color: var(--text-primary);
  font-weight: 600;
}

.status-trigger {
  margin-left: 8px;
  padding: 1px 7px;
  border-radius: 10px;
  background: rgba(59, 130, 246, 0.1);
  color: var(--text-active);
  font-size: 11px;
}

.detected-line {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.trigger-body {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.trigger-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
}

.trigger-icon {
  color: var(--text-active);
  font-size: 13px;
}

.trigger-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 13px;
  color: var(--text-primary);
}

.trigger-add {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 2px;
}

.trigger-kind {
  width: 118px;
  flex-shrink: 0;
}

.trigger-input {
  flex: 1;
}

.trigger-note {
  flex: 1;
  font-size: 12px;
  color: var(--text-muted);
}

.empty-line {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}

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

/* ===== 状态英雄区 ===== */
.hero-card {
  background:
    linear-gradient(180deg, rgba(59, 130, 246, 0.06), rgba(59, 130, 246, 0) 62%),
    var(--bg-primary);
}
.hero-body {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-top: 14px;
}
.hero-left {
  display: flex;
  align-items: center;
  gap: 18px;
  min-width: 0;
}
.hero-count {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.hero-count strong {
  font-size: 26px;
  line-height: 1.15;
  font-weight: 600;
  color: var(--text-primary);
  font-variant-numeric: tabular-nums;
}
.hero-count em {
  font-style: normal;
  font-size: 11px;
  color: var(--text-muted);
}
.hero-idle {
  margin: 0;
  font-size: 13px;
  color: var(--text-secondary);
}
.hero-stop {
  flex-shrink: 0;
  height: 32px;
  padding: 0 16px;
  font-size: 12px;
}

/* ===== 开启新会话 ===== */
.preset-block {
  display: flex;
  align-items: flex-start;
  gap: 14px;
  margin-top: 12px;
}
.preset-label {
  flex-shrink: 0;
  width: 44px;
  padding-top: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
}
.preset-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
}
.custom-block {
  margin-top: 14px;
  padding-top: 12px;
  border-top: 1px dashed var(--border-color);
}
.custom-input {
  width: 72px;
}
.custom-unit {
  align-self: center;
  font-size: 12px;
  color: var(--text-muted);
}
.allow-sleep {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-secondary);
}
.allow-sleep.disabled {
  opacity: 0.65;
  cursor: not-allowed;
}
.section-hint {
  margin: 10px 0 0;
  font-size: 11px;
  color: var(--text-muted);
}

.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}
</style>
