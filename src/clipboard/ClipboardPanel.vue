<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  clipboardImageUrl,
  copyClipboardImage,
  copyClipboardItem,
  getClipboardLimits,
  hideClipboardPanel,
  setClipboardPinned,
} from '@/api/clipboard'
import { useClipboardHistory } from '@/composables/useClipboardHistory'
import { formatShortcut } from '@/clipboard/shortcut'
import type { ClipboardImageMeta, ClipboardItemSummary, ClipboardLimits } from '@/types/clipboard'

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
  loadMore,
  refresh,
} = useClipboardHistory()

const limits = ref<ClipboardLimits>({
  shortcut: 'CommandOrControl+Shift+V',
  max_text_chars: 200000,
})
/** 快捷键显示成平台习惯的写法：mac ⌘⇧V / Windows Ctrl+Shift+V */
const shortcutLabel = computed(() => formatShortcut(limits.value.shortcut))
/** 置顶（贴屏幕）：置顶后点外面、复制都不收起，面板一关就失效 */
const pinned = ref(false)
const selected = ref(0)
const searchInput = ref<HTMLInputElement | null>(null)
const listEl = ref<HTMLElement | null>(null)
const sentinel = ref<HTMLElement | null>(null)
/** 点缩略图查看原图，不触发复制 */
const viewer = ref<{ item: ClipboardItemSummary; index: number } | null>(null)
const viewerImages = computed<ClipboardImageMeta[]>(() => viewer.value?.item.images ?? [])
const viewerImage = computed(() =>
  viewer.value ? viewerImages.value[viewer.value.index] ?? null : null,
)
let unlistenShown: UnlistenFn | null = null
let observer: IntersectionObserver | null = null

function preview(item: ClipboardItemSummary): string {
  // 纯图片条目只留缩略图，不再用文字说明
  return item.text_preview
}

function openViewer(item: ClipboardItemSummary, index: number) {
  if (item.images.length === 0) return
  viewer.value = { item, index: Math.min(Math.max(0, index), item.images.length - 1) }
}

function closeViewer() {
  viewer.value = null
  // 关掉大图后焦点回到搜索框，键盘操作不中断
  searchInput.value?.focus()
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
    // 置顶时不收起，方便连续复制
    if (!pinned.value) await hideClipboardPanel()
  } catch {
    // 写回失败（剪贴板被占用等）时保持面板打开，让用户可以再选一次
  }
}

async function togglePin() {
  try {
    pinned.value = await setClipboardPinned(!pinned.value)
  } catch {
    // 设置失败时保持原状态
  }
}

function refreshPanel() {
  void refresh()
}

async function closePanel() {
  pinned.value = false
  await hideClipboardPanel()
}

/** 图文条目按「文字优先」贴，需要单独发图时在大图里复制这一张 */
async function copyViewerImage() {
  const image = viewerImage.value
  if (!image) return
  try {
    await copyClipboardImage(image.id)
    viewer.value = null
    if (!pinned.value) await hideClipboardPanel()
  } catch {
    // 图片读不出来时保持大图，让用户可以再试
  }
}

function stepViewer(delta: number) {
  if (!viewer.value) return
  const total = viewerImages.value.length
  if (total <= 1) return
  viewer.value = {
    ...viewer.value,
    index: (viewer.value.index + delta + total) % total,
  }
}

/**
 * 整行点击：只有点在空白处才复制。
 * 文字要能选中、缩略图点开大图，都不能误触发复制（和主窗口列表一致）。
 */
function onRowClick(item: ClipboardItemSummary, event: MouseEvent) {
  const target = event.target as Element | null
  if (target?.closest('.cp-text, .cp-thumb-btn')) return
  // 拖选文字后松手也会冒泡一次 click，选中了就不复制
  if ((window.getSelection()?.toString() ?? '').length > 0) return
  void copyItem(item)
}

