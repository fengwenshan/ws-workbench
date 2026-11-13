<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import AppIcon from '@/components/cleaner/AppIcon.vue'
import appIcon from '@/assets/app-icon.png'
import { platform } from '@/platform'
import { formatBytes } from '@/components/cleaner/format'
import {
  CLEANER_PROGRESS_EVENT,
  CLEANER_TARGET_CHANGED_EVENT,
  appRunning,
  deleteRelated,
  forceQuitApp,
  relatedFiles,
  uninstallApp,
  uninstallTarget,
} from '@/api/cleaner'
import { errorText } from '@/utils/error'
import type {
  CleanerProgress,
  DeleteOutcome,
  InstalledApp,
  RelatedFileItem,
  RelatedFiles,
} from '@/types/cleaner'
import ForceQuitDialog from './ForceQuitDialog.vue'
import RelatedList from './RelatedList.vue'

/**
 * 软件卸载独立窗口（820×560，由后端 `cleaner_open_uninstall_window` 创建）。
 *
 * 状态机：loading → ready →（运行中则弹强制退出确认）→ quitting → uninstalling
 *        → rescanning → ready(postScan) → deleting → done | error
 *
 * 请求编排顺序与设计 3.1 一致：
 * 1. `cleaner_uninstall_target` 取目标应用；
 * 2. `cleaner_related_files` 扫三组关联文件（同时刷新后端删除白名单）；
 * 3. 点「确认删除」先 `cleaner_app_running`：运行中弹强制退出确认；
 * 4. 「强制退出」→ `cleaner_force_quit`，进程归零后继续；
 * 5. `cleaner_uninstall_app(quitRunning=false)` 执行系统卸载；
 * 6. 卸载成功后自动再扫一次残留：有残留回到 ready（提示「卸载已完成」，
 *    按钮变为「清理残留」）让用户确认；没有残留直接进 done；
 * 7. `cleaner_delete_related` 删除勾选的关联文件，结果留在窗口内展示。
 *
 * 默认只勾缓存/日志/崩溃报告（后端 `DEFAULT_SELECTED_KINDS`），设置、账号、
 * 插件配置等用户数据默认保留；用户可显式切到「彻底删除（含用户数据）」。
 */
type Phase =
  | 'loading'
  | 'ready'
  | 'quitting'
  | 'uninstalling'
  | 'deleting'
  | 'done'
  | 'error'

/** 操作失败后「重试」按钮该重放哪一步 */
type RetrySource = 'force_quit' | 'uninstall' | 'delete' | ''

const phase = ref<Phase>('loading')
const app = ref<InstalledApp | null>(null)
const related = ref<RelatedFiles | null>(null)
/** path -> 是否勾选；只有 true 的键存在 */
const selected = ref<Record<string, boolean>>({})
/** 清单行内的操作失败提示（不关闭窗口，可重试） */
const actionError = ref('')
const actionErrorSource = ref<RetrySource>('')
/** 加载目标/清单失败（整页错误态） */
const fatalError = ref('')
/** 后端 `cleaner-progress` 的文案 */
const progressText = ref('')
const forceQuitOpen = ref(false)
const deleteOutcome = ref<DeleteOutcome | null>(null)
/** 系统卸载已成功、只剩删关联文件：重试时不再走卸载命令 */
const uninstallDone = ref(false)
/** 当前清单是否是「卸载完成后二次扫描」的残留确认页 */
const postScan = ref(false)

const unlisteners: UnlistenFn[] = []

const busy = computed(
  () =>
    phase.value === 'quitting' ||
    phase.value === 'uninstalling' ||
    phase.value === 'deleting',
)

/** Windows 自绘标题栏；macOS / Linux 仍用系统窗口装饰 */
const isWindows = computed(() => platform.is_windows)

/** 标题栏文案：目标应用还没加载出来时退回「卸载」 */
const windowTitle = computed(() => (app.value ? `卸载 ${app.value.name}` : '卸载'))

