<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Checkbox, Input, Modal, message } from 'ant-design-vue'
import { SearchOutlined } from '@ant-design/icons-vue'
import type { CleanerApi } from '@/composables/useCleaner'
import type {
  InstalledApp,
  LeftoverItem,
  RunningState,
  UninstallOutcome,
} from '@/types/cleaner'
import { safeGet, safeSet } from '@/composables/safeStorage'
import AppIcon from './AppIcon.vue'
import {
  confidenceClass,
  confidenceLabel,
  formatBytes,
  leftoverKindLabel,
} from './format'

/**
 * 软件卸载页签：搜索 / 排序 / 列表 + 卸载流程。
 *
 * 卸载确认 → 执行 → 成功后自动进入残留扫描；残留弹窗按「文件系统 / 注册表」分组，
 * 组级全选，默认勾选后端标记为 selected（high 置信）的条目，删除后逐条展示结果。
 */
const props = defineProps<{
  cleaner: CleanerApi
}>()

const cleaner = props.cleaner

// ============================ 搜索 / 排序 ============================

type SortBy = 'name' | 'size' | 'install_date'
type SortDir = 'asc' | 'desc'

const SORT_STORAGE_KEY = 'ws-workbench:cleaner:sort'

const SORT_OPTIONS: Array<{ value: SortBy; label: string }> = [
  { value: 'name', label: '名称' },
  { value: 'size', label: '大小' },
  { value: 'install_date', label: '安装日期' },
]

const search = ref('')
const sort = ref<{ by: SortBy; dir: SortDir }>({ by: 'name', dir: 'asc' })

onMounted(async () => {
  const raw = await safeGet(SORT_STORAGE_KEY)
  if (!raw) return
  try {
    const parsed = JSON.parse(raw) as { by?: SortBy; dir?: SortDir }
    const by = parsed.by
    if (by !== 'name' && by !== 'size' && by !== 'install_date') return
    const dir = parsed.dir === 'desc' ? 'desc' : 'asc'
    sort.value = { by, dir }
  } catch {
    // 历史数据坏了就按默认排序，不打断页面
  }
})

function persistSort() {
  void safeSet(SORT_STORAGE_KEY, JSON.stringify(sort.value))
}

/** 同一列再点一次切升降序；换列时名称升序、大小/日期降序（先看最大的） */
function toggleSort(by: SortBy) {
  if (sort.value.by === by) {
    sort.value = { by, dir: sort.value.dir === 'asc' ? 'desc' : 'asc' }
  } else {
    sort.value = { by, dir: by === 'name' ? 'asc' : 'desc' }
  }
  persistSort()
}

function sourceLabel(app: InstalledApp): string {
  if (app.source === 'registry') return '注册表'
  if (app.source === 'applications') return '应用目录'
  if (app.source === 'system') return '系统应用'
  return app.source
}

const filteredApps = computed<InstalledApp[]>(() => {
  const keyword = search.value.trim().toLowerCase()
  const list = keyword
    ? cleaner.apps.filter((app) =>
        [app.name, app.publisher, app.install_location]
          .join('\n')
          .toLowerCase()
          .includes(keyword),
      )
    : [...cleaner.apps]
  const direction = sort.value.dir === 'asc' ? 1 : -1
  return list.sort((left, right) => {
    // 安装日期为空的一律排最后，不受升降序影响
    if (sort.value.by === 'install_date') {
      const leftEmpty = !left.install_date
      const rightEmpty = !right.install_date
      if (leftEmpty !== rightEmpty) return leftEmpty ? 1 : -1
    }
    switch (sort.value.by) {
      case 'size':
        return (left.size_bytes - right.size_bytes) * direction
      case 'install_date':
        return left.install_date.localeCompare(right.install_date) * direction
      default:
        return (
          left.name.localeCompare(right.name, 'zh-Hans-CN', { sensitivity: 'base' }) *
          direction
        )
    }
  })
})

const emptyText = computed(() =>
  search.value.trim() ? '没有匹配的应用，换个关键词试试。' : '没有扫描到已安装的应用。',
)

// ============================ 卸载弹窗 ============================

const uninstallTarget = ref<InstalledApp | null>(null)
/** 没有 UninstallString 的条目走「强制移除」文案 */
const isForce = ref(false)
const commandText = computed(() => {
  const app = uninstallTarget.value
  if (!app) return ''
  if (isForce.value) {
    return '移除注册表条目与可识别的残留（仅移除应用本体，不清理 COM 组件、服务与驱动）'
  }
  if (app.uninstall_kind === 'msi') {
    return 'MsiExec.exe /X —— 调用 Windows Installer 卸载'
  }
  if (app.uninstall_kind === 'exe') {
    return '运行应用自带的卸载程序（UninstallString）'
  }
  if (app.uninstall_kind === 'app_bundle') {
    return `先退出运行中的实例，再把「${app.install_location || app.name}」移入废纸篓`
  }
  return '调用系统卸载入口'
})

