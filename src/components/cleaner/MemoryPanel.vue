<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { Checkbox, Modal, message } from 'ant-design-vue'
import type { CleanerApi } from '@/composables/useCleaner'
import type { MemoryApp, QuitOutcome } from '@/types/cleaner'
import { platform } from '@/platform'
import AppIcon from './AppIcon.vue'
import { formatMemory } from './format'

/**
 * 内存占用页签：总览 + 按应用聚合的排行 + 结束进程 / 深度释放。
 *
 * 轮询开关由父级传入的 active 决定（页签切走或整个工具页不可见就停），
 * 真正的 2 秒节拍与「页面隐藏暂停」在 useCleaner 里。
 */
const props = defineProps<{
  cleaner: CleanerApi
  active: boolean
}>()

const cleaner = props.cleaner

// ============================ 轮询生命周期 ============================

watch(
  () => props.active,
  (active) => {
    if (active) cleaner.startMemoryPolling()
    else cleaner.stopMemoryPolling()
  },
  { immediate: true },
)

onUnmounted(() => {
  cleaner.stopMemoryPolling()
})

// ============================ 总览 ============================

/** 占用率取 used/total（和系统口径一致），拿不到时退回后端给的 used_percent */
const usedPercent = computed(() => {
  const snapshot = cleaner.memory
  if (!snapshot) return 0
  const raw =
    snapshot.total > 0 && snapshot.used > 0
      ? (snapshot.used / snapshot.total) * 100
      : Number(snapshot.used_percent ?? 0)
  if (!Number.isFinite(raw)) return 0
  return Math.min(100, Math.max(0, raw))
})

const percentLabel = computed(() => `${Math.round(usedPercent.value)}%`)

/** >=80% 红、>=60% 黄、其余蓝（设计 8 / 计划 Task 8 Step 3） */
const levelClass = computed(() => {
  if (usedPercent.value >= 80) return 'level-danger'
  if (usedPercent.value >= 60) return 'level-warn'
  return 'level-primary'
})

const sampledLabel = computed(() => {
  const at = cleaner.memory?.sampled_at
  if (!at) return ''
  const date = new Date(at)
  if (Number.isNaN(date.getTime())) return ''
  return date.toLocaleTimeString('zh-CN')
})

const swapLabel = computed(() => {
  const snapshot = cleaner.memory
  if (!snapshot || snapshot.swap_total <= 0) return '—'
  return `${formatMemory(snapshot.swap_used)} / ${formatMemory(snapshot.swap_total)}`
})

const releaseText = computed(() => {
  const outcome = cleaner.releaseOutcome
  if (!outcome) return ''
  if (!outcome.ok) return outcome.message || '已取消授权，未做任何修改'
  return `最近一次深度释放：${formatMemory(outcome.freed)}（${formatMemory(
    outcome.before_used,
  )} → ${formatMemory(outcome.after_used)}）`
})

const releaseNote = computed(() => cleaner.releaseOutcome?.note ?? '')
const releaseOk = computed(() => cleaner.releaseOutcome?.ok ?? false)

// ============================ 应用排行 / 勾选 ============================

const selectedIds = ref<string[]>([])

const apps = computed<MemoryApp[]>(() => {
  const list = cleaner.memory?.apps ?? []
  // 后端已按 memory 降序，这里再兜一层，保证 UI 顺序稳定
  return [...list].sort((left, right) => right.memory - left.memory)
})

const selectedApps = computed(() =>
  apps.value.filter((app) => selectedIds.value.includes(app.id)),
)

const selectedMemory = computed(() =>
  selectedApps.value.reduce((sum, app) => sum + app.memory, 0),
)

const selectedProcesses = computed(() =>
  selectedApps.value.reduce((sum, app) => sum + app.process_count, 0),
)

const allSelected = computed(
  () => apps.value.length > 0 && selectedApps.value.length === apps.value.length,
)
const someSelected = computed(
  () => selectedApps.value.length > 0 && !allSelected.value,
)

// 应用退出后（自己关掉或者被结束）从勾选里剔掉，避免底部一直显示「已选 3 项」
watch(
  () => apps.value.map((app) => app.id).join('|'),
  () => {
    const alive = new Set(apps.value.map((app) => app.id))
    selectedIds.value = selectedIds.value.filter((id) => alive.has(id))
  },
)