function onKeydown(event: KeyboardEvent) {
  // 大图打开时只响应 Esc 与左右切换，避免方向键/回车误操作后面的列表
  if (viewer.value && !['Escape', 'ArrowLeft', 'ArrowRight'].includes(event.key)) return
  if (viewer.value && event.key === 'ArrowLeft') {
    event.preventDefault()
    stepViewer(-1)
    return
  }
  if (viewer.value && event.key === 'ArrowRight') {
    event.preventDefault()
    stepViewer(1)
    return
  }
  if (event.key === 'Escape') {
    event.preventDefault()
    if (viewer.value) {
      closeViewer()
      return
    }
    if (query.value) {
      query.value = ''
      return
    }
    void closePanel()
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

  unlistenShown = await listen<boolean>('clipboard-panel-shown', async (event) => {
    query.value = ''
    selected.value = 0
    viewer.value = null
    pinned.value = event.payload === true
    // 管理页可能刚改过快捷键，每次呼出都重新读一次
    try {
      limits.value = await getClipboardLimits()
    } catch {
      // 读不到就继续用上一次的值
    }
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
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
  unlistenShown?.()
  observer?.disconnect()
})
</script>

<template>
  <div class="cp">
    <header class="cp-head" data-tauri-drag-region>
      <div class="cp-title">剪贴板历史</div>
      <div class="cp-head-actions">
        <button
          class="cp-icon-btn"
          type="button"
          title="刷新"
          aria-label="刷新"
          :disabled="loading"
          @click="refreshPanel"
        >
          <svg
            :class="{ spinning: loading }"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            aria-hidden="true"
          >
            <path d="M21 12a9 9 0 1 1-2.64-6.36" stroke-linecap="round" />
            <path d="M21 3v6h-6" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </button>
        <button
          class="cp-icon-btn"
          :class="{ active: pinned }"
          type="button"
          :title="pinned ? '取消置顶' : '置顶'"
          :aria-pressed="pinned"
          aria-label="置顶"
          @click="togglePin"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
            <path d="M9 3h6l-1 7 3 3v2H7v-2l3-3-1-7z" stroke-linejoin="round" />
            <path d="M12 15v6" stroke-linecap="round" />
          </svg>
        </button>
        <button class="cp-icon-btn" type="button" title="关闭" aria-label="关闭" @click="closePanel">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" stroke-linecap="round" />
          </svg>
        </button>
      </div>
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
      <div
        v-for="(item, index) in list"
        :key="item.id"
        class="cp-row"
        :class="{ selected: index === selected }"
        @mouseenter="selected = index"
        @click="onRowClick(item, $event)"
      >
        <div class="cp-row-main">
          <button
            v-if="item.images.length"
            type="button"
            class="cp-thumb-btn"
            :title="`预览图片 ${item.images[0].width}×${item.images[0].height}`"
            @click.stop="openViewer(item, 0)"
          >
            <img
              class="cp-thumb"
              :src="clipboardImageUrl(item.images[0].id)"
              :alt="`图片附件 ${item.images[0].width}×${item.images[0].height}`"
              loading="lazy"
              draggable="false"
            />
            <span v-if="item.images.length > 1" class="cp-thumb-count">
              {{ item.images.length }}
            </span>
          </button>
          <div v-if="preview(item)" class="cp-text" :class="{ mono: item.kind === 'code' }">
            {{ preview(item) }}
          </div>
        </div>
      </div>

      <div v-if="loading && list.length === 0" class="cp-empty">
        <p>正在读取历史…</p>
      </div>

      <div v-else-if="searchingMode && !searching && list.length === 0" class="cp-empty">
        <p>没有匹配「{{ query.trim() }}」的记录</p>
      </div>

      <div v-else-if="!searchingMode && list.length === 0" class="cp-empty">
        <p>还没有剪贴板记录</p>
        <p v-if="shortcutLabel" class="cp-empty-sub">
          复制一段文字、图片或带图文的选区，再按 <kbd>{{ shortcutLabel }}</kbd> 呼出面板。
        </p>
      </div>

      <div v-if="loadingMore" class="cp-more">加载中…</div>
      <div ref="sentinel" class="cp-sentinel" aria-hidden="true"></div>
    </div>

    <!-- 图片预览：点缩略图打开，点空白处关闭；这里可以单独复制这张图 -->
    <div v-if="viewerImage" class="cp-viewer" @click="closeViewer">
      <img
        class="cp-viewer-image"
        :src="clipboardImageUrl(viewerImage.id, 'full')"
        :alt="`图片 ${viewerImage.width}×${viewerImage.height}`"
        draggable="false"
      />
      <template v-if="viewerImages.length > 1">
        <button
          class="cp-viewer-nav prev"
          type="button"
          aria-label="上一张"
          @click.stop="stepViewer(-1)"
        >
          ‹
        </button>
        <button
          class="cp-viewer-nav next"
          type="button"
          aria-label="下一张"
          @click.stop="stepViewer(1)"
        >
          ›
        </button>
      </template>
      <button class="cp-viewer-copy" type="button" @click.stop="copyViewerImage">
        复制图片
      </button>
    </div>
  </div>
</template>

<style>
/* 窗口透明，圆角与描边都画在卡片这一层（和托盘快捷面板一致）。
   这里不能加外投影：窗口正好和卡片一样大，投影会被窗口矩形裁掉，只在
   四个圆角外侧留下一圈灰色直角。

   选择器必须比 `main.css` 里的 `body { background: ... }` 更具体：
   全局样式是后加载的，权重相同就会盖掉这里的 transparent，四个圆角外侧
   于是露出一整块白底（Windows 上就是这个现象）。 */
html,
html body,
#clipboard-app {
  margin: 0;
  padding: 0;
  width: 100%;
  height: 100%;
  background: transparent;
  overflow: hidden;
}

.cp {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: 14px;
  background: var(--bg-primary);
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

.cp-head-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}

.cp-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
}