function normalizePath(value: string): string {
  return (value ?? '')
    .trim()
    .replace(/\\/g, '/')
    .replace(/\/+$/, '')
    .toLowerCase()
}

async function openUninstall(app: InstalledApp) {
  uninstallTarget.value = app
  isForce.value = app.uninstall_kind === 'force'
  cleaner.resetUninstallOutcome()
}

function closeUninstall() {
  if (cleaner.uninstalling) return
  uninstallTarget.value = null
  cleaner.resetUninstallOutcome()
}

/** 运行中应用的二次确认弹窗 */
const runningPrompt = ref<{ app: InstalledApp; state: RunningState } | null>(null)

/** 二次弹窗里的进程名摘要：最多 3 个，超出加「等」 */
const runningSummary = computed(() => {
  const state = runningPrompt.value?.state
  if (!state || state.names.length === 0) return ''
  const names = state.names.slice(0, 3).join('、')
  return `（${names}${state.names.length > 3 ? ' 等' : ''}）`
})

async function confirmUninstall() {
  const app = uninstallTarget.value
  if (!app || cleaner.uninstalling || cleaner.checkingRunning) return
  // 先只读检测一次；检测失败按未运行处理，不阻断卸载
  const state = await cleaner.checkRunning(app.id)
  if (state?.running) {
    runningPrompt.value = { app, state }
    return
  }
  await runUninstall(app, false)
}

async function runUninstall(
  app: InstalledApp,
  quitRunning: boolean,
): Promise<boolean> {
  const outcome = await cleaner.uninstall(app.id, quitRunning)
  if (!outcome) {
    message.error(cleaner.actionError || '卸载失败')
    return false
  }
  if (!outcome.ok) return false // 失败结果留在弹窗里展示（命令 / 退出码 / 错误）
  message.success(`「${app.name}」已卸载`)
  uninstallTarget.value = null
  // 先收起二次确认弹窗，避免残留扫描期间它还盖在最上层
  runningPrompt.value = null
  cleaner.resetUninstallOutcome()
  // 设计 4.2：成功后自动进入残留扫描
  await openLeftovers(app, outcome)
  return true
}

function cancelRunningPrompt() {
  if (cleaner.uninstalling) return
  runningPrompt.value = null
}

async function confirmRunningPrompt() {
  const prompt = runningPrompt.value
  if (!prompt || cleaner.uninstalling) return
  // 保持弹窗可见，让按钮文案显示「正在结束进程…」
  await runUninstall(prompt.app, true)
  runningPrompt.value = null
}

// ============================ 残留弹窗 ============================

const leftoversOpen = ref(false)
const leftoversTarget = ref<InstalledApp | null>(null)
const lastUninstall = ref<{ name: string; outcome: UninstallOutcome } | null>(null)
/** path -> 是否勾选；用对象而不是 Set，方便模板里直接判断 */
const selection = ref<Record<string, boolean>>({})

const deleteResults = ref<Array<{ path: string; ok: boolean; error: string }>>([])

const deleteSummary = computed(() => {
  const ok = deleteResults.value.filter((item) => item.ok).length
  return { ok, fail: deleteResults.value.length - ok }
})

/** 安装收据本版本只读展示，不参与删除（设计 4.3） */
function isReadonly(item: LeftoverItem): boolean {
  return item.kind === 'receipt'
}

async function openLeftovers(app: InstalledApp, outcome: UninstallOutcome | null = null) {
  leftoversTarget.value = app
  lastUninstall.value = outcome ? { name: app.name, outcome } : null
  leftoversOpen.value = true
  selection.value = {}
  deleteResults.value = []
  cleaner.resetDeleteOutcome()
  await runScanLeftovers()
}

async function runScanLeftovers() {
  const app = leftoversTarget.value
  if (!app) return
  deleteResults.value = []
  await cleaner.scanLeftovers(app.id)
  const next: Record<string, boolean> = {}
  for (const item of cleaner.leftovers) {
    if (!isReadonly(item) && item.selected) next[item.path] = true
  }
  selection.value = next
}