const allItems = computed(() => related.value?.groups.flatMap((group) => group.items) ?? [])

/**
 * 应用本体行（必须勾选、不可取消）：确认删除一定会执行系统卸载。
 * macOS 目标 id 形如 `mac:/Applications/Foo.app`，对应路径的 app_bundle；
 * Windows 用 install_dir 行。
 */
const primaryPaths = computed<string[]>(() => {
  const target = app.value
  if (!target) return []
  const items = allItems.value
  if (target.id.startsWith('mac:')) {
    const path = target.id.slice(4)
    return items
      .filter((item) => item.kind === 'app_bundle' && item.path === path)
      .map((item) => item.path)
  }
  return items.filter((item) => item.kind === 'install_dir').map((item) => item.path)
})

/** 任何勾选变更都把本体行补回 true，避免绕过行内 UI 漏掉系统卸载目标 */
function withPrimary(next: Record<string, boolean>): Record<string, boolean> {
  for (const path of primaryPaths.value) next[path] = true
  return next
}

function setSelection(next: Record<string, boolean>): void {
  selected.value = withPrimary({ ...next })
}

const lastUsed = computed(() => related.value?.last_used ?? '')

const selectedCount = computed(
  () => allItems.value.filter((item) => !!selected.value[item.path]).length,
)

const selectedPaths = computed(() =>
  allItems.value.filter((item) => !!selected.value[item.path]).map((item) => item.path),
)

const allSelected = computed(
  () => allItems.value.length > 0 && selectedCount.value === allItems.value.length,
)

const partlySelected = computed(
  () => selectedCount.value > 0 && !allSelected.value,
)

/**
 * 默认/安全清理的 kind：缓存、日志、崩溃报告，以及卸载本体。
 * 与后端 `related::DEFAULT_SELECTED_KINDS` 保持一致。
 */
const SAFE_CLEAN_KINDS = new Set(['app_bundle', 'install_dir', 'cache', 'log', 'crash_report'])

function isSafeItem(item: RelatedFileItem): boolean {
  return item.confidence !== 'low' && SAFE_CLEAN_KINDS.has(item.kind)
}

/**
 * 当前勾选是否只包含安全清理项（即「保留用户数据」状态）。
 * 用户手动勾选设置/账号类条目后会自动变为 false，提示条随之为「彻底删除」。
 */
const preserveData = computed(() =>
  allItems.value.every((item) => !selected.value[item.path] || isSafeItem(item)),
)

/** 保留用户数据：只勾缓存/日志/崩溃报告 + 卸载本体 */
function selectSafeOnly(): void {
  const next: Record<string, boolean> = {}
  for (const item of allItems.value) {
    if (isSafeItem(item)) next[item.path] = true
  }
  selected.value = withPrimary(next)
}

/** 彻底删除：连同设置、账号、插件配置等用户数据一起勾选 */
function selectEverything(): void {
  const next: Record<string, boolean> = {}
  for (const item of allItems.value) next[item.path] = true
  selected.value = withPrimary(next)
}

function togglePreserveData(): void {
  if (preserveData.value) selectEverything()
  else selectSafeOnly()
}

const busyText = computed(() => {
  if (phase.value === 'quitting') return '正在强制退出…'
  if (phase.value === 'uninstalling') return '正在卸载…'
  if (phase.value === 'deleting') return '正在删除关联文件…'
  return ''
})

const statusText = computed(() => progressText.value || busyText.value)

const failureRows = computed(() => deleteOutcome.value?.failed ?? [])

const hasFailures = computed(() => failureRows.value.length > 0)

/** 卸载中 / 完成态：顶部只留图标 + 应用名，隐藏「使用情况」和合计大小 */
const compactHead = computed(() => busy.value || phase.value === 'done')

const resultSummary = computed(() => {
  const outcome = deleteOutcome.value
  if (!outcome) return ''
  return `删除 ${outcome.trashed} 项，跳过 ${outcome.skipped} 项，失败 ${outcome.failed.length} 项`
})

