import { computed, onMounted, onUnmounted, ref } from 'vue'
import { safeGet, safeSet } from './safeStorage'
import { platform } from '@/platform'

/** 工具页标识：日志是右侧抽屉，不属于工具页 */
export type AppView = 'home' | 'proxy' | 'cleaner' | 'settings'

const TREE_WIDTH_KEY = 'ws-workbench:layout:tree-width'
const TREE_COLLAPSED_KEY = 'ws-workbench:layout:tree-collapsed'
const LOG_OPEN_KEY = 'ws-workbench:layout:log-open'
const TREE_WIDTH_DEFAULT = 260
const TREE_WIDTH_MIN = 200
const TREE_WIDTH_MAX = 420

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
  const isMac = computed(() => platform.is_macos)

  const treeWidth = ref(TREE_WIDTH_DEFAULT)
  const treeCollapsed = ref(false)
  /** 应用日志抽屉是否打开（持久化，不改尺寸） */
  const logOpen = ref(false)
  /** 代理页左侧 VPN 配置列表是否收起（再次点击「代理」切换） */
  const vpnMenuCollapsed = ref(false)

  type DragState = { kind: 'tree'; startX: number; startWidth: number }
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
    if (tool === 'proxy' && view.value === 'proxy') {
      vpnMenuCollapsed.value = !vpnMenuCollapsed.value
      return
    }
    view.value = tool
    if (tool === 'home' && treeCollapsed.value) setTreeCollapsed(false)
    if (tool === 'proxy') vpnMenuCollapsed.value = false
  }

  function shortcutTitle(label: string, key: string) {
    return `${label} (${isMac.value ? '⌘' : 'Ctrl+'}${key})`
  }

  function startTreeResize(event: PointerEvent) {
    event.preventDefault()
    drag = { kind: 'tree', startX: event.clientX, startWidth: treeWidth.value }
    ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  }

  function onResizeMove(event: PointerEvent) {
    if (!drag) return
    treeWidth.value = clamp(
      drag.startWidth + event.clientX - drag.startX,
      TREE_WIDTH_MIN,
      TREE_WIDTH_MAX,
    )
  }

  function onResizeEnd() {
    if (!drag) return
    void safeSet(TREE_WIDTH_KEY, String(treeWidth.value))
    drag = null
  }

  function resetTreeWidth() {
    setTreeCollapsed(false)
    treeWidth.value = TREE_WIDTH_DEFAULT
    void safeSet(TREE_WIDTH_KEY, String(TREE_WIDTH_DEFAULT))
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
    isMac,
    shortcutTitle,
    selectTool,
    treeWidth,
    treeCollapsed,
    vpnMenuCollapsed,
    toggleTree,
    logOpen,
    setLogOpen,
    toggleLogs,
    startTreeResize,
    resetTreeWidth,
    onTreeResizerKeydown,
  }
}