/** 删除成功后重扫：列表按最新磁盘状态刷新，但保留本次逐条删除结果 */
async function refreshLeftovers() {
  const app = leftoversTarget.value
  if (!app) return
  // 重扫失败时保留删除后的列表与本次逐条结果，只显示错误条 + 重试
  const before = [...cleaner.leftovers]
  const failed = new Set(
    deleteResults.value.filter((result) => !result.ok).map((result) => result.path),
  )
  await cleaner.scanLeftovers(app.id)
  if (cleaner.leftoversError) {
    cleaner.leftovers = before
    return
  }
  const next: Record<string, boolean> = {}
  for (const item of cleaner.leftovers) {
    // 本次删除失败的条目保持未勾选，避免重扫后又默认选上
    if (!isReadonly(item) && item.selected && !failed.has(item.path)) {
      next[item.path] = true
    }
  }
  selection.value = next
}

function closeLeftovers() {
  if (cleaner.deleting) return
  leftoversOpen.value = false
  leftoversTarget.value = null
  lastUninstall.value = null
  selection.value = {}
  deleteResults.value = []
  cleaner.resetDeleteOutcome()
}

/** 分组成「文件系统 / 注册表」两组，空组不显示 */
const groupedLeftovers = computed(() => {
  const files = cleaner.leftovers.filter((item) => item.kind !== 'registry')
  const registry = cleaner.leftovers.filter((item) => item.kind === 'registry')
  return [
    { key: 'filesystem', label: '文件系统', items: files },
    { key: 'registry', label: '注册表', items: registry },
  ].filter((group) => group.items.length > 0)
})

function selectableItems(group: { items: LeftoverItem[] }): LeftoverItem[] {
  return group.items.filter((item) => !isReadonly(item))
}

function groupSelectedCount(group: { items: LeftoverItem[] }): number {
  return selectableItems(group).filter((item) => selection.value[item.path]).length
}

function groupSize(group: { items: LeftoverItem[] }): number {
  return group.items.reduce((sum, item) => sum + item.size_bytes, 0)
}

function groupChecked(group: { items: LeftoverItem[] }): boolean {
  const total = selectableItems(group).length
  return total > 0 && groupSelectedCount(group) === total
}

function groupIndeterminate(group: { items: LeftoverItem[] }): boolean {
  const count = groupSelectedCount(group)
  return count > 0 && count < selectableItems(group).length
}

function toggleGroup(group: { items: LeftoverItem[] }, checked: boolean) {
  const next = { ...selection.value }
  for (const item of selectableItems(group)) next[item.path] = checked
  selection.value = next
}

function toggleItem(item: LeftoverItem, checked: boolean) {
  selection.value = { ...selection.value, [item.path]: checked }
}

const selectedPaths = computed(() =>
  cleaner.leftovers
    .filter((item) => !isReadonly(item) && selection.value[item.path])
    .map((item) => item.path),
)

const selectedSize = computed(() =>
  cleaner.leftovers
    .filter((item) => selection.value[item.path])
    .reduce((sum, item) => sum + item.size_bytes, 0),
)

const selectedElevationCount = computed(
  () =>
    cleaner.leftovers.filter(
      (item) => selection.value[item.path] && item.requires_elevation,
    ).length,
)

/** 删除前再确认一次（设计 6：破坏性操作全部二次确认） */
function requestDeleteLeftovers() {
  const app = leftoversTarget.value
  const paths = [...selectedPaths.value]
  if (!app || paths.length === 0 || cleaner.deleting) return
  const elevation = selectedElevationCount.value
  Modal.confirm({
    title: `删除 ${paths.length} 项残留`,
    content:
      elevation > 0
        ? `将把 ${paths.length} 项移入废纸篓；其中 ${elevation} 项需要一次系统授权，提权删除不经过废纸篓（永久删除），取消授权时这些条目不会执行。`
        : `将把 ${paths.length} 项移入废纸篓，不会永久删除。确定继续？`,
    okText: '移入废纸篓',
    okType: 'danger',
    cancelText: '取消',
    async onOk() {
      const current = leftoversTarget.value
      if (!current) return
      const outcome = await cleaner.deleteLeftovers(current.id, paths)
      if (!outcome) {
        message.error(cleaner.actionError || '删除失败')
        return
      }
      const failed = new Map(outcome.failed.map((item) => [item.path, item.error]))
      deleteResults.value = paths.map((path) => {
        const error = failed.get(path)
        return error === undefined
          ? { path, ok: true, error: '' }
          : { path, ok: false, error }
      })
      // 失败条目保留在列表里但取消勾选，方便用户二次确认后再试
      const next = { ...selection.value }
      for (const path of paths) {
        if (failed.has(path)) next[path] = false
        else delete next[path]
      }
      selection.value = next
      const okCount = paths.length - outcome.failed.length
      if (outcome.failed.length === 0) {
        message.success(`已把 ${okCount} 项移入废纸篓`)
      } else if (
        outcome.failed.every((item) => /取消|cancel/i.test(item.error))
      ) {
        // 提权取消按未执行处理，不红字报错
        message.info('已取消系统授权，未做任何修改')
      } else {
        message.warning(`成功 ${okCount} 项，失败 ${outcome.failed.length} 项`)
      }
      // 成功删掉的条目重新扫描后会消失；失败条目仍留在列表里
      if (okCount > 0) await refreshLeftovers()
    },
  })
}
</script>

