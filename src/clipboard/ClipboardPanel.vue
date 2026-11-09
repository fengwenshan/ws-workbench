<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  clipboardImageUrl,
  copyClipboardItem,
  getClipboardLimits,
  hideClipboardPanel,
} from '@/api/clipboard'
import { useClipboardHistory } from '@/composables/useClipboardHistory'
import type { ClipboardItemSummary, ClipboardKind, ClipboardLimits } from '@/types/clipboard'

/**
 * 剪贴板历史面板（独立窗口 /clipboard.html）。
 *
 * 采集、加密、去重都在 Rust 侧；这里只负责展示与选择。
 * 选中条目只写回系统剪贴板并收起面板，不自动模拟 ⌘V（不申请辅助功能授权）。
 */

const {
  query,
  list,
  loading,
  loadingMore,
  searching,
  searchingMode,
  hasMore,
  stats,
  loadMore,
  refresh,
} = useClipboardHistory()

const limits = ref<ClipboardLimits>({ shortcut: '⌘⇧V', max_text_chars: 200000 })
const selected = ref(0)
/** 每分钟刷新一次「刚刚 / N 分钟前」，否则时间会停在打开那一刻 */
const clock = ref(Date.now())
const searchInput = ref<HTMLInputElement | null>(null)
const listEl = ref<HTMLElement | null>(null)
const sentinel = ref<HTMLElement | null>(null)
let clockTimer: ReturnType<typeof setInterval> | null = null
let unlistenShown: UnlistenFn | null = null
let observer: IntersectionObserver | null = null

const kindLabels: Record<ClipboardKind, string> = {
  text: '文本',
  url: '链接',
  code: '代码',
  image: '图片',
  rich: '图文',
}

function kindLabel(kind: ClipboardKind): string {
  return kindLabels[kind] ?? '文本'
}

function preview(item: ClipboardItemSummary): string {
  const text = item.text_preview
  if (text) return text
  if (item.kind === 'image') return `${item.image_count} 张图片`
  return '（空内容）'
}

function thumbnails(item: ClipboardItemSummary) {
  // 纯图片用首图做封面，图文最多展示 3 张
  return item.kind === 'image' ? item.images.slice(0, 1) : item.images.slice(0, 3)
}

function relativeTime(timestamp: number): string {
  const diff = Math.max(0, clock.value - timestamp)
  if (diff < 60_000) return '刚刚'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`
  const date = new Date(timestamp)
  const sameYear = date.getFullYear() === new Date(clock.value).getFullYear()
  const pad = (value: number) => String(value).padStart(2, '0')
  const md = `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
  return sameYear ? md : `${date.getFullYear()}-${md}`
}

function clampSelection() {
  if (list.value.length === 0) {
    selected.value = 0
    return
  }
  if (selected.value >= list.value.length) {
    selected.value = list.value.length - 1
  }
}

async function focusSearch() {
  await nextTick()
  searchInput.value?.focus()
  searchInput.value?.select()
}

function scrollToSelected() {
  void nextTick(() => {
    const rows = listEl.value?.querySelectorAll<HTMLElement>('.cp-row')
    rows?.[selected.value]?.scrollIntoView({ block: 'nearest' })
  })
}

function moveSelection(delta: number) {
  const total = list.value.length
  if (total === 0) return
  selected.value = (selected.value + delta + total) % total
  // 选到底部时提前加载下一页，键盘导航不中断
  if (selected.value >= total - 2 && hasMore.value) void loadMore()
  scrollToSelected()
}

async function copyItem(item: ClipboardItemSummary) {
  try {
    await copyClipboardItem(item.id)
    await hideClipboardPanel()
  } catch {
    // 写回失败（剪贴板被占用等）时保持面板打开，让用户可以再选一次
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    if (query.value) {
      query.value = ''
      return
    }
    void hideClipboardPanel()
    return
  }
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    moveSelection(1)
    return
  }
  if (event.key === 'ArrowUp') {
    event.preventDefault()
    moveSelection(-1)
    return
  }
  if (event.key === 'Enter') {
    event.preventDefault()
    const item = list.value[selected.value]
    if (item) void copyItem(item)
  }
}

// 列表变短（搜索 / 刷新 / 删除）时把选中项夹回范围内；翻页追加不重置选择
watch(list, () => {
  if (selected.value >= list.value.length) {
    selected.value = Math.max(0, list.value.length - 1)
  }
})

// 开始 / 清空搜索时回到第一条
watch(query, () => {
  selected.value = 0
})

