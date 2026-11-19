import { computed, onMounted, onUnmounted, ref } from 'vue'
import type { InjectionKey } from 'vue'
import { safeGet, safeSet } from './safeStorage'
import { platform } from '@/platform'

/** 工具页标识 */
export type AppView = 'home' | 'proxy' | 'cleaner' | 'lanshare' | 'settings'

const PANE_WIDTH_KEY = 'ws-workbench:layout:tree-width'
const TREE_COLLAPSED_KEY = 'ws-workbench:layout:tree-collapsed'
/**
 * 左栏宽度：项目树 / VPN / 系统 / LANLink / 设置共用同一个值。
 * App.vue 把它写成 CSS 变量 --pane-left-width，切换工具时右栏起点不跳。
 * 持久化键沿用历史 tree-width，避免升级后宽度丢失。
 */
const PANE_WIDTH_DEFAULT = 260
const PANE_WIDTH_MIN = 200
const PANE_WIDTH_MAX = 420

/** 左栏拖拽条需要的最小能力；由 App.vue provide，各模块的左栏拖拽条 inject */
export interface PaneResizeApi {
  start: (event: PointerEvent) => void
  keydown: (event: KeyboardEvent) => void
  reset: () => void
}

export const PANE_RESIZE_KEY: InjectionKey<PaneResizeApi> = Symbol('pane-resize')

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max)
}

/**
 * 工作台布局与工具切换：
 * 左栏宽度、折叠状态、快捷键、拖拽与持久化都收在这里，
 * App.vue 只负责组装界面。
 */