<template>
  <div class="apps-panel">
    <!-- 工具条：搜索 / 排序 / 数量 -->
    <div class="apps-toolbar">
      <Input
        v-model:value="search"
        class="apps-search"
        placeholder="搜索名称、发布者或安装位置"
        allow-clear
      >
        <template #prefix>
          <SearchOutlined />
        </template>
      </Input>
      <div class="sort-group" role="group" aria-label="排序方式">
        <button
          v-for="option in SORT_OPTIONS"
          :key="option.value"
          type="button"
          class="sort-btn"
          :class="{ active: sort.by === option.value }"
          :aria-pressed="sort.by === option.value"
          :title="`按${option.label}排序`"
          @click="toggleSort(option.value)"
        >
          {{ option.label }}
          <span v-if="sort.by === option.value" class="sort-arrow">{{
            sort.dir === 'asc' ? '↑' : '↓'
          }}</span>
        </button>
      </div>
      <span class="apps-count">
        共 {{ filteredApps.length }} 项<template v-if="search.trim()">
          / {{ cleaner.apps.length }}</template
        >
      </span>
    </div>

    <!-- 扫描进行中：文字 + 细进度条 -->
    <div v-if="cleaner.appsLoading" class="apps-progress">
      <span class="apps-progress-text">
        {{ cleaner.progress?.message || '正在扫描已安装应用…' }}
      </span>
      <span v-if="cleaner.progress && cleaner.progress.total > 0" class="apps-progress-count">
        {{ cleaner.progress.current }} / {{ cleaner.progress.total }}
      </span>
      <div class="progress-track">
        <div
          class="progress-fill"
          :class="{ indeterminate: !cleaner.progress || cleaner.progress.total <= 0 }"
          :style="
            cleaner.progress && cleaner.progress.total > 0
              ? { width: cleaner.scanPercent + '%' }
              : undefined
          "
        ></div>
      </div>
    </div>

    <!-- 错误条：保留已有数据，只提示失败 -->
    <p v-if="cleaner.appsError" class="error-bar">
      <span>{{ cleaner.appsError }}</span>
      <button class="btn btn-secondary btn-xs" @click="cleaner.loadApps(false)">重试</button>
    </p>

    <!-- 首次加载占位 -->
    <div v-if="cleaner.appsLoading && cleaner.apps.length === 0" class="apps-empty">
      首次扫描需要枚举注册表 / /Applications 并统计大小，请稍候…
    </div>
    <div v-else-if="filteredApps.length === 0" class="apps-empty">
      {{ emptyText }}
    </div>

    <!-- 列表 -->
    <div v-else class="apps-scroll">
      <div class="apps-table">
        <div class="apps-row apps-header">
          <span class="col-app">应用</span>
          <span class="col-version">版本</span>
          <span class="col-publisher">发布者</span>
          <span class="col-size">大小</span>
          <span class="col-date">安装日期</span>
          <span class="col-path">安装位置</span>
          <span class="col-action">操作</span>
        </div>
        <div
          v-for="app in filteredApps"
          :key="app.id"
          class="apps-row"
          :title="app.detail || undefined"
        >
          <div class="col-app app-cell">
            <AppIcon :icon="app.icon" :name="app.name" :size="28" />
            <div class="app-meta">
              <span class="app-name" :title="app.name">{{ app.name }}</span>
              <span class="app-source">{{ sourceLabel(app) }}</span>
            </div>
          </div>
          <span class="col-version" :title="app.version">{{ app.version || '—' }}</span>
          <span class="col-publisher" :title="app.publisher">{{ app.publisher || '—' }}</span>
          <span class="col-size">{{ formatBytes(app.size_bytes) }}</span>
          <span class="col-date">{{ app.install_date || '—' }}</span>
          <span class="col-path mono" :title="app.install_location">{{
            app.install_location || '—'
          }}</span>
          <div class="col-action">
            <span v-if="!app.can_uninstall" class="tag tag-muted" title="系统应用，不能卸载">
              系统应用
            </span>
            <button
              v-else-if="app.uninstall_kind === 'force'"
              type="button"
              class="row-btn row-btn-danger"
              :disabled="cleaner.uninstalling"
              title="没有卸载程序，强制移除应用本体与可识别残留"
              @click="openUninstall(app)"
            >
              强制移除
            </button>
            <button
              v-else
              type="button"
              class="row-btn"
              :disabled="cleaner.uninstalling"
              @click="openUninstall(app)"
            >
              卸载
            </button>
          </div>
        </div>
      </div>
    </div>

    <!-- 卸载确认 -->
    <Modal
      v-if="uninstallTarget"
      :open="true"
      :footer="null"
      :width="580"
      :mask-closable="false"
      :title="isForce ? '强制移除应用' : '卸载应用'"
      @cancel="closeUninstall"
    >
      <div class="uninstall-dialog">
        <div class="dialog-app">
          <AppIcon :icon="uninstallTarget.icon" :name="uninstallTarget.name" :size="40" />
          <div class="dialog-app-meta">
            <span class="dialog-app-name">{{ uninstallTarget.name }}</span>
            <span class="dialog-app-sub">
              {{
                [
                  uninstallTarget.version ? `v${uninstallTarget.version}` : '',
                  uninstallTarget.publisher,
                ]
                  .filter(Boolean)
                  .join(' · ') || '—'
              }}
            </span>
          </div>
        </div>

        <div class="dialog-block">
          <span class="dialog-label">将执行</span>
          <p class="dialog-command mono">{{ commandText }}</p>
          <p class="dialog-hint">实际执行的命令与退出码会在完成后显示。</p>
          <p v-if="uninstallTarget.detail" class="dialog-hint">
            备注：{{ uninstallTarget.detail }}
          </p>
        </div>

        <p v-if="isForce" class="dialog-warn">
          强制移除只删除应用本体与可识别残留，不清理 COM 组件、服务与驱动。
        </p>

        <!-- 执行结果：命令 / 退出码 / 错误 -->
        <div
          v-if="cleaner.uninstallOutcome"
          class="outcome-box"
          :class="cleaner.uninstallOutcome.ok ? 'outcome-ok' : 'outcome-fail'"
        >
          <p class="outcome-title">
            {{ cleaner.uninstallOutcome.ok ? '卸载完成' : '卸载失败' }}
          </p>
          <p v-if="cleaner.uninstallOutcome.command" class="outcome-command mono">
            {{ cleaner.uninstallOutcome.command }}
          </p>
          <p class="outcome-message">{{ cleaner.uninstallOutcome.message }}</p>
          <p v-if="cleaner.uninstallOutcome.exit_code !== null" class="outcome-sub">
            退出码 {{ cleaner.uninstallOutcome.exit_code
            }}{{ cleaner.uninstallOutcome.elevated ? ' · 已提权' : '' }}
          </p>
        </div>

        <div class="dialog-actions">
          <button
            class="btn btn-secondary"
            :disabled="cleaner.uninstalling"
            @click="closeUninstall"
          >
            {{ cleaner.uninstallOutcome?.ok ? '关闭' : '取消' }}
          </button>
          <button
            v-if="!cleaner.uninstallOutcome"
            type="button"
            class="btn"
            :class="isForce ? 'btn-danger' : 'btn-primary'"
            :disabled="cleaner.uninstalling || cleaner.checkingRunning"
            @click="confirmUninstall"
          >
            {{
              cleaner.checkingRunning
                ? '检查运行状态…'
                : cleaner.uninstalling
                  ? isForce
                    ? '移除中…'
                    : '卸载中…'
                  : isForce
                    ? '确认强制移除'
                    : '确认卸载'
            }}
          </button>
          <button
            v-else-if="cleaner.uninstallOutcome.ok"
            type="button"
            class="btn btn-primary"
            @click="openLeftovers(uninstallTarget!, cleaner.uninstallOutcome ?? undefined)"
          >
            扫描残留
          </button>
          <button
            v-else
            type="button"
            class="btn btn-secondary"
            :disabled="cleaner.uninstalling"
            @click="confirmUninstall"
          >
            重试
          </button>
        </div>

        <p v-if="!isForce && !cleaner.uninstallOutcome" class="dialog-note">
          卸载程序可能弹出自己的窗口，请按其提示完成；失败时也可以到系统「应用和功能」里卸载。
        </p>
      </div>
    </Modal>

    <!-- 运行中二次确认：结束进程并卸载 / 取消 -->
    <Modal
      v-if="runningPrompt"
      :open="true"
      :footer="null"
      :width="520"
      :mask-closable="false"
      title="应用正在运行"
      @cancel="cancelRunningPrompt"
    >
      <div class="uninstall-dialog">
        <p class="dialog-hint">
          「{{ runningPrompt.app.name }}」有
          {{ runningPrompt.state.processes }} 个进程正在运行{{ runningSummary }}。卸载前会先结束这些进程；未能退出时本次卸载会中止，请手动退出后重试。
        </p>
        <div class="dialog-actions">
          <button
            class="btn btn-secondary"
            :disabled="cleaner.uninstalling"
            @click="cancelRunningPrompt"
          >
            取消
          </button>
          <button
            class="btn btn-primary"
            :disabled="cleaner.uninstalling"
            @click="confirmRunningPrompt"
          >
            {{ cleaner.uninstalling ? '正在结束进程…' : '结束进程并卸载' }}
          </button>
        </div>
      </div>
    </Modal>

    <!-- 残留清理 -->
    <Modal
      v-if="leftoversOpen"
      :open="true"
      :footer="null"
      :width="760"
      :mask-closable="false"
      title="清理残留"
      @cancel="closeLeftovers"
    >
      <div class="leftover-dialog">
        <div class="leftover-head">
          <div class="leftover-head-main">
            <span class="leftover-app-name">{{ leftoversTarget?.name }}</span>
            <p class="leftover-sub">
              勾选要清理的残留条目：普通条目移入废纸篓，提权条目会合并成一次系统授权（不经过废纸篓）。
            </p>
          </div>
          <button
            type="button"
            class="btn btn-secondary btn-xs"
            :disabled="cleaner.leftoversLoading"
            @click="runScanLeftovers"
          >
            {{ cleaner.leftoversLoading ? '扫描中…' : '重新扫描' }}
          </button>
        </div>

        <div
          v-if="lastUninstall"
          class="outcome-box"
          :class="lastUninstall.outcome.ok ? 'outcome-ok' : 'outcome-fail'"
        >
          <p class="outcome-title">
            {{ lastUninstall.outcome.ok ? '卸载完成，已扫描残留' : '卸载失败' }}
          </p>
          <p v-if="lastUninstall.outcome.command" class="outcome-command mono">
            {{ lastUninstall.outcome.command }}
          </p>
          <p class="outcome-message">{{ lastUninstall.outcome.message }}</p>
        </div>

        <p v-if="cleaner.leftoversError" class="error-bar">
          <span>{{ cleaner.leftoversError }}</span>
          <button class="btn btn-secondary btn-xs" @click="runScanLeftovers">重试</button>
        </p>

        <div v-if="cleaner.leftoversLoading" class="leftover-empty">
          正在扫描残留文件和注册表项…
        </div>
        <div v-else-if="cleaner.leftovers.length === 0" class="leftover-empty">
          没有发现残留文件或注册表项。
        </div>
        <div v-else class="leftover-body">
          <div v-for="group in groupedLeftovers" :key="group.key" class="leftover-group">
            <div class="group-head">
              <Checkbox
                :checked="groupChecked(group)"
                :indeterminate="groupIndeterminate(group)"
                :disabled="selectableItems(group).length === 0"
                @change="
                  (event) => toggleGroup(group, event.target.checked ?? false)
                "
              />
              <span class="group-title">{{ group.label }}</span>
              <span class="group-count">{{ group.items.length }} 项</span>
              <span v-if="groupSize(group) > 0" class="group-size">
                {{ formatBytes(groupSize(group)) }}
              </span>
            </div>
            <div
              v-for="item in group.items"
              :key="item.path"
              class="leftover-item"
              :class="{ 'leftover-item-readonly': isReadonly(item) }"
            >
              <Checkbox
                :checked="!!selection[item.path]"
                :disabled="isReadonly(item)"
                @change="
                  (event) => toggleItem(item, event.target.checked ?? false)
                "
              />
              <span class="leftover-kind">{{ leftoverKindLabel(item.kind) }}</span>
              <span class="leftover-path mono" :title="item.path">{{ item.path }}</span>
              <span class="leftover-size">{{ formatBytes(item.size_bytes) }}</span>
              <span class="tag" :class="confidenceClass(item.confidence)">
                {{ confidenceLabel(item.confidence) }}
              </span>
              <span v-if="item.requires_elevation" class="tag tag-warn">需授权</span>
              <span v-if="isReadonly(item)" class="tag tag-muted" title="本版本只读展示，不自动删除">
                只读
              </span>
            </div>
          </div>

          <!-- 删除结果：逐条展示成功 / 失败 -->
          <div v-if="deleteResults.length" class="delete-results">
            <p class="results-title">
              删除结果：成功 {{ deleteSummary.ok }} 项<template v-if="deleteSummary.fail">，失败
                {{ deleteSummary.fail }} 项</template>
            </p>
            <div
              v-for="result in deleteResults"
              :key="result.path"
              class="result-item"
              :class="{ failed: !result.ok }"
            >
              <span class="result-icon">{{ result.ok ? '✓' : '✗' }}</span>
              <span class="result-path mono" :title="result.path">{{ result.path }}</span>
              <span v-if="!result.ok" class="result-error">{{ result.error }}</span>
            </div>
          </div>
        </div>

        <div class="leftover-actions">
          <span class="selected-summary">
            已选 {{ selectedPaths.length }} 项<template v-if="selectedSize > 0">
              · {{ formatBytes(selectedSize) }}</template
            ><template v-if="selectedElevationCount > 0">
              · {{ selectedElevationCount }} 项需授权</template
            >
          </span>
          <div class="leftover-buttons">
            <button
              type="button"
              class="btn btn-secondary"
              :disabled="cleaner.deleting"
              @click="closeLeftovers"
            >
              关闭
            </button>
            <button
              type="button"
              class="btn btn-primary"
              :disabled="selectedPaths.length === 0 || cleaner.deleting"
              @click="requestDeleteLeftovers"
            >
              {{
                cleaner.deleting
                  ? '删除中…'
                  : `删除 ${selectedPaths.length} 项（进废纸篓）`
              }}
            </button>
          </div>
        </div>
      </div>
    </Modal>
  </div>
