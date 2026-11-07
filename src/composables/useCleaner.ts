import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  CLEANER_PROGRESS_EVENT,
  deleteLeftovers as deleteLeftoversApi,
  listInstalledApps,
  scanLeftovers as scanLeftoversApi,
  uninstallApp as uninstallAppApi,
} from '@/api/cleaner'
import { errorText } from '@/utils/error'
import type {
  CleanerProgress,
  DeleteOutcome,
  InstalledApp,
  LeftoverItem,
  UninstallOutcome,
} from '@/types/cleaner'

/**
 * 「系统清理」的状态与动作。
 *
 * 安装列表只在进入页面时拉一次（后端有缓存）；`cleaner-progress` 事件用来驱动列表扫描进度。
 * 卸载 / 残留 / 进度是页面全部职责（内存占用功能已移除）。
 */
export function useCleaner() {
  // ========== 安装列表 ==========
  const apps = ref<InstalledApp[]>([])
  const appsLoading = ref(false)
  const appsError = ref('')
  /** 安装列表扫描进度（只认 task === 'list_apps' 的事件） */
  const progress = ref<CleanerProgress | null>(null)

  const scanPercent = computed(() => {
    const current = progress.value
    if (!current || current.total <= 0) return 0
    return Math.min(100, Math.round((current.current / current.total) * 100))
  })

  // ========== 卸载 ==========
  const uninstalling = ref(false)
  const uninstallOutcome = ref<UninstallOutcome | null>(null)

  // ========== 残留 ==========
  const leftovers = ref<LeftoverItem[]>([])
  const leftoversLoading = ref(false)
  const leftoversError = ref('')
  const deleting = ref(false)
  const deleteOutcome = ref<DeleteOutcome | null>(null)

  /** 动作类错误（删除残留等命令本身失败） */
  const actionError = ref('')

  const unlisteners: UnlistenFn[] = []

  // ============================ 安装列表 ============================

  /**
   * 拉取安装列表。
   * 失败时保留已有数据，只把错误文案交给页面里的错误条（设计 9.1）。
   */
  async function loadApps(refresh = false): Promise<void> {
    if (appsLoading.value) return
    appsLoading.value = true
    appsError.value = ''
    progress.value = null
    try {
      apps.value = await listInstalledApps(refresh)
    } catch (err) {
      appsError.value = errorText(err, '读取已安装应用失败')
    } finally {
      appsLoading.value = false
      progress.value = null
    }
  }

  // ============================ 卸载 ============================

  function resetUninstallOutcome(): void {
    uninstallOutcome.value = null
  }

  /**
   * 执行卸载；失败不抛异常，统一塞进 UninstallOutcome.ok=false，
   * 让弹窗能同时展示「实际命令 + 退出码 + 错误」。
   */
  async function uninstall(appId: string): Promise<UninstallOutcome | null> {
    if (!appId || uninstalling.value) return null
    uninstalling.value = true
    uninstallOutcome.value = null
    actionError.value = ''
    try {
      const outcome = await uninstallAppApi(appId)
      uninstallOutcome.value = outcome
      // 成功后本地先摘掉这一行，避免用户回到列表还看到已卸载的应用
      if (outcome.ok) {
        apps.value = apps.value.filter((app) => app.id !== appId)
      }
      return outcome
    } catch (err) {
      const outcome: UninstallOutcome = {
        ok: false,
        command: '',
        exit_code: null,
        elevated: false,
        message: errorText(err, '卸载失败'),
      }
      uninstallOutcome.value = outcome
      return outcome
    } finally {
      uninstalling.value = false
    }
  }

  // ============================ 残留 ============================

  /** 扫描残留；失败时保留错误文案给弹窗里的错误条 + 重试按钮 */
  async function scanLeftovers(appId: string): Promise<LeftoverItem[]> {
    if (!appId) return []
    leftoversLoading.value = true
    leftoversError.value = ''
    deleteOutcome.value = null
    leftovers.value = []
    try {
      leftovers.value = await scanLeftoversApi(appId)
      return leftovers.value
    } catch (err) {
      leftoversError.value = errorText(err, '扫描残留失败')
      return []
    } finally {
      leftoversLoading.value = false
    }
  }

  /**
   * 删除勾选的残留。成功条目从列表移除、失败条目保留；
   * 命令整体失败时按「逐条失败」处理，方便页面统一展示每条结果。
   */
  async function deleteLeftovers(
    appId: string,
    paths: string[],
  ): Promise<DeleteOutcome | null> {
    if (!appId || paths.length === 0 || deleting.value) return null
    deleting.value = true
    deleteOutcome.value = null
    actionError.value = ''
    try {
      const outcome = await deleteLeftoversApi(appId, paths)
      deleteOutcome.value = outcome
      const failed = new Set(outcome.failed.map((item) => item.path))
      leftovers.value = leftovers.value.filter(
        (item) => !paths.includes(item.path) || failed.has(item.path),
      )
      return outcome
    } catch (err) {
      const message = errorText(err, '删除失败')
      const outcome: DeleteOutcome = {
        trashed: 0,
        failed: paths.map((path) => ({ path, error: message })),
      }
      deleteOutcome.value = outcome
      actionError.value = message
      return outcome
    } finally {
      deleting.value = false
    }
  }

  function resetDeleteOutcome(): void {
    deleteOutcome.value = null
  }

  // ============================ 生命周期 ============================

  onMounted(async () => {
    unlisteners.push(
      await listen<CleanerProgress>(CLEANER_PROGRESS_EVENT, (event) => {
        // 列表扫描进度驱动安装列表的进度条；其余任务的进度交给各弹窗的 loading 文案
        if (event.payload?.task === 'list_apps') progress.value = event.payload
      }),
    )
  })

  onUnmounted(() => {
    unlisteners.forEach((unlisten) => unlisten())
    unlisteners.length = 0
  })

  // 用 reactive 包一层：模板里直接写 cleaner.apps / cleaner.uninstallOutcome，不用到处 .value
  return reactive({
    // 安装列表
    apps,
    appsLoading,
    appsError,
    progress,
    scanPercent,
    loadApps,
    // 卸载
    uninstalling,
    uninstallOutcome,
    uninstall,
    resetUninstallOutcome,
    // 残留
    leftovers,
    leftoversLoading,
    leftoversError,
    scanLeftovers,
    deleting,
    deleteOutcome,
    deleteLeftovers,
    resetDeleteOutcome,
    // 其它
    actionError,
  })
}

/** 清理页与子面板之间共享的接口类型（和 useVpn 的 VpnApi 一个用法） */
export type CleanerApi = ReturnType<typeof useCleaner>
