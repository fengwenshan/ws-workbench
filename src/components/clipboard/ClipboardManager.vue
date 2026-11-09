<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Modal, message } from 'ant-design-vue'
import { save } from '@tauri-apps/plugin-dialog'
import {
  clipboardImageUrl,
  clearClipboardItems,
  copyClipboardImage,
  copyClipboardItem,
  exportClipboardImage,
} from '@/api/clipboard'
import { useClipboardHistory } from '@/composables/useClipboardHistory'
import { platform } from '@/platform'
import type {
  ClipboardImageMeta,
  ClipboardItemSummary,
} from '@/types/clipboard'

/**
 * 「系统 → 剪贴板」管理页：历史列表 + 搜索 + 复制 / 删除 / 清空 + 图片预览导出。
 *
 * 数据与面板共用 `useClipboardHistory`；删除 / 清空由 Rust 侧级联删掉本地图片附件。
 * Windows 不采集剪贴板，这里只显示说明。
 */

const props = defineProps<{
  /** 当前页签是否激活（CleanerPanel 传入） */
  active: boolean
}>()

const isMacos = computed(() => platform.is_macos)

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
  removeItem,
} = useClipboardHistory({ autoStart: false })

const started = ref(false)
const listEl = ref<HTMLElement | null>(null)
const sentinel = ref<HTMLElement | null>(null)
let parentObserver: IntersectionObserver | null = null

/** 当前查看的图片弹层 */
const viewer = ref<{
  item: ClipboardItemSummary
  index: number
} | null>(null)
const viewerOpen = computed({
  get: () => viewer.value !== null,
  set: (open: boolean) => {
    if (!open) viewer.value = null
  },
})
const viewerImages = computed<ClipboardImageMeta[]>(() => viewer.value?.item.images ?? [])
const viewerImage = computed(() =>
  viewer.value ? viewerImages.value[viewer.value.index] ?? null : null,
)

function preview(item: ClipboardItemSummary): string {
  // 纯图片条目只留缩略图，不写「N 张图片」这种类型说明
  return item.text_preview
}

function absoluteTime(timestamp: number): string {
  const date = new Date(timestamp)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`
}

const summaryText = computed(
  () => `共 ${stats.value.items} 条 · ${stats.value.images} 张图片`,
)

async function ensureStarted() {
  if (started.value) return
  started.value = true
  await refresh()
}

watch(
  () => props.active,
  (active) => {
    if (active) void ensureStarted()
  },
  { immediate: true },
)

/** 列表滚动懒加载：哨兵进入可视区就拉下一页 */
function setupLazyLoad() {
  if (!sentinel.value || !listEl.value) return
  parentObserver = new IntersectionObserver(
    (entries) => {
      if (entries.some((entry) => entry.isIntersecting)) void loadMore()
    },
    { root: listEl.value, rootMargin: '120px' },
  )
  parentObserver.observe(sentinel.value)
}

async function copyOne(item: ClipboardItemSummary) {
  try {
    await copyClipboardItem(item.id)
    message.success('已复制到剪贴板')
  } catch (err) {
    message.error(String(err))
  }
}

async function deleteOne(item: ClipboardItemSummary) {
  try {
    const removed = await removeItem(item.id)
    message.success(removed > 0 ? `已删除（含 ${removed} 张图片附件）` : '已删除')
  } catch (err) {
    message.error(String(err))
  }
}

function confirmClear() {
  if (stats.value.items === 0) return
  Modal.confirm({
    title: '清空剪贴板历史',
    content: `将删除全部 ${stats.value.items} 条历史及 ${stats.value.images} 张图片附件，不可恢复。`,
    okText: '清空',
    okType: 'danger',
    cancelText: '取消',
    async onOk() {
      try {
        const result = await clearClipboardItems()
        await refresh()
        message.success(
          `已清空 ${result.removed_items} 条历史（含 ${result.removed_images} 张图片附件）`,
        )
      } catch (err) {
        message.error(String(err))
        throw err
      }
    },
  })
}

function openViewer(item: ClipboardItemSummary, index = 0) {
  if (item.images.length === 0) return
  viewer.value = { item, index }
}

function stepViewer(delta: number) {
  if (!viewer.value) return
  const total = viewerImages.value.length
  if (total === 0) return
  viewer.value = {
    ...viewer.value,
    index: (viewer.value.index + delta + total) % total,
  }
}

async function copyViewerImage() {
  if (!viewerImage.value) return
  try {
    await copyClipboardImage(viewerImage.value.id)
    message.success('图片已复制到剪贴板')
  } catch (err) {
    message.error(String(err))
  }
}

function extensionFor(mime: string): string {
  if (mime.includes('jpeg') || mime.includes('jpg')) return 'jpg'
  if (mime.includes('gif')) return 'gif'
  if (mime.includes('webp')) return 'webp'
  return 'png'
}

function stamp(): string {
  const date = new Date()
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(
    date.getHours(),
  )}${pad(date.getMinutes())}${pad(date.getSeconds())}`
}