</template>

<style scoped>
.apps-panel {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 20px 16px;
  overflow: hidden;
}

/* 工具条 */
.apps-toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 10px;
}
.apps-search {
  width: 300px;
  flex-shrink: 0;
}
.apps-search :deep(.ant-input-prefix) {
  color: var(--text-muted);
}
.sort-group {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}
.sort-btn {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  height: 32px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}
.sort-btn:hover {
  border-color: var(--primary);
  color: var(--primary);
}
.sort-btn.active {
  border-color: var(--primary);
  background: var(--bg-active);
  color: var(--primary);
}
.sort-arrow {
  font-size: 11px;
  line-height: 1;
}
.apps-count {
  margin-left: auto;
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
}

/* 扫描进度 */
.apps-progress {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-secondary);
}
.apps-progress-text {
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.apps-progress-count {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}
.progress-track {
  flex: 1;
  min-width: 80px;
  height: 4px;
  border-radius: 2px;
  background: var(--bg-tag);
  overflow: hidden;
}
.progress-fill {
  height: 100%;
  border-radius: 2px;
  background: var(--primary);
  transition: width 0.2s ease;
}
.progress-fill.indeterminate {
  width: 35%;
  animation: cleaner-progress-slide 1.1s ease-in-out infinite;
}
@keyframes cleaner-progress-slide {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(300%);
  }
}

