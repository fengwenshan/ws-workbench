<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { Input, InputNumber, Select, Switch, message } from 'ant-design-vue'
import { useKeepAwake } from '@/composables/useKeepAwake'
import { platform } from '@/platform'
import type { SleepTrigger, SleepTriggerKind } from '@/types/sleep'

/**
 * 屏幕休眠页签：开始 / 切换时长 / 停止都收在「开启新会话」一张卡里 ——
 * 「允许显示器睡眠」开关 + 不限时 / 分钟 / 小时 / 自定义，保持中直接显示剩余与
 * 已保持时长、可点其它时长直接切换；下面是「自动保持唤醒」规则
 * （App 运行 / 接通电源 / 外接显示器）。
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

/** 是否允许显示器睡眠（默认关 = 显示器保持常亮） */
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

/**
 * 主窗口重新获得焦点（比如从托盘面板点「显示主界面」回来）先补一次状态，
 * 不用等最多 5 秒的轮询；托盘面板里的开关会立刻反映到这里。
 */
function onWindowFocus(): void {
  if (props.active) keepAwake.wake()
}

onMounted(() => {
  window.addEventListener('focus', onWindowFocus)
})

onUnmounted(() => {
  window.removeEventListener('focus', onWindowFocus)
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

/** 保持中时，把「后端剩余秒数 + 已过时间」还原成会话开始时的时长，用来高亮对应时长项 */
const selectedPresetSeconds = computed<number | null>(() => {
  const current = status.value
  if (!active.value || !current || current.remaining_secs == null) return null
  if (current.started_at_ms == null) return null
  // remaining_secs 是拉取状态那一刻的值，加上从开始到那一刻的时长就是原始时长
  const totalSecs = current.remaining_secs + (statusAtMs.value - current.started_at_ms) / 1000
  const presets = [
    ...MINUTE_PRESETS.map((minutes) => minutes * 60),
    ...HOUR_PRESETS.map((hours) => hours * 3600),
  ]
  return presets.find((seconds) => Math.abs(totalSecs - seconds) <= 5) ?? null
})

/** 是否无限期会话（高亮「无限期」） */
const unlimitedSelected = computed(
  () => active.value && status.value?.remaining_secs == null,
)

/** 保持中时开关只反映开始时的选择，不在会话中途改动 */
const allowSleepChecked = computed(() =>
  active.value ? (status.value?.allow_display_sleep ?? false) : allowDisplaySleep.value,
)

/** 保持中的一句话摘要 */
const sessionSummary = computed(() => {
  if (!active.value) return ''
  return unlimitedSelected.value
    ? `不限时 · 已保持 ${elapsedLabel.value}`
    : `剩余 ${remainingLabel.value} · 已保持 ${elapsedLabel.value}`
})

/** 点的是不是当前正在跑的时长（点中已选中的项不重置计时） */
function isCurrentSelection(seconds: number | null): boolean {
  if (!active.value) return false
  if (seconds === null) return unlimitedSelected.value
  return selectedPresetSeconds.value === seconds
}

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

/** 当前生效规则的名字（用于会话卡副标题） */
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
  if (!next) {
    message.success('已关闭自动保持')
    return
  }
  // 总开关和规则是两件事：没有规则时也允许先开，但要说清「开了不会生效」
  const rules = triggersState.value?.triggers.length ?? 0
  message.success(
    rules > 0 ? '已开启自动保持' : '已开启自动保持；还没有规则，添加规则后才会生效',
  )
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

/**
 * 开始 / 切换会话；`seconds = null` 表示无限期。
 * 保持中点其它时长会直接按新时长重新开始（后端会替换旧会话）。
 */
async function startSession(seconds: number | null): Promise<void> {
  if (keepAwake.starting || !supported.value) return
  // 点中正在跑的时长不重置计时
  if (isCurrentSelection(seconds)) return
  const wasActive = active.value
  // 已有会话时沿用它开始时的「允许显示器睡眠」，中途不可改
  const allowSleepAtStart = wasActive
    ? (status.value?.allow_display_sleep ?? false)
    : allowDisplaySleep.value
  const ok = await keepAwake.start(seconds, allowSleepAtStart)
  if (ok) {
    const durationText = seconds == null ? '无限期' : formatDuration(seconds)
    message.success(
      `${wasActive ? '已切换保持唤醒时长' : '已开始保持唤醒'}（${durationText}）${
        allowSleepAtStart ? '，允许显示器睡眠' : '，显示器保持常亮'
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
    <!-- 会话控制：开始 / 切换 / 停止都在这一张卡里 -->
    <section class="sleep-card">
      <div class="card-head">
        <div class="card-titles">
          <span class="card-label">开启新会话</span>
          <span class="card-sub">
            <template v-if="active">
              保持中 · {{ sessionSummary }}
              <template v-if="activeTriggerLabel">
                · 自动规则：{{ activeTriggerLabel }}
              </template>
            </template>
            <template v-else>点一个时长立刻开始，也可以自定义小时 / 分钟</template>
          </span>
        </div>
        <div class="card-actions">
          <div class="allow-sleep" :class="{ disabled: active || !supported }">
            <span>允许显示器睡眠</span>
            <Switch
              size="small"
              :checked="allowSleepChecked"
              :disabled="active || !supported"
              @change="(checked) => (allowDisplaySleep = Boolean(checked))"
            />
          </div>
          <button
            v-if="active"
            type="button"
            class="btn btn-danger-ghost btn-xs"
            :disabled="keepAwake.stopping"
            @click="handleStop"
          >
            {{ keepAwake.stopping ? '停止中…' : '停止保持' }}
          </button>
        </div>
      </div>

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

      <div class="preset-block">
        <span class="preset-label">不限时</span>
        <div class="preset-row">
          <button
            type="button"
            class="duration-chip"
            :class="{ active: unlimitedSelected }"
            :disabled="keepAwake.starting || !supported"
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
            :class="{ active: selectedPresetSeconds === minutes * 60 }"
            :disabled="keepAwake.starting || !supported"
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
            :class="{ active: selectedPresetSeconds === hours * 3600 }"
            :disabled="keepAwake.starting || !supported"
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
            :disabled="keepAwake.starting || !supported"
          />
          <span class="custom-unit">小时</span>
          <InputNumber
            v-model:value="customMinutes"
            :min="0"
            :max="59"
            size="small"
            class="custom-input"
            :disabled="keepAwake.starting || !supported"
          />
          <span class="custom-unit">分钟</span>
          <button
            type="button"
            class="btn btn-primary btn-xs"
            :disabled="keepAwake.starting || !supported"
            @click="startCustom"
          >
            {{ keepAwake.starting ? '开启中…' : '开始' }}
          </button>
        </div>
      </div>
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
          还没有规则；先添加一条，自动保持才会生效。
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
.card-sub strong {
  color: var(--text-primary);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.card-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
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
/* 选中项在开始/切换的瞬间会短暂 disabled，保持高亮不被压暗 */
.duration-chip.active,
.duration-chip.active:disabled {
  opacity: 1;
  cursor: default;
}
.duration-chip:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: 2px;
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

.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}
</style>