.cp-icon-btn:hover {
  background: var(--bg-hover);
  color: var(--text-secondary);
}

.cp-icon-btn:disabled {
  cursor: default;
}

.cp-icon-btn.active {
  background: var(--bg-secondary);
  color: var(--primary);
}

.cp-icon-btn svg {
  width: 15px;
  height: 15px;
}

.cp-icon-btn svg.spinning {
  animation: cp-spin 0.8s linear infinite;
}

@keyframes cp-spin {
  to {
    transform: rotate(360deg);
  }
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
  margin-bottom: 2px;
  padding: 8px 12px;
  border: 1px solid transparent;
  border-radius: 8px;
  cursor: pointer;
}

.cp-row:hover {
  background: var(--bg-hover);
}

.cp-row.selected {
  background: var(--bg-active);
  border-color: rgba(59, 130, 246, 0.35);
}

/* 和主窗口一致：首图在左、文字在右 */
.cp-row-main {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  min-width: 0;
}

.cp-thumb-btn {
  position: relative;
  flex-shrink: 0;
  padding: 0;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-primary);
  line-height: 0;
  cursor: zoom-in;
  overflow: hidden;
}

.cp-thumb-btn:hover {
  border-color: var(--primary);
}

.cp-thumb {
  width: 44px;
  height: 44px;
  object-fit: cover;
  display: block;
}

.cp-thumb-count {
  position: absolute;
  right: 2px;
  bottom: 2px;
  padding: 0 4px;
  border-radius: 999px;
  background: rgba(15, 23, 42, 0.7);
  color: #fff;
  font-size: 10px;
  line-height: 14px;
  pointer-events: none;
}

.cp-text {
  /* 宽度跟着文字走：行尾空白留给整行点击复制，不要整行都是文本光标 */
  flex: 0 1 auto;
  max-width: 100%;
  cursor: text;
  user-select: text;
  min-width: 0;
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  color: var(--text-primary);
  font-size: 13px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
}

.cp-text.mono {
  font-family: 'SF Mono', Menlo, Consolas, monospace;
  font-size: 12px;
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

/* ============================== 图片预览 ============================== */

.cp-viewer {
  position: absolute;
  inset: 0;
  z-index: 10;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  background: rgba(15, 23, 42, 0.55);
  cursor: zoom-out;
}

.cp-viewer-image {
  max-width: 100%;
  max-height: 100%;
  border-radius: 8px;
  background: var(--bg-primary);
  object-fit: contain;
}

.cp-viewer-nav {
  position: absolute;
  top: 50%;
  width: 30px;
  height: 30px;
  margin-top: -15px;
  border: 0;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.9);
  color: var(--text-secondary);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
}

.cp-viewer-nav.prev {
  left: 10px;
}

.cp-viewer-nav.next {
  right: 10px;
}

.cp-viewer-copy {
  position: absolute;
  bottom: 14px;
  left: 50%;
  transform: translateX(-50%);
  height: 30px;
  padding: 0 14px;
  border: 0;
  border-radius: 8px;
  background: var(--primary);
  color: #fff;
  font-size: 12px;
  cursor: pointer;
}
</style>