/* 错误条 */
.error-bar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 10px;
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

/* 空态 / 加载态 */
.apps-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px 12px;
  font-size: 13px;
  color: var(--text-muted);
  text-align: center;
}

/* 列表 */
.apps-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
}
.apps-table {
  /* 7 列 + 间距的最小宽度；窗口更窄时横向滚动，不把内容挤变形 */
  min-width: 780px;
}
.apps-row {
  display: grid;
  grid-template-columns:
    minmax(150px, 1.5fr) 78px minmax(90px, 0.9fr) 78px 92px
    minmax(110px, 1.2fr) 92px;
  align-items: center;
  gap: 10px;
  height: 44px;
  padding: 0 12px;
  border-bottom: 1px solid var(--border-color);
  font-size: 12px;
  color: var(--text-secondary);
}
.apps-row:last-child {
  border-bottom: none;
}
.apps-row:not(.apps-header):hover {
  background: var(--bg-hover);
}
.apps-header {
  position: sticky;
  top: 0;
  z-index: 1;
  height: 32px;
  background: var(--bg-secondary);
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.2px;
}
.app-cell {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.app-meta {
  display: flex;
  flex-direction: column;
  min-width: 0;
  line-height: 1.35;
}
.app-name {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.app-source {
  font-size: 11px;
  color: var(--text-muted);
}
.col-version,
.col-publisher,
.col-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.col-size {
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.col-date {
  font-variant-numeric: tabular-nums;
}
.col-action {
  display: flex;
  justify-content: flex-end;
}
.row-btn {
  height: 24px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 5px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}
.row-btn:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--primary);
}
.row-btn-danger {
  color: var(--danger);
}
.row-btn-danger:hover:not(:disabled) {
  border-color: var(--danger);
  color: var(--danger);
  background: #fef2f2;
}
.row-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.tag {
  display: inline-block;
  font-size: 11px;
  border-radius: 9px;
  padding: 1px 8px;
  white-space: nowrap;
}
.tag-ok {
  color: var(--success);
  background: rgba(16, 185, 129, 0.1);
}
.tag-warn {
  color: var(--warning);
  background: rgba(245, 158, 11, 0.12);
}
.tag-muted {
  color: var(--text-secondary);
  background: var(--bg-tag);
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}

/* 弹窗通用 */
.dialog-app {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 14px;
}
.dialog-app-meta {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.dialog-app-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}
.dialog-app-sub {
  font-size: 12px;
  color: var(--text-muted);
}
.dialog-block {
  margin-bottom: 14px;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
}
.dialog-label {
  display: block;
  margin-bottom: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
}
.dialog-command {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-primary);
  word-break: break-all;
}
.dialog-hint {
  margin-top: 6px;
  font-size: 11px;
  color: var(--text-muted);
}
.dialog-warn {
  margin-bottom: 12px;
  padding: 8px 12px;
  border: 1px solid rgba(245, 158, 11, 0.3);
  border-radius: 6px;
  background: rgba(245, 158, 11, 0.08);
  color: #b45309;
  font-size: 12px;
  line-height: 1.6;
}
.dialog-note {
  margin-top: 12px;
  font-size: 11px;
  line-height: 1.6;
  color: var(--text-muted);
}
.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 4px;
}
.outcome-box {
  margin-bottom: 14px;
  padding: 10px 12px;
  border-radius: 8px;
  border: 1px solid var(--border-color);
  background: var(--bg-secondary);
}
.outcome-ok {
  border-color: rgba(16, 185, 129, 0.35);
  background: rgba(16, 185, 129, 0.06);
}
.outcome-fail {
  border-color: rgba(239, 68, 68, 0.3);
  background: #fef2f2;
}
.outcome-title {
  margin-bottom: 4px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary);
}
.outcome-ok .outcome-title {
  color: var(--success);
}
.outcome-fail .outcome-title {
  color: var(--danger);
}
.outcome-command {
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-secondary);
  word-break: break-all;
}
.outcome-message {
  margin-top: 4px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-secondary);
  word-break: break-word;
}
.outcome-sub {
  margin-top: 4px;
  font-size: 11px;
  color: var(--text-muted);
}

