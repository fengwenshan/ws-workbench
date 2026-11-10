<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import AppIcon from '@/components/cleaner/AppIcon.vue'
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
  RelatedFiles,
} from '@/types/cleaner'
import ForceQuitDialog from './ForceQuitDialog.vue'
import RelatedList from './RelatedList.vue'

/**
 * 软件卸载独立窗口（820×560，由后端 `cleaner_open_uninstall_window` 创建）。
 *
 * 状态机：loading → ready →（运行中则弹强制退出确认）→ quitting → uninstalling
 *        → deleting → done | error
 *
 * 请求编排顺序与设计 3.1 一致：
 * 1. `cleaner_uninstall_target` 取目标应用；
 * 2. `cleaner_related_files` 扫三组关联文件（同时刷新后端删除白名单）；
 * 3. 点「确认删除」先 `cleaner_app_running`：运行中弹强制退出确认；
 * 4. 「强制退出」→ `cleaner_force_quit`，进程归零后继续；
 * 5. `cleaner_uninstall_app(quitRunning=false)` 执行系统卸载；
 * 6. `cleaner_delete_related` 删除勾选的关联文件，结果留在窗口内展示。
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

const unlisteners: UnlistenFn[] = []

const busy = computed(
  () =>
    phase.value === 'quitting' ||
    phase.value === 'uninstalling' ||
    phase.value === 'deleting',
)

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
    await runDelete()
  } catch (err) {
    phase.value = 'ready'
    actionErrorSource.value = 'uninstall'
    actionError.value = errorText(err, '卸载失败')
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
          <button type="button" class="btn btn-primary" @click="closeWindow">关闭</button>
        </div>
      </template>

      <template v-else-if="phase === 'loading' || busy">
        <span class="foot-status">
          <span v-if="busy" class="foot-dot"></span>
          {{ busy ? statusText : '正在加载…' }}
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
            确认删除
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