export function useWorkbenchLayout() {
  const view = ref<AppView>('home')
  const isMac = computed(() => platform.is_macos)

  const paneWidth = ref(PANE_WIDTH_DEFAULT)
  const treeCollapsed = ref(false)
  /** 代理页左侧 VPN 配置列表是否收起（再次点击「代理」切换） */
  const vpnMenuCollapsed = ref(false)
  /** 系统 / LANLink / 设置 的左侧模块栏是否收起（再次点击对应图标切换） */
  const cleanerMenuCollapsed = ref(false)
  const lanMenuCollapsed = ref(false)
  const settingsMenuCollapsed = ref(false)

  type DragState = { startX: number; startWidth: number }
  let drag: DragState | null = null

  function setTreeCollapsed(collapsed: boolean) {
    treeCollapsed.value = collapsed
    void safeSet(TREE_COLLAPSED_KEY, String(collapsed))
  }

  function toggleTree() {
    if (view.value !== 'home') {
      view.value = 'home'
      setTreeCollapsed(false)
      return
    }
    setTreeCollapsed(!treeCollapsed.value)
  }

  function selectTool(tool: AppView) {
    if (tool === 'home' && view.value === 'home') {
      setTreeCollapsed(!treeCollapsed.value)
      return
    }
    if (tool === 'proxy' && view.value === 'proxy') {
      vpnMenuCollapsed.value = !vpnMenuCollapsed.value
      return
    }
    if (tool === 'cleaner' && view.value === 'cleaner') {
      cleanerMenuCollapsed.value = !cleanerMenuCollapsed.value
      return
    }
    if (tool === 'lanshare' && view.value === 'lanshare') {
      lanMenuCollapsed.value = !lanMenuCollapsed.value
      return
    }
    if (tool === 'settings' && view.value === 'settings') {
      settingsMenuCollapsed.value = !settingsMenuCollapsed.value
      return
    }
    view.value = tool
    if (tool === 'home' && treeCollapsed.value) setTreeCollapsed(false)
    if (tool === 'proxy') vpnMenuCollapsed.value = false
    if (tool === 'cleaner') cleanerMenuCollapsed.value = false
    if (tool === 'lanshare') lanMenuCollapsed.value = false
    if (tool === 'settings') settingsMenuCollapsed.value = false
  }

  /**
   * 程序化跳回项目页：只切视图、必要时展开项目树。
   * 点「项目」图标走 selectTool——那套语义是「再点一次就把项目树收起」，
   * 搜索面板跳转不能带这个副作用（在首页选搜索结果会把树收起来）。
   */
  function openHome() {
    view.value = 'home'
    if (treeCollapsed.value) setTreeCollapsed(false)
  }

  function shortcutTitle(label: string, key: string) {
    return `${label} (${isMac.value ? '⌘' : 'Ctrl+'}${key})`
  }

  function startPaneResize(event: PointerEvent) {
    event.preventDefault()
    drag = { startX: event.clientX, startWidth: paneWidth.value }
    ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  }

  function onResizeMove(event: PointerEvent) {
    if (!drag) return
    paneWidth.value = clamp(
      drag.startWidth + event.clientX - drag.startX,
      PANE_WIDTH_MIN,
      PANE_WIDTH_MAX,
    )
  }

  function onResizeEnd() {
    if (!drag) return
    void safeSet(PANE_WIDTH_KEY, String(paneWidth.value))
    drag = null
  }

  function resetPaneWidth() {
    paneWidth.value = PANE_WIDTH_DEFAULT
    void safeSet(PANE_WIDTH_KEY, String(PANE_WIDTH_DEFAULT))
  }

  function onPaneResizerKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
    event.preventDefault()
    const delta = event.key === 'ArrowRight' ? 16 : -16
    paneWidth.value = clamp(
      paneWidth.value + delta,
      PANE_WIDTH_MIN,
      PANE_WIDTH_MAX,
    )
    void safeSet(PANE_WIDTH_KEY, String(paneWidth.value))
  }

  function isEditableTarget(target: EventTarget | null) {
    const element = target as HTMLElement | null
    if (!element) return false
    const tag = element.tagName.toLowerCase()
    return (
      tag === 'input' ||
      tag === 'textarea' ||
      tag === 'select' ||
      element.isContentEditable
    )
  }

  function handleShortcut(event: KeyboardEvent) {
    if (!(event.metaKey || event.ctrlKey) || event.altKey) return
    if (isEditableTarget(event.target)) return
    switch (event.key.toLowerCase()) {
      case '1':
        event.preventDefault()
        selectTool('home')
        break
      case '2':
        event.preventDefault()
        selectTool('proxy')
        break
      case '3':
        event.preventDefault()
        selectTool('settings')
        break
      case '4':
        event.preventDefault()
        selectTool('cleaner')
        break
      case '5':
        event.preventDefault()
        selectTool('lanshare')
        break
      case 'b':
        event.preventDefault()
        toggleTree()
        break
    }
  }

  async function restoreLayout() {
    const width = Number(await safeGet(PANE_WIDTH_KEY))
    if (Number.isFinite(width) && width > 0) {
      paneWidth.value = clamp(width, PANE_WIDTH_MIN, PANE_WIDTH_MAX)
    }
    const collapsed = await safeGet(TREE_COLLAPSED_KEY)
    if (collapsed === 'true' || collapsed === 'false') {
      treeCollapsed.value = collapsed === 'true'
    }
  }

  onMounted(() => {
    void restoreLayout()
    window.addEventListener('keydown', handleShortcut)
    window.addEventListener('pointermove', onResizeMove)
    window.addEventListener('pointerup', onResizeEnd)
  })
  onUnmounted(() => {
    window.removeEventListener('keydown', handleShortcut)
    window.removeEventListener('pointermove', onResizeMove)
    window.removeEventListener('pointerup', onResizeEnd)
  })

  return {
    view,
    isMac,
    shortcutTitle,
    selectTool,
    openHome,
    paneWidth,
    treeCollapsed,
    vpnMenuCollapsed,
    cleanerMenuCollapsed,
    lanMenuCollapsed,
    settingsMenuCollapsed,
    toggleTree,
    startPaneResize,
    resetPaneWidth,
    onPaneResizerKeydown,
  }
}
