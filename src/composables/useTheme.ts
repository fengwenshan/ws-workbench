import { computed, ref } from 'vue'
import { safeGet, safeSet } from './safeStorage'

/**
 * 主题三档：跟随系统 / 浅色 / 深色。
 *
 * - 亮暗 token 在 `assets/styles/main.css` 的 `html[data-theme]` 下，
 *   这里只负责决定用哪套并写到 `<html>` 上；
 * - antd 的 token 由 `theme/antdTheme.ts` 生成，App.vue 的 ConfigProvider 消费；
 * - 偏好走 `safeStorage` 加密持久化（规范见 docs/ui.md「主题」）。
 *
 * 初始化顺序：先按系统外观同步落一次 data-theme（避免首帧闪烁），
 * 再异步读本地偏好覆盖。
 */
export type ThemeMode = 'system' | 'light' | 'dark'
export type ResolvedTheme = 'light' | 'dark'

const STORAGE_KEY = 'ws-workbench:ui:theme'

function systemPrefersDark(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false
  return window.matchMedia('(prefers-color-scheme: dark)').matches
}

const mode = ref<ThemeMode>('system')
const systemDark = ref(systemPrefersDark())
/** 当前实际生效的亮暗（system 时跟随系统） */
const resolved = computed<ResolvedTheme>(() =>
  mode.value === 'system' ? (systemDark.value ? 'dark' : 'light') : mode.value,
)

let media: MediaQueryList | null = null
let initPromise: Promise<void> | null = null

function applyResolved(): void {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.dataset.theme = resolved.value
  root.style.colorScheme = resolved.value
}

function watchSystem(): void {
  if (media || typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
  media = window.matchMedia('(prefers-color-scheme: dark)')
  media.addEventListener('change', (event) => {
    systemDark.value = event.matches
    if (mode.value === 'system') applyResolved()
  })
}

/** 切换并持久化；`system` 表示交还给系统外观 */
export async function setThemeMode(next: ThemeMode): Promise<void> {
  mode.value = next
  applyResolved()
  await safeSet(STORAGE_KEY, next)
}

/**
 * 初始化主题，可重复调用（幂等）。
 * 主窗口、快捷面板、剪贴板窗口各自调用一次。
 */
export function initTheme(): Promise<void> {
  if (initPromise) return initPromise
  applyResolved()
  watchSystem()
  initPromise = (async () => {
    const stored = await safeGet(STORAGE_KEY)
    if (stored === 'light' || stored === 'dark' || stored === 'system') {
      mode.value = stored
      applyResolved()
    }
  })()
  return initPromise
}

/** 组件里用的主题状态：只读 mode/resolved，改主题调用 setThemeMode */
export function useTheme() {
  void initTheme()
  return { mode, resolved, setThemeMode }
}
