import { computed, onMounted, onUnmounted, ref } from 'vue'
import { safeGet, safeSet } from './safeStorage'
import { platform } from '@/platform'

/** 工具页标识：日志已改为工作区底部面板，不属于工具页 */
export type AppView = 'home' | 'proxy' | 'settings'

const VIEW_TITLES: Record<AppView, string> = {
  home: '项目',
  proxy: '代理',
  settings: '设置',
}

const TREE_WIDTH_KEY = 'ws-workbench:layout:tree-width'
const TREE_COLLAPSED_KEY = 'ws-workbench:layout:tree-collapsed'
const LOG_HEIGHT_KEY = 'ws-workbench:layout:log-height'
const LOG_OPEN_KEY = 'ws-workbench:layout:log-open'
const TREE_WIDTH_DEFAULT = 260
const TREE_WIDTH_MIN = 200
const TREE_WIDTH_MAX = 420
const LOG_HEIGHT_DEFAULT = 220
const LOG_HEIGHT_MIN = 120

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max)
}

/**
 * 工作台布局与工具切换：
 * 项目树 / 日志面板尺寸、折叠状态、快捷键、拖拽与持久化都收在这里，
 * App.vue 只负责组装界面。
 */
export function useWorkbenchLayout() {
  const view = ref<AppView>('home')
  const toolTitle = computed(() => VIEW_TITLES[view.value])
  const isMac = computed(() => platform.is_macos)

  const treeWidth = ref(TREE_WIDTH_DEFAULT)
  const treeCollapsed = ref(false)
  const logHeight = ref(LOG_HEIGHT_DEFAULT)
  const logOpen = ref(false)

  type DragState =
    | { kind: 'tree'; startX: number; startWidth: number }
    | { kind: 'log'; startY: number; startHeight: number }
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

  function setLogOpen(open: boolean) {
    logOpen.value = open
    void safeSet(LOG_OPEN_KEY, String(open))
  }

  function toggleLogs() {
    setLogOpen(!logOpen.value)
  }

  function selectTool(tool: AppView) {
    if (tool === 'home' && view.value === 'home') {
      setTreeCollapsed(!treeCollapsed.value)
      return
    }
    view.value = tool
    if (tool === 'home' && treeCollapsed.value) setTreeCollapsed(false)
  }

  function shortcutTitle(label: string, key: string) {
    return `${label} (${isMac.value ? '⌘' : 'Ctrl+'}${key})`
  }

  function startTreeResize(event: PointerEvent) {
    event.preventDefault()
    drag = { kind: 'tree', startX: event.clientX, startWidth: treeWidth.value }
    ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  }

  function startLogResize(event: PointerEvent) {
    event.preventDefault()
    drag = { kind: 'log', startY: event.clientY, startHeight: logHeight.value }
    ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  }

  function onResizeMove(event: PointerEvent) {
    if (!drag) return
    if (drag.kind === 'tree') {
      treeWidth.value = clamp(
        drag.startWidth + event.clientX - drag.startX,
        TREE_WIDTH_MIN,
        TREE_WIDTH_MAX,
      )
    } else {
      const maxHeight = Math.round(window.innerHeight * 0.5)
      logHeight.value = clamp(
        drag.startHeight + drag.startY - event.clientY,
        LOG_HEIGHT_MIN,
        maxHeight,
      )
    }
  }

  function onResizeEnd() {
    if (!drag) return
    if (drag.kind === 'tree') {
      void safeSet(TREE_WIDTH_KEY, String(treeWidth.value))
    } else {
      void safeSet(LOG_HEIGHT_KEY, String(logHeight.value))
    }
    drag = null
  }

  function resetTreeWidth() {
    setTreeCollapsed(false)
    treeWidth.value = TREE_WIDTH_DEFAULT
    void safeSet(TREE_WIDTH_KEY, String(TREE_WIDTH_DEFAULT))
  }

  function resetLogHeight() {
    logHeight.value = LOG_HEIGHT_DEFAULT
    void safeSet(LOG_HEIGHT_KEY, String(LOG_HEIGHT_DEFAULT))
  }

  function onTreeResizerKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
    event.preventDefault()
    const delta = event.key === 'ArrowRight' ? 16 : -16
    treeWidth.value = clamp(
      treeWidth.value + delta,
      TREE_WIDTH_MIN,
      TREE_WIDTH_MAX,
    )
    void safeSet(TREE_WIDTH_KEY, String(treeWidth.value))
  }

  function onLogResizerKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return
    event.preventDefault()
    const delta = event.key === 'ArrowUp' ? 16 : -16
    logHeight.value = clamp(
      logHeight.value + delta,
      LOG_HEIGHT_MIN,
      Math.round(window.innerHeight * 0.5),
    )
    void safeSet(LOG_HEIGHT_KEY, String(logHeight.value))
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
      case 'b':
        event.preventDefault()
        toggleTree()
        break
      case 'j':
        event.preventDefault()
        toggleLogs()
        break
    }
  }

  async function restoreLayout() {
    const width = Number(await safeGet(TREE_WIDTH_KEY))
    if (Number.isFinite(width) && width > 0) {
      treeWidth.value = clamp(width, TREE_WIDTH_MIN, TREE_WIDTH_MAX)
    }
    const collapsed = await safeGet(TREE_COLLAPSED_KEY)
    if (collapsed === 'true' || collapsed === 'false') {
      treeCollapsed.value = collapsed === 'true'
    }
    const height = Number(await safeGet(LOG_HEIGHT_KEY))
    if (Number.isFinite(height) && height > 0) {
      logHeight.value = clamp(
        height,
        LOG_HEIGHT_MIN,
        Math.round(window.innerHeight * 0.5),
      )
    }
    const open = await safeGet(LOG_OPEN_KEY)
    if (open === 'true' || open === 'false') {
      logOpen.value = open === 'true'
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
    toolTitle,
    isMac,
    shortcutTitle,
    selectTool,
    treeWidth,
    treeCollapsed,
    setTreeCollapsed,
    toggleTree,
    logHeight,
    logOpen,
    setLogOpen,
    toggleLogs,
    startTreeResize,
    startLogResize,
    resetTreeWidth,
    resetLogHeight,
    onTreeResizerKeydown,
    onLogResizerKeydown,
  }
}