/** 合计大小按整数展示（296.0 MB → 296 MB），条目行仍用 formatBytes */
function formatTotal(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return ''
  return formatBytes(bytes).replace(/\.0(?= [A-Z]+$)/, '')
}

// ============================ 加载 ============================

async function load(): Promise<void> {
  phase.value = 'loading'
  fatalError.value = ''
  actionError.value = ''
  actionErrorSource.value = ''
  progressText.value = ''
  deleteOutcome.value = null
  uninstallDone.value = false
  postScan.value = false
  try {
    const target = await uninstallTarget()
    app.value = target
    const files = await relatedFiles(target.id)
    related.value = files
    const next: Record<string, boolean> = {}
    for (const group of files.groups) {
      for (const item of group.items) {
        if (item.selected) next[item.path] = true
      }
    }
    selected.value = withPrimary(next)
    phase.value = 'ready'
  } catch (err) {
    fatalError.value = errorText(err, '读取关联文件失败')
    phase.value = 'error'
  }
}

// ============================ 勾选 ============================

function toggleAll(): void {
  if (allSelected.value) {
    selected.value = withPrimary({})
    return
  }
  const next: Record<string, boolean> = {}
  for (const item of allItems.value) next[item.path] = true
  selected.value = withPrimary(next)
}

// ============================ 卸载编排 ============================

/** 「确认删除」：先只读检测运行状态，运行中先弹强制退出确认 */
async function requestDelete(): Promise<void> {
  if (busy.value || !app.value || selectedPaths.value.length === 0) return
  actionError.value = ''
  actionErrorSource.value = ''
  // 应用本体已经卸载成功（上一轮删除关联文件失败）：只重试剩下的删除
  if (uninstallDone.value) {
    await runDelete()
    return
  }
  let running = false
  try {
    const state = await appRunning(app.value.id)
    running = !!state?.running
  } catch (err) {
    // 进程状态不确定时绝不能继续删：宁可让用户重试，也不能出现「进程还在、文件已删」。
    phase.value = 'ready'
    actionErrorSource.value = ''
    actionError.value = errorText(err, '无法确认应用运行状态，已中止卸载，未删除任何文件')
    return
  }
  if (running) {
    forceQuitOpen.value = true
    return
  }
  await runUninstall()
}

/** 「强制退出」：先正常退出再强杀，后端轮询到相关进程归零才返回 */
async function confirmForceQuit(): Promise<void> {
  const target = app.value
  if (!target || busy.value) return
  forceQuitOpen.value = false
  actionError.value = ''
  actionErrorSource.value = ''
  phase.value = 'quitting'
  progressText.value = '正在退出应用进程…'
  try {
    const outcome = await forceQuitApp(target.id)
    if (!outcome.ok) {
      phase.value = 'ready'
      actionErrorSource.value = 'force_quit'
      actionError.value = outcome.message || `仍有 ${outcome.processes} 个进程未退出`
      return
    }
    await runUninstall()
  } catch (err) {
    phase.value = 'ready'
    actionErrorSource.value = 'force_quit'
    actionError.value = errorText(err, '强制退出失败')
  }
}

/** 系统卸载前再确认一次进程已归零；查询失败也视为不安全。 */
async function ensureNotRunning(): Promise<boolean> {
  const target = app.value
  if (!target) return false
  try {
    const state = await appRunning(target.id)
    if (state?.running) {
      phase.value = 'ready'
      actionErrorSource.value = ''
      actionError.value = `应用仍在运行（${state.processes ?? 0} 个进程），已中止卸载，未删除任何文件`
      return false
    }
    return true
  } catch (err) {
    phase.value = 'ready'
    actionErrorSource.value = ''
    actionError.value = errorText(err, '无法确认应用运行状态，已中止卸载，未删除任何文件')
    return false
  }
}