async function exportViewerImage() {
  if (!viewer.value || !viewerImage.value) return
  const image = viewerImage.value
  const seq = viewer.value.index + 1
  const ext = extensionFor(image.mime)
  try {
    const target = await save({
      defaultPath: `clipboard-${stamp()}-${seq}.${ext}`,
      filters: [{ name: '图片', extensions: [ext] }],
    })
    if (!target) return
    await exportClipboardImage(image.id, target)
    message.success('图片已导出')
  } catch (err) {
    message.error(String(err))
  }
}

/** 图片弹层键盘：← → 切图，Esc 关闭（交给 Modal 自己处理） */
function onViewerKeydown(event: KeyboardEvent) {
  if (!viewer.value) return
  if (event.key === 'ArrowLeft') {
    event.preventDefault()
    stepViewer(-1)
  } else if (event.key === 'ArrowRight') {
    event.preventDefault()
    stepViewer(1)
  }
}

onMounted(() => {
  void nextTick(() => setupLazyLoad())
  window.addEventListener('keydown', onViewerKeydown)
})

onUnmounted(() => {
  parentObserver?.disconnect()
  window.removeEventListener('keydown', onViewerKeydown)
})
</script>

<template>
  <div class="cm">
    <div v-if="!isMacos" class="cm-blocked">
      <p class="cm-blocked-title">剪贴板历史仅在 macOS 启用</p>
      <p class="cm-blocked-sub">Windows 请使用系统自带的 <kbd>Win</kbd> + <kbd>V</kbd>。</p>
    </div>

    <template v-else>
      <div class="cm-toolbar">
        <div class="cm-search">
          <svg class="cm-search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <circle cx="11" cy="11" r="7" />
            <path d="M20 20l-3.5-3.5" stroke-linecap="round" />
          </svg>
          <input
            v-model="query"
            class="cm-search-input"
            type="text"
            placeholder="搜索历史内容…"
            spellcheck="false"
            autocomplete="off"
          />
        </div>
        <span class="cm-summary">{{ summaryText }}</span>
        <button
          class="cm-clear"
          type="button"
          :disabled="stats.items === 0"
          @click="confirmClear"
        >
          清空历史
        </button>
      </div>

      <div v-if="searchingMode" class="cm-result-hint">
        {{ searching ? '搜索中…' : `共 ${list.length} 条结果` }}
      </div>

      <div ref="listEl" class="cm-list">
        <div
          v-for="item in list"
          :key="item.id"
          class="cm-row"
          @click="copyOne(item)"
        >
          <!-- 上半部分：内容 -->
          <div class="cm-row-main">
            <button
              v-if="item.images.length"
              type="button"
              class="cm-thumb-btn"
              :title="`预览图片 ${item.images[0].width}×${item.images[0].height}`"
              @click.stop="openViewer(item, 0)"
            >
              <img
                class="cm-thumb"
                :src="clipboardImageUrl(item.images[0].id)"
                alt="图片缩略图"
                loading="lazy"
                draggable="false"
              />
              <span v-if="item.images.length > 1" class="cm-thumb-count">
                {{ item.images.length }}
              </span>
            </button>
            <div class="cm-text" :class="{ mono: item.kind === 'code' }">{{ preview(item) }}</div>
          </div>
          <!-- 下半部分：右下角的时间与操作 -->
          <div class="cm-row-foot">
            <span class="cm-time">{{ absoluteTime(item.created_at) }}</span>
            <button type="button" class="cm-action copy" @click.stop="copyOne(item)">复制</button>
            <button type="button" class="cm-action delete" @click.stop="deleteOne(item)">删除</button>
          </div>
        </div>

        <div v-if="loading && list.length === 0" class="cm-empty">正在读取历史…</div>
        <div v-else-if="searchingMode && !searching && list.length === 0" class="cm-empty">
          没有匹配「{{ query.trim() }}」的记录
        </div>
        <div v-else-if="!searchingMode && list.length === 0" class="cm-empty">
          还没有剪贴板记录；在 macOS 上复制内容后再按 ⌘⇧V 呼出面板看看。
        </div>

        <div v-if="loadingMore" class="cm-more">加载中…</div>
        <div v-else-if="!hasMore && !searchingMode && list.length > 0" class="cm-more">
          没有更多了
        </div>
        <div ref="sentinel" class="cm-sentinel" aria-hidden="true"></div>
      </div>

      <Modal
        v-model:open="viewerOpen"
        :title="viewer ? `图片 ${(viewer.index ?? 0) + 1} / ${viewerImages.length}` : '图片'"
        :footer="null"
        :width="720"
        centered
      >
        <div v-if="viewerImage" class="cm-viewer">
          <div class="cm-viewer-stage">
            <button
              v-if="viewerImages.length > 1"
              type="button"
              class="cm-viewer-nav prev"
              aria-label="上一张"
              @click="stepViewer(-1)"
            >
              ‹
            </button>
            <img
              class="cm-viewer-image"
              :src="clipboardImageUrl(viewerImage.id, 'full')"
              :alt="`图片 ${viewerImage.width}×${viewerImage.height}`"
            />
            <button
              v-if="viewerImages.length > 1"
              type="button"
              class="cm-viewer-nav next"
              aria-label="下一张"
              @click="stepViewer(1)"
            >
              ›
            </button>
          </div>
          <div class="cm-viewer-meta">
            {{ viewerImage.mime }} · {{ viewerImage.width }}×{{ viewerImage.height }} ·
            {{ formatBytes(viewerImage.orig_size) }}
          </div>
          <div v-if="viewerImages.length > 1" class="cm-viewer-strip">
            <button
              v-for="(image, imageIndex) in viewerImages"
              :key="image.id"
              type="button"
              class="cm-viewer-strip-item"
              :class="{ active: imageIndex === viewer?.index }"
              @click="viewer && (viewer = { ...viewer, index: imageIndex })"
            >
              <img :src="clipboardImageUrl(image.id)" alt="缩略图" draggable="false" />
            </button>
          </div>
          <div class="cm-viewer-actions">
            <button type="button" class="cm-btn" @click="copyViewerImage">复制图片</button>
            <button type="button" class="cm-btn primary" @click="exportViewerImage">导出</button>
          </div>
        </div>
      </Modal>
    </template>
  </div>