function toggleApp(id: string, checked: boolean) {
  if (checked) {
    if (!selectedIds.value.includes(id)) selectedIds.value = [...selectedIds.value, id]
  } else {
    selectedIds.value = selectedIds.value.filter((item) => item !== id)
  }
}

function toggleAll(checked: boolean) {
  selectedIds.value = checked ? apps.value.map((app) => app.id) : []
}

// ============================ 结束进程 ============================

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms)
  })
}

function reportQuit(outcome: QuitOutcome, force: boolean) {
  const total = outcome.quit + outcome.forced
  if (total > 0) {
    message.success(
      `${force ? '已强制结束' : '已结束'} ${total} 个应用的进程`,
    )
  }
  if (outcome.failed.length > 0) {
    message.warning(
      `${outcome.failed.length} 个应用未能结束：${outcome.failed.join('、')}`,
    )
  }
}

/** 先优雅退出；等 3 秒重采样，仍存活的再二次确认强制结束（设计 5.4） */
async function runQuit(targets: MemoryApp[]) {
  const outcome = await cleaner.quitApps(
    targets.map((app) => app.id),
    false,
  )
  if (!outcome) {
    message.error(cleaner.actionError || '结束进程失败')
    return
  }
  reportQuit(outcome, false)
  await delay(3000)
  await cleaner.sampleMemory()

  const alive = targets.filter((app) =>
    (cleaner.memory?.apps ?? []).some((item) => item.id === app.id),
  )
  if (alive.length === 0) return
  const aliveProcesses = alive.reduce((sum, app) => sum + app.process_count, 0)
  const aliveMemory = alive.reduce((sum, app) => sum + app.memory, 0)
  Modal.confirm({
    title: '应用仍在运行',
    content: `等待 3 秒后仍有 ${alive.length} 个应用存活（${aliveProcesses} 个进程，${formatMemory(
      aliveMemory,
    )}）：${alive.map((app) => app.name).join('、')}。强制结束会立即丢失未保存内容，是否继续？`,
    okText: '强制结束',
    okType: 'danger',
    cancelText: '保留',
    async onOk() {
      const forced = await cleaner.quitApps(
        alive.map((app) => app.id),
        true,
      )
      if (!forced) {
        message.error(cleaner.actionError || '强制结束失败')
        return
      }
      reportQuit(forced, true)
      await cleaner.sampleMemory()
    },
  })
}

function requestQuit() {
  const targets = selectedApps.value
  if (targets.length === 0 || cleaner.quitting) return
  Modal.confirm({
    title: '结束进程',
    content: `将优雅退出 ${targets.length} 个应用（${selectedProcesses.value} 个进程，合计 ${formatMemory(
      selectedMemory.value,
    )}）：${targets.map((app) => app.name).join('、')}。未保存的内容会丢失，确定继续？`,
    okText: '结束进程',
    okType: 'danger',
    cancelText: '取消',
    async onOk() {
      await runQuit(targets)
    },
  })
}

// ============================ 深度释放 ============================

function requestRelease() {
  if (cleaner.releasing) return
  // 原理说明按平台分开写：Windows 清工作集/待机列表，macOS purge 只清文件缓存
  const principle = platform.is_windows
    ? 'Windows：清空各进程的工作集并清理系统待机列表，被清掉的页仍在磁盘 / 页面文件里，系统会按需换入。'
    : 'macOS：用 purge 清可回收的文件缓存，它不释放应用已 malloc 的内存，收益有限。'
  Modal.confirm({
    title: '深度释放内存',
    content: `${principle}过程中会弹出一次系统授权；取消授权不会做任何修改。是否继续？`,
    okText: '开始释放',
    cancelText: '取消',
    async onOk() {
      const outcome = await cleaner.release()
      if (!outcome) {
        message.error(cleaner.actionError || '深度释放失败')
        return
      }
      if (!outcome.ok) {
        // 取消授权按「未执行」处理，不红字报错
        if (/取消|cancel/i.test(outcome.message)) {
          message.info(outcome.message || '已取消授权，未做任何修改')
        } else {
          message.warning(outcome.message || '深度释放没有成功')
        }
        return
      }
      message.success(
        `已释放 ${formatMemory(outcome.freed)}：${formatMemory(
          outcome.before_used,
        )} → ${formatMemory(outcome.after_used)}`,
      )
    },
  })
}
</script>