/** 系统卸载：macOS 移入废纸篓 / Windows 跑自带卸载程序 */
async function runUninstall(): Promise<void> {
  const target = app.value
  // 注意：这里不能再用 busy 判断——强制退出成功后 phase 还是 quitting，
  // 下一跳 runUninstall 必须能接管状态。
  if (!target) return
  if (!(await ensureNotRunning())) return
  actionError.value = ''
  actionErrorSource.value = ''
  phase.value = 'uninstalling'
  progressText.value = '正在卸载应用…'
  try {
    const outcome = await uninstallApp(target.id, false)
    if (!outcome.ok) {
      phase.value = 'ready'
      actionErrorSource.value = 'uninstall'
      actionError.value = [outcome.message || '卸载失败', outcome.command]
        .filter(Boolean)
        .join('\n')
      return
    }
    uninstallDone.value = true
    await rescanAfterUninstall()
  } catch (err) {
    phase.value = 'ready'
    actionErrorSource.value = 'uninstall'
    actionError.value = errorText(err, '卸载失败')
  }
}

/** 卸载完成后重新扫描残留：有残留进二次确认，没有残留直接完成 */
async function rescanAfterUninstall(): Promise<void> {
  const target = app.value
  if (!target) return
  phase.value = 'loading'
  progressText.value = '正在扫描卸载残留…'
  try {
    const files = await relatedFiles(target.id)
    const items = files.groups.flatMap((group) => group.items)
    related.value = files
    progressText.value = ''
    if (items.length === 0) {
      deleteOutcome.value = null
      phase.value = 'done'
      return
    }
    // 保留用户卸载前的勾选（包括明确取消的项）；卸载后新出现的条目才按后端默认置信度勾选
    const previous = selected.value
    const next: Record<string, boolean> = {}
    for (const item of items) {
      const known = Object.prototype.hasOwnProperty.call(previous, item.path)
      if (known ? previous[item.path] : item.selected) next[item.path] = true
    }
    selected.value = withPrimary(next)
    postScan.value = true
    phase.value = 'ready'
  } catch (err) {
    // 重扫失败不阻断：退回原清单，用户仍可按原勾选删除
    progressText.value = ''
    phase.value = 'ready'
    actionErrorSource.value = 'delete'
    actionError.value = errorText(
      err,
      '卸载已完成，但重新扫描残留失败；仍可删除之前勾选的项目',
    )
  }
}

/** 删除勾选的关联文件 */
async function runDelete(): Promise<void> {
  const target = app.value
  if (!target) return
  const paths = [...selectedPaths.value]
  if (paths.length === 0) {
    phase.value = 'done'
    return
  }
  phase.value = 'deleting'
  progressText.value = '正在删除关联文件…'
  try {
    deleteOutcome.value = await deleteRelated(target.id, paths)
    progressText.value = ''
    phase.value = 'done'
  } catch (err) {
    phase.value = 'ready'
    actionErrorSource.value = 'delete'
    actionError.value = errorText(err, '删除关联文件失败')
  }
}

function retryAction(): void {
  const source = actionErrorSource.value
  actionError.value = ''
  actionErrorSource.value = ''
  if (source === 'force_quit') void confirmForceQuit()
  else if (source === 'uninstall') void runUninstall()
  else if (source === 'delete') void runDelete()
}

function dismissError(): void {
  actionError.value = ''
  actionErrorSource.value = ''
}

// ============================ 关闭窗口 ============================

/** 「返回」与完成后的「关闭」：只关这个独立窗口，主窗口不跟随 */
function closeWindow(): void {
  void getCurrentWindow().close()
}

/** 自绘标题栏的最小化按钮 */
function minimizeWindow(): void {
  void getCurrentWindow().minimize()
}

// ============================ 生命周期 ============================

onMounted(async () => {
  try {
    unlisteners.push(
      await listen<CleanerProgress>(CLEANER_PROGRESS_EVENT, (event) => {
        const payload = event.payload
        if (!payload) return
        const task = String(payload.task)
        if (task === 'related_files' || task === 'force_quit' || task === 'uninstall') {
          progressText.value = payload.message || ''
        }
      }),
    )
    unlisteners.push(
      await listen(CLEANER_TARGET_CHANGED_EVENT, () => {
        // 卸载进行中不切换目标，避免把半截操作丢掉
        if (busy.value) return
        void load()
      }),
    )
  } catch {
    // 事件订阅失败不阻断卸载主流程
  }
  await load()
})