onMounted(async () => {
  try {
    limits.value = await getClipboardLimits()
  } catch {
    // 老后端没有这个命令时用默认值
  }
  await focusSearch()

  unlistenShown = await listen('clipboard-panel-shown', async () => {
    query.value = ''
    selected.value = 0
    await refresh()
    await focusSearch()
  })

  if (sentinel.value && listEl.value) {
    observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) void loadMore()
      },
      { root: listEl.value, rootMargin: '80px' },
    )
    observer.observe(sentinel.value)
  }

  window.addEventListener('keydown', onKeydown)
  clockTimer = setInterval(() => {
    clock.value = Date.now()
  }, 30_000)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
  unlistenShown?.()
  observer?.disconnect()
  if (clockTimer) clearInterval(clockTimer)
})
</script>

<template>
  <div class="cp">
    <header class="cp-head" data-tauri-drag-region>
      <div class="cp-title">
        剪贴板历史
        <span v-if="stats.items > 0" class="cp-count">{{ stats.items }}</span>
      </div>
      <span class="cp-kbd" :title="`全局快捷键 ${limits.shortcut}`">{{ limits.shortcut }}</span>
    </header>

    <div class="cp-search">
      <svg class="cp-search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
        <circle cx="11" cy="11" r="7" />
        <path d="M20 20l-3.5-3.5" stroke-linecap="round" />
      </svg>
      <input
        ref="searchInput"
        v-model="query"
        class="cp-search-input"
        type="text"
        placeholder="搜索剪贴板历史…"
        spellcheck="false"
        autocomplete="off"
      />
      <button
        v-if="query"
        class="cp-search-clear"
        type="button"
        title="清空搜索"
        aria-label="清空搜索"
        @click="query = ''"
      >
        ×
      </button>
    </div>

    <div v-if="searchingMode" class="cp-result-hint">
      {{ searching ? '搜索中…' : `共 ${list.length} 条结果` }}
    </div>

    <div ref="listEl" class="cp-list">
      <button
        v-for="(item, index) in list"
        :key="item.id"
        type="button"
        class="cp-row"
        :class="{ selected: index === selected }"
        :title="item.text_preview"
        @mouseenter="selected = index"
        @click="copyItem(item)"
      >
        <div class="cp-row-top">
          <span class="cp-kind" :class="'kind-' + item.kind">{{ kindLabel(item.kind) }}</span>
          <span v-if="item.kind === 'rich'" class="cp-image-count">
            {{ item.images.length }} 张图
          </span>
          <span class="cp-time">{{ relativeTime(item.created_at) }}</span>
        </div>
        <div class="cp-row-body">
          <div class="cp-text" :class="{ mono: item.kind === 'code' }">{{ preview(item) }}</div>
          <div v-if="thumbnails(item).length" class="cp-thumbs" :class="{ cover: item.kind === 'image' }">
            <img
              v-for="image in thumbnails(item)"
              :key="image.id"
              class="cp-thumb"
              :src="clipboardImageUrl(image.id)"
              :alt="`图片附件 ${image.width}×${image.height}`"
              loading="lazy"
              draggable="false"
            />
          </div>
        </div>
      </button>

      <div v-if="loading && list.length === 0" class="cp-empty">
        <p>正在读取历史…</p>
      </div>

      <div v-else-if="searchingMode && !searching && list.length === 0" class="cp-empty">
        <p>没有匹配「{{ query.trim() }}」的记录</p>
      </div>

      <div v-else-if="!searchingMode && list.length === 0" class="cp-empty">
        <p>还没有剪贴板记录</p>
        <p class="cp-empty-sub">
          复制一段文字、图片或带图文的选区，再按 <kbd>{{ limits.shortcut }}</kbd> 呼出面板。
        </p>
      </div>

      <div v-if="loadingMore" class="cp-more">加载中…</div>
      <div v-else-if="!hasMore && !searchingMode && list.length > 0" class="cp-more">没有更多了</div>
      <div ref="sentinel" class="cp-sentinel" aria-hidden="true"></div>
    </div>

    <footer class="cp-foot">
      <span class="cp-hint">↑↓ 选择 · ↵ 复制 · Esc 关闭</span>
    </footer>
  </div>
</template>

<style>
/* 窗口透明，圆角、边框与投影都画在卡片这一层（和托盘快捷面板一致） */
html,
body,
#clipboard-app {
  margin: 0;
  padding: 0;
  width: 100%;
  height: 100%;
  background: transparent;
  overflow: hidden;
}

.cp {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: 14px;
  background: var(--bg-primary);
  box-shadow:
    0 18px 42px rgba(15, 23, 42, 0.22),
    0 2px 8px rgba(15, 23, 42, 0.08);
  color: var(--text-primary);
  font-size: 13px;
}

