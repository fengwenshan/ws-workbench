import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  clipboardPage,
  clipboardSearch,
  clipboardStats,
  deleteClipboardItem,
} from '@/api/clipboard'
import type { ClipboardItemSummary, ClipboardStats } from '@/types/clipboard'

/** 首屏 / 每次翻页条数（后端 clamp 1–50） */
const PAGE_SIZE = 20
/** 搜索防抖，避免每敲一个字都扫一遍全库 */
const SEARCH_DEBOUNCE_MS = 250

export interface UseClipboardHistoryOptions {
  /** 是否监听 Rust 侧事件自动刷新（面板与管理页都需要） */
  listenEvents?: boolean
  /** 是否在 onMounted 时自动拉首屏；管理页未激活时传 false，由调用方触发 refresh */
  autoStart?: boolean
}

/**
 * 剪贴板历史共享状态：分页懒加载、后端搜索、统计、删除。
 *
 * 面板与管理页共用；两边各自实例化，但数据源都是同一张 SQLite 表。
 */
export function useClipboardHistory(options: UseClipboardHistoryOptions = {}) {
  const listenEvents = options.listenEvents ?? true
  const autoStart = options.autoStart ?? true

  const items = ref<ClipboardItemSummary[]>([])
  const results = ref<ClipboardItemSummary[]>([])
  const query = ref('')
  const loading = ref(false)
  const loadingMore = ref(false)
  const searching = ref(false)
  const hasMore = ref(true)
  const error = ref('')
  const stats = ref<ClipboardStats>({ items: 0, images: 0, bytes: 0 })

  const searchingMode = computed(() => query.value.trim().length > 0)
  /** 当前展示的列表：搜索时用搜索结果，否则用分页列表 */
  const list = computed(() => (searchingMode.value ? results.value : items.value))

  let searchTimer: ReturnType<typeof setTimeout> | null = null
  /** 搜索请求序号：只认最后一次，避免慢请求覆盖新结果 */
  let searchSeq = 0
  let unlistenChanged: UnlistenFn | null = null
  let unlistenShown: UnlistenFn | null = null

  async function loadStats() {
    try {
      stats.value = await clipboardStats()
    } catch {
      // 统计失败不影响列表，保留上一次的值
    }
  }

  async function refresh() {
    if (loading.value) return
    loading.value = true
    error.value = ''
    try {
      const page = await clipboardPage(undefined, undefined, PAGE_SIZE)
      items.value = page.items
      hasMore.value = page.has_more
      void loadStats()
      // 搜索态下刷新，搜索结果也要跟着更新
      if (searchingMode.value) {
        void runSearch(query.value)
      }
    } catch (err) {
      error.value = String(err)
    } finally {
      loading.value = false
    }
  }

  async function loadMore() {
    if (searchingMode.value || !hasMore.value || loading.value || loadingMore.value) return
    const last = items.value[items.value.length - 1]
    if (!last) return
    loadingMore.value = true
    try {
      const page = await clipboardPage(last.created_at, last.id, PAGE_SIZE)
      items.value.push(...page.items)
      hasMore.value = page.has_more
    } catch (err) {
      error.value = String(err)
    } finally {
      loadingMore.value = false
    }
  }

  async function runSearch(raw: string) {
    const keyword = raw.trim()
    if (!keyword) {
      results.value = []
      searching.value = false
      return
    }
    const seq = ++searchSeq
    searching.value = true
    try {
      const found = await clipboardSearch(keyword)
      if (seq === searchSeq) results.value = found
    } catch {
      if (seq === searchSeq) results.value = []
    } finally {
      if (seq === searchSeq) searching.value = false
    }
  }

  watch(query, (raw) => {
    if (searchTimer) clearTimeout(searchTimer)
    searchTimer = setTimeout(() => {
      void runSearch(raw)
    }, SEARCH_DEBOUNCE_MS)
  })

  /** 删除单条：本地列表先移除，再刷新统计（失败时回滚交给 refresh） */
  async function removeItem(id: string): Promise<number> {
    const removed = await deleteClipboardItem(id)
    items.value = items.value.filter((item) => item.id !== id)
    results.value = results.value.filter((item) => item.id !== id)
    void loadStats()
    return removed
  }

  /** 重建首屏（清空后调用） */
  async function reload() {
    items.value = []
    results.value = []
    query.value = ''
    hasMore.value = true
    await refresh()
  }

  onMounted(() => {
    if (autoStart) void refresh()
    if (!listenEvents) return
    void (async () => {
      unlistenChanged = await listen('clipboard-changed', () => {
        void refresh()
      })
      unlistenShown = await listen('clipboard-panel-shown', () => {
        void refresh()
      })
    })()
  })

  onUnmounted(() => {
    if (searchTimer) clearTimeout(searchTimer)
    unlistenChanged?.()
    unlistenShown?.()
  })

  return {
    items,
    results,
    query,
    list,
    loading,
    loadingMore,
    searching,
    searchingMode,
    hasMore,
    error,
    stats,
    refresh,
    loadMore,
    runSearch,
    removeItem,
    reload,
    loadStats,
  }
}