onUnmounted(() => {
  unlisteners.forEach((unlisten) => unlisten())
  unlisteners.length = 0
})
</script>

<template>
  <div class="unwin">
    <!-- Windows：自绘标题栏（后端已用 decorations(false) 去掉系统标题栏）。
         和主界面保持一致的 38px 高度、左侧品牌、右侧最小化/关闭。 -->
    <header v-if="isWindows" class="unwin-titlebar">
      <div class="unwin-titlebar-drag" data-tauri-drag-region></div>
      <div class="unwin-titlebar-brand">
        <img class="unwin-titlebar-icon" :src="appIcon" alt="" draggable="false" />
        <span class="unwin-titlebar-name" :title="windowTitle">{{ windowTitle }}</span>
      </div>
      <div class="unwin-titlebar-controls">
        <button
          type="button"
          class="unwin-window-btn"
          title="最小化"
          aria-label="最小化"
          @click="minimizeWindow"
        >
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M2 6h8" />
          </svg>
        </button>
        <button
          type="button"
          class="unwin-window-btn unwin-window-btn-close"
          title="关闭"
          aria-label="关闭"
          @click="closeWindow"
        >
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
          </svg>
        </button>
      </div>
    </header>

    <!-- 顶部：图标 / 名称 / 使用情况 / 合计大小 -->
    <header v-if="app" class="unwin-head">
      <AppIcon
        class="unwin-head-icon"
        :icon="app.icon"
        :name="app.name"
        :size="44"
        :radius="10"
      />
      <div class="unwin-head-meta">
        <h1 class="unwin-name">{{ app.name }}</h1>
        <p v-if="lastUsed && !compactHead" class="unwin-used">{{ lastUsed }}</p>
      </div>
      <span v-if="!compactHead && related && related.total_bytes > 0" class="unwin-total">
        {{ formatTotal(related.total_bytes) }}
      </span>
    </header>

    <main class="unwin-body">
      <!-- 加载中 -->
      <div v-if="phase === 'loading'" class="unwin-center">
        <span class="unwin-spinner"></span>
        <p class="unwin-center-text">{{ progressText || '正在扫描关联文件…' }}</p>
      </div>

      <!-- 加载目标 / 清单失败 -->
      <div v-else-if="phase === 'error' && fatalError" class="unwin-center">
        <p class="unwin-center-title">无法读取卸载清单</p>
        <p class="unwin-center-error">{{ fatalError }}</p>
        <div class="unwin-center-actions">
          <button type="button" class="btn btn-ghost" @click="closeWindow">关闭</button>
          <button type="button" class="btn btn-primary" @click="load">重试</button>
        </div>
      </div>

      <!-- 完成：结果 + 失败明细 -->
      <div v-else-if="phase === 'done'" class="unwin-result">
        <span class="result-check" :class="{ 'is-warning': hasFailures }">
          <!-- 全部删干净：绿勾；有失败：琥珀色警告 -->
          <svg v-if="!hasFailures" viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M4.8 12.6 9.7 17.5 19.4 6.9"
              fill="none"
              stroke="currentColor"
              stroke-width="2.4"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
          <svg v-else viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M12 6v8"
              fill="none"
              stroke="currentColor"
              stroke-width="2.4"
              stroke-linecap="round"
            />
            <circle cx="12" cy="18" r="1.5" fill="currentColor" />
          </svg>
        </span>
        <h2 class="result-title">
          <template v-if="hasFailures">
            已卸载「{{ app?.name }}」，但有 {{ failureRows.length }} 项未删除
          </template>
          <template v-else>已卸载「{{ app?.name }}」</template>
        </h2>
        <p class="result-summary">{{ resultSummary }}</p>
        <div v-if="failureRows.length" class="result-failures">
          <p class="result-failures-title">以下项目未能删除</p>
          <ul>
            <li v-for="row in failureRows" :key="row.path">
              <span class="result-path" :title="row.path">{{ row.path }}</span>
              <span class="result-reason">{{ row.error }}</span>
            </li>
          </ul>
        </div>
      </div>

      <!-- 清单 / 空态 -->
      <template v-else>
        <div v-if="postScan" class="unwin-rescan-note">
          <strong>卸载已完成。</strong>重新扫描到 {{ allItems.length }} 项残留，确认后会一并清理。
        </div>

        <div v-if="actionError" class="unwin-banner" role="alert">
          <span class="unwin-banner-text">{{ actionError }}</span>
          <span class="unwin-banner-actions">
            <button type="button" class="banner-btn" @click="dismissError">取消</button>
            <button
              v-if="actionErrorSource"
              type="button"
              class="banner-btn banner-btn-primary"
              @click="retryAction"
            >
              重试
            </button>
          </span>
        </div>

        <div
          v-if="related && allItems.length > 0"
          class="unwin-preserve"
          :class="{ 'is-all': !preserveData }"
        >
          <span class="unwin-preserve-text">
            <template v-if="preserveData">
              默认保留设置、账号与插件配置，只清理缓存/日志等可重建文件。
            </template>
            <template v-else>
              将连同设置、账号与插件配置一起删除，删除后不可恢复。
            </template>
          </span>
          <button type="button" class="banner-btn" @click="togglePreserveData">
            {{ preserveData ? '彻底删除（含用户数据）' : '改回保留用户数据' }}
          </button>
        </div>

        <div v-if="!related || allItems.length === 0" class="unwin-empty">
          <p>没有扫描到可以清理的关联文件。</p>
          <button type="button" class="btn btn-ghost" @click="load">重新扫描</button>
        </div>
        <RelatedList
          v-else
          :groups="related.groups"
          :selected="selected"
          :app-icon="app?.icon ?? null"
          :app-name="app?.name ?? ''"
          :primary-paths="primaryPaths"
          @update:selected="setSelection"
        />
      </template>
    </main>

    <!-- 底部：全选 / 返回 / 确认删除（或结果态的关闭） -->
    <footer class="unwin-foot">
      <template v-if="phase === 'done'">
        <span class="foot-done">{{ hasFailures ? '卸载完成，部分文件未删除' : '卸载完成' }}</span>
        <div class="foot-actions">
          <button
            v-if="hasFailures"
            type="button"
            class="btn btn-ghost"
            @click="runDelete"
          >
            重试清理
          </button>
          <button type="button" class="btn btn-primary" @click="closeWindow">关闭</button>
        </div>
      </template>

      <template v-else-if="phase === 'loading' || busy">
        <span class="foot-status">
          <span v-if="busy" class="foot-dot"></span>
          {{ busy ? statusText : progressText || '正在加载…' }}
        </span>
        <div class="foot-actions">
          <button type="button" class="btn btn-ghost" disabled>返回</button>
          <button type="button" class="btn btn-primary" disabled>
            {{ busy ? busyText : '确认删除' }}
          </button>
        </div>
      </template>

      <template v-else-if="phase === 'error' && fatalError">
        <span class="foot-status">卸载窗口未就绪</span>
        <div class="foot-actions">
          <button type="button" class="btn btn-ghost" @click="closeWindow">返回</button>
        </div>
      </template>

      <template v-else>
        <button
          type="button"
          class="u-check"
          :class="{ 'is-checked': allSelected, 'is-mixed': partlySelected }"
          role="checkbox"
          :aria-checked="allSelected ? 'true' : partlySelected ? 'mixed' : 'false'"
          aria-label="全选关联文件"
          @click="toggleAll"
        >
          <svg v-if="allSelected" viewBox="0 0 14 14" aria-hidden="true">
            <path
              d="M3.2 7.5 5.9 10 10.8 4.4"
              fill="none"
              stroke="currentColor"
              stroke-width="1.9"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
          <span v-else-if="partlySelected" class="u-check-dash"></span>
        </button>
        <span class="foot-all" @click="toggleAll">全选</span>
        <div class="foot-actions">
          <button type="button" class="btn btn-ghost" @click="closeWindow">返回</button>
          <button
            type="button"
            class="btn btn-primary"
            :disabled="selectedCount === 0"
            @click="requestDelete"
          >
            {{ postScan ? '清理残留' : '确认删除' }}
          </button>
        </div>
      </template>
    </footer>

    <!-- 强制退出确认 -->
    <ForceQuitDialog
      :open="forceQuitOpen"
      :name="app?.name ?? ''"
      :icon="app?.icon ?? null"
      @cancel="forceQuitOpen = false"
      @confirm="confirmForceQuit"
    />
  </div>
