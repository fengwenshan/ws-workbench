import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  CLEANER_PROGRESS_EVENT,
  deepRelease,
  deleteLeftovers as deleteLeftoversApi,
  listInstalledApps,
  memorySnapshot,
  quitApps as quitAppsApi,
  scanLeftovers as scanLeftoversApi,
  uninstallApp as uninstallAppApi,
} from '@/api/cleaner'
import { errorText } from '@/utils/error'
import type {
  CleanerProgress,
  DeleteOutcome,
  InstalledApp,
  LeftoverItem,
  MemorySnapshot,
  QuitOutcome,
  ReleaseOutcome,
  UninstallOutcome,
} from '@/types/cleaner'

/** 内存采样间隔：与设计 5.3 的 2 秒一致 */
const MEMORY_POLL_INTERVAL_MS = 2000

/**
 * 「系统清理」的状态与动作。
 *
 * 安装列表只在进入页面时拉一次（后端有缓存），内存快照在内存页签激活时按 2 秒轮询，
 * 页面不可见 / 页签切走 / 卸载过程中都会停表；`cleaner-progress` 事件只用来驱动列表扫描进度。
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

  // ========== 内存 ==========
  const memory = ref<MemorySnapshot | null>(null)
  const memoryError = ref('')
  const sampling = ref(false)
  /** 是否「应该」轮询：页面不可见时只是暂停计时器，这个标记仍为 true */
  const memoryPolling = ref(false)
  const quitting = ref(false)
  const releasing = ref(false)
  const releaseOutcome = ref<ReleaseOutcome | null>(null)

  /** 动作类错误（结束进程 / 深度释放等命令本身失败）；采样错误单独放 memoryError */
  const actionError = ref('')

  let memoryTimer: ReturnType<typeof setTimeout> | null = null
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
    // 卸载期间停止内存采样：系统卸载器会拉起自己的进程，没必要同时读全量进程表
    stopMemoryPolling()
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

  // ============================ 内存采样 ============================

  function clearMemoryTimer(): void {
    if (memoryTimer !== null) {
      clearTimeout(memoryTimer)
      memoryTimer = null
    }
  }

  /** 单次采样：失败只在页面内提示，不弹全局错误，也不停掉轮询 */
  async function sampleMemory(): Promise<void> {
    if (sampling.value) return
    sampling.value = true
    try {
      memory.value = await memorySnapshot()
      memoryError.value = ''
    } catch (err) {
      memoryError.value = errorText(err, '内存采样失败')
    } finally {
      sampling.value = false
    }
  }

  /** 递归 setTimeout：上一次采样结束再排下一次，避免慢采样时请求堆叠 */
  async function memoryTick(): Promise<void> {
    if (!memoryPolling.value) return
    await sampleMemory()
    if (!memoryPolling.value) return
    // 页面隐藏期间不排下一次；恢复可见时由 visibilitychange 立即补一次
    if (typeof document !== 'undefined' && document.hidden) return
    clearMemoryTimer()
    memoryTimer = setTimeout(() => {
      void memoryTick()
    }, MEMORY_POLL_INTERVAL_MS)
  }

  /** 进入内存页签时调用；页面不可见时只置标记，等可见后补采 */
  function startMemoryPolling(): void {
    if (memoryPolling.value) return
    memoryPolling.value = true
    if (typeof document !== 'undefined' && document.hidden) return
    void memoryTick()
  }

  function stopMemoryPolling(): void {
    memoryPolling.value = false
    clearMemoryTimer()
  }

  function onVisibilityChange(): void {
    if (typeof document === 'undefined') return
    if (document.hidden) {
      clearMemoryTimer()
      return
    }
    if (memoryPolling.value && memoryTimer === null) void memoryTick()
  }

  // ============================ 结束进程 / 深度释放 ============================

  /** 结束进程；命令本身失败返回 null，原因放 actionError */
  async function quitApps(
    appIds: string[],
    force = false,
  ): Promise<QuitOutcome | null> {
    if (appIds.length === 0 || quitting.value) return null
    quitting.value = true
    actionError.value = ''
    try {
      return await quitAppsApi(appIds, force)
    } catch (err) {
      actionError.value = errorText(err, '结束进程失败')
      return null
    } finally {
      quitting.value = false
    }
  }

  /**
   * 深度释放；取消系统授权时后端返回 ok=false + message="已取消授权"，
   * 这里原样返回给页面做「不报错」的提示（设计 5.5）。
   */
  async function release(): Promise<ReleaseOutcome | null> {
    if (releasing.value) return null
    releasing.value = true
    releaseOutcome.value = null
    actionError.value = ''
    // 释放期间暂停轮询：否则会打断后端「释放前后各采样一次」的对比
    clearMemoryTimer()
    try {
      const outcome = await deepRelease()
      releaseOutcome.value = outcome
      return outcome
    } catch (err) {
      actionError.value = errorText(err, '深度释放失败')
      return null
    } finally {
      releasing.value = false
      // 恢复轮询：不立刻采样，让后端释放后的系统状态先稳定一下
      if (memoryPolling.value) {
        clearMemoryTimer()
        memoryTimer = setTimeout(() => {
          void memoryTick()
        }, MEMORY_POLL_INTERVAL_MS)
      }
    }
  }

  // ============================ 生命周期 ============================

  onMounted(async () => {
    unlisteners.push(
      await listen<CleanerProgress>(CLEANER_PROGRESS_EVENT, (event) => {
        // 列表扫描进度驱动安装列表的进度条；其余任务的进度交给各弹窗的 loading 文案
        if (event.payload?.task === 'list_apps') progress.value = event.payload
      }),
    )
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', onVisibilityChange)
    }
  })

  onUnmounted(() => {
    stopMemoryPolling()
    unlisteners.forEach((unlisten) => unlisten())
    unlisteners.length = 0
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange)
    }
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
    // 内存
    memory,
    memoryError,
    sampling,
    memoryPolling,
    sampleMemory,
    startMemoryPolling,
    stopMemoryPolling,
    quitting,
    quitApps,
    releasing,
    releaseOutcome,
    release,
    // 其它
    actionError,
  })
}

/** 清理页与子面板之间共享的接口类型（和 useVpn 的 VpnApi 一个用法） */
export type CleanerApi = ReturnType<typeof useCleaner>