</template>

<style scoped>
.cm {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
  /* 与 Hosts / 屏幕休眠等页签一致的内容区边距 */
  padding: 10px 20px 16px;
  overflow: hidden;
}

/* ============================== Windows 说明 ============================== */

.cm-blocked {
  margin: 24px 0;
  padding: 32px 24px;
  border: 1px dashed var(--border-color);
  border-radius: 10px;
  background: var(--bg-secondary);
  text-align: center;
}

.cm-blocked-title {
  margin: 0 0 6px;
  color: var(--text-primary);
  font-size: 14px;
  font-weight: 600;
}

.cm-blocked-sub {
  margin: 0;
  color: var(--text-muted);
  font-size: 12px;
}

.cm-blocked kbd {
  padding: 1px 5px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--bg-primary);
  font-size: 11px;
  font-family: inherit;
}

/* ============================== 工具栏 ============================== */

.cm-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-shrink: 0;
}

.cm-search {
  position: relative;
  flex: 0 0 240px;
  display: flex;
  align-items: center;
}

.cm-search-icon {
  position: absolute;
  left: 9px;
  width: 14px;
  height: 14px;
  color: var(--text-muted);
  pointer-events: none;
}

.cm-search-input {
  width: 100%;
  height: 32px;
  padding: 0 12px 0 30px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: 13px;
  outline: none;
}

.cm-search-input:focus {
  border-color: var(--primary);
  background: var(--bg-input);
}