</template>

<style scoped>
/* 默认保留用户数据 / 彻底删除 的切换提示 */
.unwin-preserve {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  margin: 10px 40px 0;
  padding: 8px 12px;
  border: 1px solid rgba(0, 122, 255, 0.22);
  border-radius: 8px;
  background: rgba(0, 122, 255, 0.06);
  color: var(--un-text-secondary);
  font-size: 12.5px;
}

.unwin-preserve.is-all {
  border-color: rgba(255, 149, 0, 0.38);
  background: rgba(255, 149, 0, 0.1);
}

.unwin-preserve-text {
  flex: 1;
  min-width: 0;
}

/* 卸载后二次扫描的提示条 */
.unwin-rescan-note {
  flex-shrink: 0;
  margin: 10px 40px 0;
  padding: 9px 12px;
  border: 1px solid rgba(0, 122, 255, 0.22);
  border-radius: 8px;
  background: rgba(0, 122, 255, 0.06);
  color: var(--un-text-secondary);
  font-size: 13px;
  line-height: 1.55;
}

.unwin-rescan-note strong {
  color: var(--un-text-strong);
  font-weight: 600;
}

/* Windows 自绘标题栏：与主界面 App.vue 的 .titlebar 保持同一套尺寸和交互 */
.unwin-titlebar {
  position: relative;
  flex-shrink: 0;
  height: 38px;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 0 0 0 16px;
  border-bottom: 1px solid var(--un-divider);
  background: #fff;
  user-select: none;
}

