<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Input, message } from 'ant-design-vue'
import { SearchOutlined } from '@ant-design/icons-vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { CleanerApi } from '@/composables/useCleaner'
import type { InstalledApp } from '@/types/cleaner'
import { CLEANER_APPS_CHANGED_EVENT, openUninstallWindow } from '@/api/cleaner'
import { errorText } from '@/utils/error'
import { safeGet, safeSet } from '@/composables/safeStorage'
import AppIcon from './AppIcon.vue'
import { formatBytes } from './format'

/**
 * 软件卸载页签：搜索 / 排序 / 列表。
 *
 * 卸载交互已经拆到独立窗口（`uninstall.html`）：行内「卸载」只负责调用
 * `cleaner_open_uninstall_window(appId)`，由后端创建/聚焦单例窗口并切换目标；
 * 卸载确认、运行中强制退出、关联文件勾选与结果展示都在那个窗口里完成。
 *
 * 本面板只保留列表、搜索、排序、扫描进度与错误条；监听 `cleaner-apps-changed`
 * 在卸载/删除成功后静默刷新安装列表。
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

/** 正在打开卸载窗口的应用 id（防止重复点击） */
const openingId = ref('')

const unlisteners: UnlistenFn[] = []

onMounted(async () => {
  // 页签第一次切到「软件卸载」时本组件才挂载：挂载即拉一次安装列表
  // （后端有缓存，之后来回切页签不会重扫）
  if (!cleaner.apps.length && !cleaner.appsLoading) void cleaner.loadApps(false)

  // 卸载/删除完成后后端广播，主窗口刷新列表
  try {
    unlisteners.push(
      await listen(CLEANER_APPS_CHANGED_EVENT, () => {
        void cleaner.loadApps(true)
      }),
    )
  } catch {
    // 事件订阅失败不阻断列表
  }

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

onUnmounted(() => {
  unlisteners.forEach((unlisten) => unlisten())
  unlisteners.length = 0
})

function persistSort() {
  void safeSet(SORT_STORAGE_KEY, JSON.stringify(sort.value))
}

/** 「重新扫描」：重新枚举已安装应用（列表只在首次切到本页签时拉一次） */
async function rescanApps() {
  if (cleaner.appsLoading) return
  await cleaner.loadApps(true)
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

// ============================ 打开独立卸载窗口 ============================

async function openUninstall(app: InstalledApp): Promise<void> {
  if (openingId.value) return
  openingId.value = app.id
  try {
    await openUninstallWindow(app.id)
  } catch (err) {
    message.error(errorText(err, '打开卸载窗口失败'))
  } finally {
    openingId.value = ''
  }
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
      <button
        type="button"
        class="btn btn-primary apps-rescan"
        :disabled="cleaner.appsLoading"
        title="重新枚举已安装应用"
        @click="rescanApps"
      >
        {{ cleaner.appsLoading ? '扫描中…' : '重新扫描' }}
      </button>
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
              v-else
              type="button"
              class="row-btn"
              :disabled="!!openingId"
              title="打开独立卸载窗口"
              @click="openUninstall(app)"
            >
              {{ openingId === app.id ? '打开中…' : '卸载' }}
            </button>
          </div>
        </div>
      </div>
    </div>
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
/* 「重新扫描」放在数量右侧并做高亮：主色实心 + 轻投影，扫描时禁用 */
.apps-rescan {
  flex-shrink: 0;
  height: 32px;
  padding: 0 14px;
  border-radius: 6px;
  font-size: 12px;
  transition:
    background-color 0.15s ease,
    box-shadow 0.15s ease;
  box-shadow: 0 2px 6px rgba(59, 130, 246, 0.28);
}
.apps-rescan:hover:not(:disabled) {
  box-shadow: 0 3px 10px rgba(59, 130, 246, 0.38);
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
</style>