<template>
  <div class="memory-panel">
    <!-- 总览卡 -->
    <section class="memory-overview">
      <div class="overview-head">
        <div class="overview-titles">
          <span class="overview-label">内存占用</span>
          <span class="overview-sub">
            每 2 秒自动刷新<template v-if="sampledLabel"> · 上次采样 {{ sampledLabel }}</template>
          </span>
        </div>
        <button
          type="button"
          class="btn btn-secondary btn-xs"
          :disabled="cleaner.sampling"
          @click="cleaner.sampleMemory()"
        >
          {{ cleaner.sampling ? '采样中…' : '立即刷新' }}
        </button>
      </div>

      <div class="overview-main">
        <span class="overview-percent" :class="levelClass">{{ percentLabel }}</span>
        <div class="overview-bar">
          <div
            class="overview-fill"
            :class="levelClass"
            :style="{ width: usedPercent + '%' }"
          ></div>
        </div>
      </div>

      <div class="overview-facts">
        <span class="fact">
          已用 <strong>{{ formatMemory(cleaner.memory?.used ?? 0) }}</strong>
        </span>
        <span class="fact">
          总量 <strong>{{ formatMemory(cleaner.memory?.total ?? 0) }}</strong>
        </span>
        <span class="fact">
          可用 <strong>{{ formatMemory(cleaner.memory?.available ?? 0) }}</strong>
        </span>
        <span class="fact">
          交换 <strong>{{ swapLabel }}</strong>
        </span>
      </div>

      <p v-if="cleaner.memoryError" class="error-bar">
        <span>内存采样失败：{{ cleaner.memoryError }}</span>
        <button class="btn btn-secondary btn-xs" @click="cleaner.sampleMemory()">重试</button>
      </p>

      <p v-if="releaseText" class="release-msg" :class="{ 'release-msg-muted': !releaseOk }">
        <span class="release-title">{{ releaseText }}</span>
        <span v-if="releaseNote" class="release-note">{{ releaseNote }}</span>
      </p>
    </section>

    <!-- 应用排行 -->
    <section class="memory-list">
      <div class="memory-row memory-head">
        <span class="col-check">
          <Checkbox
            :checked="allSelected"
            :indeterminate="someSelected"
            :disabled="apps.length === 0"
            @change="(event) => toggleAll(event.target.checked ?? false)"
          />
        </span>
        <span class="col-icon"></span>
        <span class="col-name">应用</span>
        <span class="col-processes">进程数</span>
        <span class="col-memory">内存</span>
      </div>

      <div class="memory-scroll">
        <div
          v-for="app in apps"
          :key="app.id"
          class="memory-row"
          :class="{ selected: selectedIds.includes(app.id) }"
        >
          <span class="col-check">
            <Checkbox
              :checked="selectedIds.includes(app.id)"
              @change="
                (event) => toggleApp(app.id, event.target.checked ?? false)
              "
            />
          </span>
          <span class="col-icon">
            <AppIcon :icon="app.icon" :name="app.name" :size="28" />
          </span>
          <div class="col-name app-name-cell">
            <span class="memory-app-name" :title="app.name">
              {{ app.name }}
              <em v-if="app.kind === 'process'" class="process-tag">进程</em>
            </span>
            <span class="memory-app-exe mono" :title="app.exe">{{ app.exe || '—' }}</span>
          </div>
          <span class="col-processes">{{ app.process_count }}</span>
          <span class="col-memory">{{ formatMemory(app.memory) }}</span>
        </div>

        <div v-if="apps.length === 0" class="memory-empty">
          {{
            cleaner.sampling || cleaner.memory
              ? '暂无按应用聚合的内存数据。'
              : '正在采样内存占用…'
          }}
        </div>
      </div>

      <div class="memory-actions">
        <span class="selected-summary">
          已选 {{ selectedApps.length }} 项<template v-if="selectedApps.length">
            · {{ selectedProcesses }} 个进程 · {{ formatMemory(selectedMemory) }}</template
          >
        </span>
        <div class="action-buttons">
          <button
            type="button"
            class="btn btn-secondary"
            :disabled="selectedApps.length === 0 || cleaner.quitting"
            :title="selectedApps.length === 0 ? '先勾选要结束的应用' : '优雅退出，3 秒后仍存活可再强制结束'"
            @click="requestQuit"
          >
            {{ cleaner.quitting ? '处理中…' : '结束进程' }}
          </button>
          <button
            type="button"
            class="btn btn-danger-ghost"
            :disabled="cleaner.releasing"
            title="清工作集 / 待机列表（Windows）或 purge（macOS），需要一次系统授权"
            @click="requestRelease"
          >
            {{ cleaner.releasing ? '释放中…' : '深度释放' }}
          </button>
        </div>
      </div>
    </section>
  </div>