/* 透明拖动层铺满标题栏，空白处和品牌文字都能拖动窗口 */
.unwin-titlebar-drag {
  position: absolute;
  inset: 0;
  z-index: 0;
}

.unwin-titlebar > * {
  z-index: 1;
}

.unwin-titlebar > .unwin-titlebar-drag {
  z-index: 0;
}

.unwin-titlebar-brand {
  display: flex;
  align-items: center;
  min-width: 0;
  z-index: 2;
  /* 品牌区域只做展示，鼠标事件交给下面的拖动层 */
  pointer-events: none;
}

.unwin-titlebar-icon {
  width: 16px;
  height: 16px;
  margin-right: 8px;
  object-fit: contain;
  flex-shrink: 0;
}

.unwin-titlebar-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 14px;
  font-weight: 600;
  color: var(--un-text-strong);
}

.unwin-titlebar-controls {
  display: flex;
  align-self: stretch;
  flex-shrink: 0;
  margin-left: auto;
  z-index: 2;
}

.unwin-window-btn {
  width: 46px;
  height: 100%;
  display: grid;
  place-items: center;
  padding: 0;
  border: 0;
  border-radius: 0;
  background: transparent;
  color: var(--un-text-secondary);
  cursor: default;
}

.unwin-window-btn:hover {
  background: var(--un-hover);
  color: var(--un-text-strong);
}

.unwin-window-btn-close:hover {
  background: #c42b1c;
  color: #fff;
}

.unwin-window-btn:focus-visible {
  outline: 1px solid var(--un-blue);
  outline-offset: -2px;
}

.unwin-window-btn svg {
  width: 12px;
  height: 12px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1;
}
</style>
