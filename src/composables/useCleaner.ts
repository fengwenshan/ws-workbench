import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { CLEANER_PROGRESS_EVENT, listInstalledApps } from '@/api/cleaner'
import { errorText } from '@/utils/error'
import type { CleanerProgress, InstalledApp } from '@/types/cleaner'

/**
 * 「系统清理」的安装列表状态。
 *
 * 安装列表只在「软件卸载」页签组件首次挂载时拉一次（后端有缓存）；`cleaner-progress`
 * 事件用来驱动列表扫描进度。
 *
 * 卸载 / 关联文件 / 残留的编排在独立卸载窗口（`src/uninstall/UninstallWindow.vue`）
 * 里，有意不经过这里：两代编排并存时，改错地方会让页面看起来毫无反应。
 */
export function useCleaner() {
  const apps = ref<InstalledApp[]>([])
  const appsLoading = ref(false)
  const appsError = ref('')
  /** 安装列表扫描进度（只认 task === 'list_apps' 的事件） */
  const progress = ref<CleanerProgress | null>(null)

  const unlisteners: UnlistenFn[] = []

  const scanPercent = computed(() => {
    const current = progress.value
    if (!current || current.total <= 0) return 0
    return Math.min(100, Math.round((current.current / current.total) * 100))
  })

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

  // 用 reactive 包一层：模板里直接写 cleaner.apps / cleaner.loadApps，不用到处 .value
  return reactive({
    apps,
    appsLoading,
    appsError,
    progress,
    scanPercent,
    loadApps,
  })
}

/** 清理页与子面板之间共享的接口类型（和 useVpn 的 VpnApi 一个用法） */
export type CleanerApi = ReturnType<typeof useCleaner>
