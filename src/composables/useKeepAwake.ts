import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import {
  keepAwakeStart,
  keepAwakeStatus,
  keepAwakeStop,
} from '@/api/sleep'
import { errorText } from '@/utils/error'
import type { KeepAwakeStatus } from '@/types/sleep'

/**
 * 状态轮询间隔：后端到点会自动释放断言，5 秒一次足够把「已自动停止」显示出来；
 * 剩余时间在面板里用本地时钟逐秒推算，不依赖轮询精度。
 */
const STATUS_POLL_INTERVAL_MS = 5000

/**
 * 「屏幕休眠」页签的状态与动作。
 *
 * 与 `useCleaner` 的内存采样同一套节拍：页面隐藏时只暂停计时器，
 * 恢复可见后由 `visibilitychange` 立即补一次；真正的「5 秒轮询」只在
 * 页签激活时由面板调用 `startPolling` 开启。
 */
export function useKeepAwake() {
  const status = ref<KeepAwakeStatus | null>(null)
  /** 轮询本身就失败时的提示（后端不可用等） */
  const statusError = ref('')
  /** 开始/停止命令本身失败的原因 */
  const actionError = ref('')
  const starting = ref(false)
  const stopping = ref(false)
  /** 是否「应该」轮询：页面不可见时只是暂停计时器，这个标记仍为 true */
  const polling = ref(false)

  let timer: ReturnType<typeof setTimeout> | null = null

  /** 是否正在保持唤醒（模板里直接用） */
  const active = computed(() => status.value?.active ?? false)

  function clearTimer(): void {
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
  }

  /** 单次拉状态：失败只在页面内提示，不弹全局错误，也不停掉轮询 */
  async function refresh(): Promise<void> {
    try {
      status.value = await keepAwakeStatus()
      statusError.value = ''
    } catch (err) {
      statusError.value = errorText(err, '读取保持唤醒状态失败')
    }
  }

  /** 递归 setTimeout：上一次请求结束再排下一次，避免慢请求堆叠 */
  async function tick(): Promise<void> {
    if (!polling.value) return
    await refresh()
    if (!polling.value) return
    // 页面隐藏期间不排下一次；恢复可见时由 visibilitychange 立即补一次
    if (typeof document !== 'undefined' && document.hidden) return
    clearTimer()
    timer = setTimeout(() => {
      void tick()
    }, STATUS_POLL_INTERVAL_MS)
  }

  /** 页签激活时调用；页面不可见时只置标记，等可见后补拉 */
  function startPolling(): void {
    if (polling.value) return
    polling.value = true
    if (typeof document !== 'undefined' && document.hidden) return
    void tick()
  }

  function stopPolling(): void {
    polling.value = false
    clearTimer()
  }

  /**
   * 开始保持唤醒；成功返回 true（后端已经把状态回传）。
   * 失败原因放 actionError，由面板展示错误条。
   */
  async function start(
    durationSeconds: number | null,
    allowDisplaySleep: boolean,
  ): Promise<boolean> {
    if (starting.value) return false
    starting.value = true
    actionError.value = ''
    try {
      status.value = await keepAwakeStart(durationSeconds, allowDisplaySleep)
      statusError.value = ''
      return true
    } catch (err) {
      actionError.value = errorText(err, '开启保持唤醒失败')
      return false
    } finally {
      starting.value = false
    }
  }

  /** 停止保持唤醒；失败时重新拉一次状态，避免 UI 与后端不一致 */
  async function stop(): Promise<boolean> {
    if (stopping.value) return false
    stopping.value = true
    actionError.value = ''
    try {
      status.value = await keepAwakeStop()
      statusError.value = ''
      return true
    } catch (err) {
      actionError.value = errorText(err, '停止保持唤醒失败')
      await refresh()
      return false
    } finally {
      stopping.value = false
    }
  }

  function onVisibilityChange(): void {
    if (typeof document === 'undefined') return
    if (document.hidden) {
      clearTimer()
      return
    }
    if (polling.value && timer === null) void tick()
  }

  onMounted(() => {
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', onVisibilityChange)
    }
  })

  onUnmounted(() => {
    stopPolling()
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange)
    }
  })

  // 用 reactive 包一层：模板里直接写 keepAwake.status / keepAwake.start(...)
  return reactive({
    status,
    statusError,
    actionError,
    starting,
    stopping,
    polling,
    active,
    refresh,
    startPolling,
    stopPolling,
    start,
    stop,
  })
}

/** 「屏幕休眠」面板与外部共享的接口类型（和 useCleaner 的 CleanerApi 一个用法） */
export type KeepAwakeApi = ReturnType<typeof useKeepAwake>
