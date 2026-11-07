import { computed, onUnmounted, reactive, ref, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  POPUP_DETECTED_EVENT,
  popupClear,
  popupDismiss,
  popupIgnore,
  popupRecent,
  popupStart,
  popupStatus,
  popupStop,
} from '@/api/popup'
import type { PopupHit, PopupStatus } from '@/types/popup'
import { errorText } from '@/utils/error'

/** 页签激活时补拉一次列表的间隔：事件已经实时推送，这里只做兜底 */
const RECENT_POLL_INTERVAL_MS = 5000

/**
 * 「弹窗拦截」页签的状态与动作。
 *
 * 只做检测：开/关检测线程、展示记录、用户逐条关闭/隐藏。
 * 每一条新记录由后端 `popup-detected` 事件推过来，列表本地维护。
 */
export function usePopup() {
  const status = ref<PopupStatus | null>(null)
  const hits = ref<PopupHit[]>([])
  const ignoredProcesses = ref<string[]>([])
  const loading = ref(false)
  const busy = ref(false)
  const errorMessage = ref('')

  const supported = computed(() => status.value?.supported ?? false)
  const running = computed(() => status.value?.running ?? false)

  let unlisten: UnlistenFn | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  let polling = false

  function clearTimer(): void {
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
  }

  async function refreshStatus(): Promise<void> {
    try {
      status.value = await popupStatus()
      errorMessage.value = ''
    } catch (err) {
      errorMessage.value = errorText(err, '读取弹窗检测状态失败')
    }
  }

  async function refreshRecent(): Promise<void> {
    try {
      hits.value = await popupRecent()
    } catch (err) {
      errorMessage.value = errorText(err, '读取弹窗记录失败')
    }
  }

  async function tick(): Promise<void> {
    if (!polling) return
    await refreshStatus()
    await refreshRecent()
    if (!polling) return
    clearTimer()
    timer = setTimeout(() => {
      void tick()
    }, RECENT_POLL_INTERVAL_MS)
  }

  async function subscribe(): Promise<void> {
    if (unlisten) return
    try {
      unlisten = await listen<PopupHit>(POPUP_DETECTED_EVENT, (event) => {
        const hit = event.payload
        if (!hit) return
        hits.value = [hit, ...hits.value].slice(0, 200)
      })
    } catch (err) {
      errorMessage.value = errorText(err, '订阅弹窗事件失败')
    }
  }

  function startPolling(): void {
    if (polling) return
    polling = true
    void subscribe()
    void tick()
  }

  function stopPolling(): void {
    polling = false
    clearTimer()
  }

  /** 页签卸载 / 切走时只停轮询，不自动关检测线程（用户开的检测保持运行） */
  onUnmounted(() => {
    stopPolling()
    if (unlisten) {
      unlisten()
      unlisten = null
    }
  })

  // 第一次拿到 supported=false 时没必要继续轮询
  watch(supported, (value) => {
    if (!value && status.value) stopPolling()
  })

  async function load(): Promise<void> {
    if (loading.value) return
    loading.value = true
    try {
      await refreshStatus()
      if (supported.value) {
        await refreshRecent()
        await subscribe()
      }
    } finally {
      loading.value = false
    }
  }

  async function toggle(next: boolean): Promise<boolean> {
    if (busy.value) return false
    busy.value = true
    errorMessage.value = ''
    try {
      status.value = next ? await popupStart() : await popupStop()
      if (next) await refreshRecent()
      return true
    } catch (err) {
      errorMessage.value = errorText(err, next ? '开启弹窗检测失败' : '停止弹窗检测失败')
      await refreshStatus()
      return false
    } finally {
      busy.value = false
    }
  }

  async function clear(): Promise<boolean> {
    busy.value = true
    errorMessage.value = ''
    try {
      await popupClear()
      hits.value = []
      await refreshStatus()
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '清空弹窗记录失败')
      return false
    } finally {
      busy.value = false
    }
  }

  async function dismiss(id: number, action: 'close' | 'hide'): Promise<boolean> {
    errorMessage.value = ''
    try {
      await popupDismiss(id, action)
      hits.value = hits.value.filter((hit) => hit.id !== id)
      await refreshStatus()
      return true
    } catch (err) {
      errorMessage.value = errorText(err, action === 'close' ? '关闭弹窗失败' : '隐藏弹窗失败')
      return false
    }
  }

  async function ignore(process: string, next: boolean): Promise<boolean> {
    errorMessage.value = ''
    try {
      ignoredProcesses.value = await popupIgnore(process, next)
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '更新忽略名单失败')
      return false
    }
  }

  return reactive({
    status,
    hits,
    ignoredProcesses,
    loading,
    busy,
    errorMessage,
    supported,
    running,
    load,
    startPolling,
    stopPolling,
    toggle,
    clear,
    dismiss,
    ignore,
  })
}

/** 「弹窗拦截」面板与外部共享的接口类型 */
export type PopupApi = ReturnType<typeof usePopup>