/* ============================== 头部 ============================== */

.cp-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  height: 44px;
  padding: 0 14px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.cp-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 14px;
  font-weight: 600;
}

.cp-count {
  padding: 0 7px;
  border-radius: 999px;
  background: var(--bg-tag);
  color: var(--text-tag);
  font-size: 11px;
  font-weight: 500;
  line-height: 18px;
}

.cp-kbd {
  padding: 2px 8px;
  border-radius: 6px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-color);
  color: var(--text-muted);
  font-size: 11px;
  white-space: nowrap;
}

/* ============================== 搜索 ============================== */

.cp-search {
  position: relative;
  display: flex;
  align-items: center;
  margin: 10px 12px 4px;
  flex-shrink: 0;
}

.cp-search-icon {
  position: absolute;
  left: 9px;
  width: 15px;
  height: 15px;
  color: var(--text-muted);
  pointer-events: none;
}

.cp-search-input {
  width: 100%;
  height: 32px;
  padding: 0 28px 0 30px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: 13px;
  outline: none;
}

.cp-search-input::placeholder {
  color: var(--text-muted);
}

.cp-search-input:focus {
  border-color: var(--primary);
  background: var(--bg-input);
}

.cp-search-clear {
  position: absolute;
  right: 6px;
  width: 20px;
  height: 20px;
  border: 0;
  border-radius: 50%;
  background: transparent;
  color: var(--text-muted);
  font-size: 15px;
  line-height: 1;
  cursor: pointer;
}

.cp-search-clear:hover {
  background: var(--bg-hover);
  color: var(--text-secondary);
}

.cp-result-hint {
  padding: 0 16px 4px;
  color: var(--text-muted);
  font-size: 11px;
  flex-shrink: 0;
}

/* ============================== 列表 ============================== */

.cp-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 6px 8px 8px;
}

.cp-row {
  display: block;
  width: 100%;
  margin-bottom: 2px;
  padding: 8px 10px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  text-align: left;
  cursor: pointer;
  font: inherit;
}

.cp-row:hover {
  background: var(--bg-hover);
}

.cp-row.selected {
  background: var(--bg-active);
  border-color: rgba(59, 130, 246, 0.35);
}

.cp-row-top {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 3px;
}

.cp-kind {
  padding: 0 6px;
  border-radius: 999px;
  font-size: 11px;
  line-height: 17px;
  background: var(--bg-tag);
  color: var(--text-tag);
  flex-shrink: 0;
}

.cp-kind.kind-url {
  background: #eff6ff;
  color: #1d4ed8;
}

.cp-kind.kind-code {
  background: #f5f3ff;
  color: #6d28d9;
}

.cp-kind.kind-image {
  background: #ecfdf5;
  color: #047857;
}

.cp-kind.kind-rich {
  background: #fef3c7;
  color: #b45309;
}

.cp-image-count {
  color: var(--text-muted);
  font-size: 11px;
}

.cp-time {
  margin-left: auto;
  color: var(--text-muted);
  font-size: 11px;
  white-space: nowrap;
}

.cp-row-body {
  display: flex;
  align-items: flex-start;
  gap: 8px;
}

.cp-text {
  flex: 1;
  min-width: 0;
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 12.5px;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-all;
}

.cp-text.mono {
  font-family: 'SF Mono', Menlo, Consolas, monospace;
  font-size: 12px;
}

.cp-thumbs {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}

.cp-thumb {
  width: 44px;
  height: 44px;
  border-radius: 6px;
  border: 1px solid var(--border-color);
  object-fit: cover;
  background: var(--bg-secondary);
}

.cp-thumbs.cover .cp-thumb {
  width: 56px;
  height: 56px;
}

.cp-thumbs.cover + .cp-text,
.cp-row-body:has(.cp-thumbs.cover) .cp-text {
  -webkit-line-clamp: 2;
}

.cp-empty {
  padding: 48px 24px;
  text-align: center;
  color: var(--text-secondary);
  font-size: 13px;
}

.cp-empty p {
  margin: 0 0 6px;
}

.cp-empty-sub {
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.7;
}

.cp-empty kbd {
  padding: 1px 5px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--bg-secondary);
  font-size: 11px;
  font-family: inherit;
}

.cp-more {
  padding: 6px 0 2px;
  text-align: center;
  color: var(--text-muted);
  font-size: 11px;
}

.cp-sentinel {
  height: 1px;
}

/* ============================== 底部 ============================== */

.cp-foot {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 38px;
  padding: 0 14px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-secondary);
  flex-shrink: 0;
}

.cp-hint {
  color: var(--text-muted);
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