/* 残留弹窗 */
.leftover-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 10px;
}
.leftover-head-main {
  min-width: 0;
}
.leftover-app-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}
.leftover-sub {
  margin-top: 3px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
.leftover-empty {
  padding: 28px 12px;
  font-size: 13px;
  color: var(--text-muted);
  text-align: center;
}
.leftover-body {
  max-height: 46vh;
  overflow-y: auto;
  padding-right: 4px;
}
.leftover-group {
  margin-bottom: 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}
.group-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 12px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-color);
}
.group-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary);
}
.group-count {
  font-size: 11px;
  color: var(--text-muted);
}
.group-size {
  margin-left: auto;
  font-size: 11px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}
.leftover-item {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 34px;
  padding: 4px 12px;
  border-bottom: 1px solid var(--bg-hover);
  font-size: 12px;
}
.leftover-item:last-child {
  border-bottom: none;
}
.leftover-item:hover {
  background: var(--bg-hover);
}
.leftover-item-readonly {
  opacity: 0.72;
}
.leftover-kind {
  flex-shrink: 0;
  width: 64px;
  color: var(--text-muted);
  font-size: 11px;
}
.leftover-path {
  flex: 1;
  min-width: 0;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.leftover-size {
  flex-shrink: 0;
  width: 62px;
  text-align: right;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}
.delete-results {
  margin-top: 4px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}
.results-title {
  padding: 7px 12px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-color);
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary);
}
.result-item {
  display: flex;
  align-items: baseline;
  gap: 6px;
  padding: 4px 12px;
  font-size: 11px;
  color: var(--text-secondary);
  border-bottom: 1px solid var(--bg-hover);
}
.result-item:last-child {
  border-bottom: none;
}
.result-item.failed {
  background: #fef2f2;
}
.result-icon {
  flex-shrink: 0;
  color: var(--success);
}
.result-item.failed .result-icon {
  color: var(--danger);
}
.result-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.result-error {
  flex-shrink: 0;
  max-width: 46%;
  color: var(--danger);
  word-break: break-all;
}
.leftover-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid var(--border-color);
}
.selected-summary {
  font-size: 12px;
  color: var(--text-muted);
}
.leftover-buttons {
  display: flex;
  gap: 8px;
  margin-left: auto;
}
</style>