.cm-summary {
  color: var(--text-muted);
  font-size: 12px;
  white-space: nowrap;
}

.cm-clear {
  margin-left: auto;
  height: 32px;
  padding: 0 14px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: var(--danger);
  font-size: 12px;
  cursor: pointer;
}

.cm-clear:hover:not(:disabled) {
  background: var(--danger-soft, rgba(239, 68, 68, 0.08));
}

.cm-clear:disabled {
  color: var(--text-muted);
  cursor: default;
  opacity: 0.6;
}

.cm-result-hint {
  color: var(--text-muted);
  font-size: 11px;
  flex-shrink: 0;
}

/* ============================== 列表 ============================== */

/* 列表：白底 + 行分隔线，整行可点（点空白处复制） */
.cm-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-primary);
}

/* 单条：上下结构（内容在上、操作在右下角） */
.cm-row {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-primary);
  cursor: pointer;
}

.cm-row:last-child {
  border-bottom: 0;
}

.cm-row:hover {
  background: var(--bg-hover);
}

.cm-row-main {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  min-width: 0;
}

.cm-row-foot {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
}

.cm-thumb-btn {
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

.cm-thumb-btn:hover {
  border-color: var(--primary);
}

.cm-thumb {
  width: 44px;
  height: 44px;
  object-fit: cover;
  display: block;
}

.cm-thumb-count {
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

.cm-text {
  flex: 1;
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

.cm-text.mono {
  font-family: 'SF Mono', Menlo, Consolas, monospace;
  font-size: 12px;
}

.cm-time {
  margin-right: 2px;
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 11px;
  white-space: nowrap;
}

.cm-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.cm-action {
  height: 24px;
  padding: 0 10px;
  border: 0;
  border-radius: 6px;
  color: #fff;
  font-size: 12px;
  cursor: pointer;
}

.cm-action.copy {
  background: #16a34a;
}

.cm-action.copy:hover {
  background: #15803d;
}

.cm-action.delete {
  background: #dc2626;
}

.cm-action.delete:hover {
  background: #b91c1c;
}

.cm-empty {
  padding: 40px 16px;
  text-align: center;
  color: var(--text-secondary);
  font-size: 13px;
}

.cm-more {
  padding: 6px 0;
  text-align: center;
  color: var(--text-muted);
  font-size: 11px;
}

.cm-sentinel {
  height: 1px;
}

/* ============================== 图片弹层 ============================== */

.cm-viewer {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.cm-viewer-stage {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 280px;
  max-height: 52vh;
  border-radius: 8px;
  background: var(--bg-secondary);
  overflow: hidden;
}

.cm-viewer-image {
  max-width: 100%;
  max-height: 52vh;
  object-fit: contain;
}

.cm-viewer-nav {
  position: absolute;
  top: 50%;
  transform: translateY(-50%);
  width: 32px;
  height: 48px;
  border: 0;
  border-radius: 8px;
  background: rgba(15, 23, 42, 0.45);
  color: #fff;
  font-size: 24px;
  line-height: 1;
  cursor: pointer;
}

.cm-viewer-nav.prev {
  left: 8px;
}

.cm-viewer-nav.next {
  right: 8px;
}

.cm-viewer-meta {
  color: var(--text-muted);
  font-size: 12px;
  text-align: center;
}

.cm-viewer-strip {
  display: flex;
  gap: 6px;
  overflow-x: auto;
  padding-bottom: 2px;
}

.cm-viewer-strip-item {
  padding: 0;
  border: 2px solid transparent;
  border-radius: 6px;
  background: transparent;
  line-height: 0;
  cursor: pointer;
  overflow: hidden;
}

.cm-viewer-strip-item.active {
  border-color: var(--primary);
}

.cm-viewer-strip-item img {
  width: 48px;
  height: 48px;
  object-fit: cover;
  display: block;
}

.cm-viewer-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.cm-btn {
  height: 32px;
  padding: 0 14px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  color: var(--text-primary);
  font-size: 12px;
  cursor: pointer;
}

.cm-btn:hover {
  background: var(--bg-hover);
}

.cm-btn.primary {
  border-color: var(--primary);
  background: var(--primary);
  color: #fff;
}

.cm-btn.primary:hover {
  filter: brightness(1.05);
}
</style>
