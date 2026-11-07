import { computed, reactive, ref } from 'vue'
import {
  getAppAutostart,
  listStartupItems,
  setAppAutostart as setAppAutostartApi,
  setStartupEnabled as setStartupEnabledApi,
} from '@/api/startup'
import { errorText } from '@/utils/error'
import { LOGIN_ITEMS_UNAVAILABLE_ID } from '@/types/startup'
import type { StartupItem } from '@/types/startup'

/**
 * 「开机自启」页签的状态与动作（设计 4.2）。
 *
 * 页面进入时拉一次列表与本应用自启状态；启停后直接用后端返回的刷新列表替换，
 * 不再本地猜测状态（Windows 的 StartupApproved 位 / macOS 的 launchctl override
 * 只有后端重新枚举才准）。
 */
export function useStartup() {
  // ========== 启动项列表 ==========
  const items = ref<StartupItem[]>([])
  const loading = ref(false)
  const error = ref('')
  /** 正在切换的条目 id：行内 loading + 防连点 */
  const togglingIds = ref<string[]>([])
  /** 切换失败的原因（列表本身仍可用） */
  const actionError = ref('')

  // ========== 本应用登录启动 ==========
  const appAutostart = ref(false)
  const appAutostartLoading = ref(false)
  const appAutostartSaving = ref(false)
  const appAutostartError = ref('')

  // ========== 派生状态 ==========
  /** 登录项授权被拒（macOS 会返回一条只读占位项） */
  const loginItemsUnavailable = computed(() =>
    items.value.some((item) => item.id === LOGIN_ITEMS_UNAVAILABLE_ID),
  )
  const loginItemsDetail = computed(
    () =>
      items.value.find((item) => item.id === LOGIN_ITEMS_UNAVAILABLE_ID)?.detail ?? '',
  )
  const toggleableCount = computed(
    () => items.value.filter((item) => item.can_toggle).length,
  )
  const enabledCount = computed(
    () => items.value.filter((item) => item.enabled && item.can_toggle).length,
  )

  // ============================ 列表 ============================

  /** 拉启动项列表；失败保留已有数据，只把错误文案交给页面里的错误条 */
  async function loadItems(): Promise<void> {
    if (loading.value) return
    loading.value = true
    error.value = ''
    try {
      items.value = await listStartupItems()
    } catch (err) {
      error.value = errorText(err, '读取启动项失败')
    } finally {
      loading.value = false
    }
  }

  /** 本应用登录启动状态；读失败不弹错，只记在卡片里 */
  async function loadAppAutostart(): Promise<void> {
    if (appAutostartLoading.value) return
    appAutostartLoading.value = true
    appAutostartError.value = ''
    try {
      appAutostart.value = await getAppAutostart()
    } catch (err) {
      appAutostartError.value = errorText(err, '读取登录启动状态失败')
    } finally {
      appAutostartLoading.value = false
    }
  }

  /** 首次进入页签时调用：列表与自启状态并行拉取 */
  async function load(): Promise<void> {
    await Promise.all([loadItems(), loadAppAutostart()])
  }

  /** 「刷新」按钮：两个都重新拉一次 */
  async function refresh(): Promise<void> {
    await load()
  }

  // ============================ 启停 ============================

  /**
   * 切换一条启动项。成功用后端返回的列表整体替换；失败保留原列表 + 错误文案。
   * 只读项（can_toggle=false）在前端直接拦掉，后端也会再校验一次。
   */
  async function setEnabled(item: StartupItem, enabled: boolean): Promise<boolean> {
    if (!item.can_toggle || togglingIds.value.includes(item.id)) return false
    togglingIds.value = [...togglingIds.value, item.id]
    actionError.value = ''
    try {
      items.value = await setStartupEnabledApi(item.id, enabled)
      return true
    } catch (err) {
      actionError.value = errorText(err, '切换启动项失败')
      return false
    } finally {
      togglingIds.value = togglingIds.value.filter((id) => id !== item.id)
    }
  }

  /** 设置本应用登录启动；成功以后端返回值为准 */
  async function setAppAutostart(enabled: boolean): Promise<boolean> {
    if (appAutostartSaving.value) return false
    appAutostartSaving.value = true
    appAutostartError.value = ''
    try {
      appAutostart.value = await setAppAutostartApi(enabled)
      return true
    } catch (err) {
      appAutostartError.value = errorText(err, '设置登录启动失败')
      return false
    } finally {
      appAutostartSaving.value = false
    }
  }

  // 用 reactive 包一层：模板里直接写 startup.items / startup.appAutostart
  return reactive({
    items,
    loading,
    error,
    togglingIds,
    actionError,
    appAutostart,
    appAutostartLoading,
    appAutostartSaving,
    appAutostartError,
    loginItemsUnavailable,
    loginItemsDetail,
    toggleableCount,
    enabledCount,
    loadItems,
    loadAppAutostart,
    load,
    refresh,
    setEnabled,
    setAppAutostart,
  })
}

/** 页签与面板之间共享的接口类型（和 useCleaner 的 CleanerApi 一个用法） */
export type StartupApi = ReturnType<typeof useStartup>