</template>

<style scoped>
.memory-panel {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 10px 20px 16px;
  overflow: hidden;
}

/* ===== 总览卡 ===== */
.memory-overview {
  flex-shrink: 0;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  padding: 14px;
}
.overview-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}
.overview-titles {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.overview-label {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}
.overview-sub {
  font-size: 11px;
  color: var(--text-muted);
}
.overview-main {
  display: flex;
  align-items: center;
  gap: 14px;
  margin: 10px 0 8px;
}
.overview-percent {
  flex-shrink: 0;
  font-size: 30px;
  font-weight: 700;
  line-height: 1;
  font-variant-numeric: tabular-nums;
}
.overview-bar {
  flex: 1;
  height: 8px;
  border-radius: 4px;
  background: var(--bg-tag);
  overflow: hidden;
}
.overview-fill {
  height: 100%;
  border-radius: 4px;
  transition: width 0.4s ease, background 0.3s ease;
}
.overview-facts {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 22px;
  font-size: 12px;
  color: var(--text-muted);
}
.overview-facts strong {
  color: var(--text-secondary);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

/* 占用率 / 进度条颜色阈值 */
.level-primary {
  color: var(--primary);
}
.level-primary.overview-fill {
  background: var(--primary);
}
.level-warn {
  color: var(--warning);
}
.level-warn.overview-fill {
  background: var(--warning);
}
.level-danger {
  color: var(--danger);
}
.level-danger.overview-fill {
  background: var(--danger);
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

.release-msg {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-top: 10px;
  padding: 8px 12px;
  border: 1px solid rgba(16, 185, 129, 0.3);
  border-radius: 6px;
  background: rgba(16, 185, 129, 0.06);
  font-size: 12px;
  color: var(--success);
}
.release-msg-muted {
  border-color: var(--border-color);
  background: var(--bg-secondary);
  color: var(--text-secondary);
}
.release-title {
  font-weight: 500;
}
.release-note {
  font-size: 11px;
  color: var(--text-muted);
}

/* ===== 应用排行 ===== */
.memory-list {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  overflow: hidden;
}
.memory-row {
  display: grid;
  grid-template-columns: 22px 30px minmax(0, 1fr) 64px 92px;
  align-items: center;
  gap: 8px;
  height: 44px;
  padding: 0 12px;
  border-bottom: 1px solid var(--border-color);
  font-size: 12px;
  color: var(--text-secondary);
}
.memory-row:last-child {
  border-bottom: none;
}
.memory-head {
  flex-shrink: 0;
  height: 32px;
  background: var(--bg-secondary);
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 600;
}
.memory-head .col-check {
  display: flex;
  align-items: center;
}
.memory-head .col-name,
.memory-head .col-memory {
  font-weight: 600;
}
.memory-head .col-processes,
.memory-head .col-memory {
  text-align: right;
}
.memory-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
.memory-scroll .memory-row:not(.memory-head):hover {
  background: var(--bg-hover);
}
.memory-scroll .memory-row.selected {
  background: var(--bg-active);
}
.memory-scroll .memory-row.selected .memory-app-name {
  color: var(--text-active);
}
.col-check {
  display: flex;
  align-items: center;
  justify-content: center;
}
.col-icon {
  display: flex;
  align-items: center;
}
.app-name-cell {
  display: flex;
  flex-direction: column;
  min-width: 0;
  line-height: 1.35;
}
.memory-app-name {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.process-tag {
  margin-left: 6px;
  padding: 0 4px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  font-size: 10px;
  font-style: normal;
  font-weight: 400;
  color: var(--text-muted);
}
.memory-app-exe {
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.col-processes {
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.col-memory {
  text-align: right;
  font-weight: 600;
  color: var(--text-primary);
  font-variant-numeric: tabular-nums;
}
.memory-empty {
  padding: 28px 12px;
  font-size: 13px;
  color: var(--text-muted);
  text-align: center;
}

/* 底部固定操作条 */
.memory-actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 12px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-secondary);
}
.selected-summary {
  font-size: 12px;
  color: var(--text-muted);
}
.action-buttons {
  display: flex;
  gap: 8px;
  margin-left: auto;
}
.action-buttons .btn {
  height: 28px;
  padding: 0 12px;
  font-size: 12px;
}
.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
</style>
