import { computed, onUnmounted, reactive, ref, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { message } from 'ant-design-vue'
import {
  POPUP_DETECTED_EVENT,
  popupAppMeta,
  popupClear,
  popupDismiss,
  popupIgnore,
  popupIgnored,
  popupRecent,
  popupRuleAdd,
  popupRuleApply,
  popupRuleRemove,
  popupRules,
  popupRuleToggle,
  popupAppToggle,
  popupSettingsGet,
  popupSettingsSet,
  popupStart,
  popupStatus,
  popupStop,
  popupWindows,
} from '@/api/popup'
import type {
  PopupAction,
  PopupAppMeta,
  PopupHit,
  PopupRule,
  PopupRuleDraft,
  PopupSettings,
  PopupStatus,
  PopupWindow,
} from '@/types/popup'
import { errorText } from '@/utils/error'

/** 页签激活时补拉一次列表的间隔：事件已经实时推送，这里只做兜底 + 同步规则命中数 */
const POLL_INTERVAL_MS = 4000

const DEFAULT_SETTINGS: PopupSettings = {
  enabled: false,
  action: 'close',
  smart: false,
  min_score: 10,
  max_per_minute: 6,
}

/**
 * 「弹窗拦截」页签的状态与动作。
 *
 * - 记录由后端 `popup-detected` 事件实时推送，轮询只做兜底；
 * - 规则命中后后端直接处理窗口，前端这里只负责把记录/统计刷新出来；
 * - 应用图标按 exe 路径懒加载并缓存，第一次会慢一点，之后走缓存。
 */
export function usePopup() {
  const status = ref<PopupStatus | null>(null)
  const hits = ref<PopupHit[]>([])
  const rules = ref<PopupRule[]>([])
  const ignoredProcesses = ref<string[]>([])
  const settings = ref<PopupSettings>({ ...DEFAULT_SETTINGS })
  const windows = ref<PopupWindow[]>([])
  const meta = ref<Record<string, PopupAppMeta>>({})
  const windowsLoading = ref(false)
  const loading = ref(false)
  const busy = ref(false)
  const errorMessage = ref('')

  const supported = computed(() => status.value?.supported ?? false)
  const running = computed(() => status.value?.running ?? false)
  const blockedHits = computed(() => hits.value.filter((hit) => !!hit.auto_action))
  const pendingHits = computed(() => hits.value.filter((hit) => !hit.auto_action))
  const enabledRules = computed(() => rules.value.filter((rule) => rule.enabled))
  const lastBlockedAt = computed(() => blockedHits.value[0]?.at_ms ?? 0)
  /** 所有规则持久化的命中数之和：列表顶部的「N 项弹窗已拦截」 */
  const totalHits = computed(() => rules.value.reduce((sum, rule) => sum + rule.hits, 0))
  /** 规则覆盖的应用数（按进程名去重） */
  const appCount = computed(
    () => new Set(rules.value.map((rule) => rule.process.toLowerCase())).size,
  )
  const lastHitAt = computed(() => {
    const fromRules = rules.value.reduce((max, rule) => Math.max(max, rule.last_hit_ms), 0)
    return Math.max(fromRules, lastBlockedAt.value)
  })

  let unlisten: UnlistenFn | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  let polling = false
  let tickBusy = false
  /** 事件推来的本地变更计数：轮询结果回来时若已变化就丢弃，避免把新记录覆盖掉 */
  let localVersion = 0
  /** 正在请求图标的应用路径，避免每个 tick 重复发 */
  const metaPending = new Set<string>()

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
      errorMessage.value = errorText(err, '读取弹窗拦截状态失败')
    }
  }

  async function refreshSettings(): Promise<void> {
    try {
      settings.value = await popupSettingsGet()
    } catch (err) {
      errorMessage.value = errorText(err, '读取拦截设置失败')
    }
  }

  async function refreshRules(): Promise<void> {
    try {
      rules.value = await popupRules()
      void ensureMeta(rules.value)
    } catch (err) {
      errorMessage.value = errorText(err, '读取拦截规则失败')
    }
  }

  async function refreshRecent(): Promise<void> {
    const startedAt = localVersion
    try {
      const recent = await popupRecent()
      if (localVersion === startedAt) {
        hits.value = recent
      } else {
        // 期间有实时事件，按 id 合并，别把刚推送的那条冲掉
        const known = new Set(recent.map((hit) => hit.id))
        hits.value = [...hits.value.filter((hit) => !known.has(hit.id)), ...recent].slice(0, 300)
      }
      void ensureMeta(hits.value)
    } catch (err) {
      errorMessage.value = errorText(err, '读取弹窗记录失败')
    }
  }

  async function refreshIgnored(): Promise<void> {
    try {
      ignoredProcesses.value = await popupIgnored()
    } catch (err) {
      errorMessage.value = errorText(err, '读取忽略名单失败')
    }
  }

  /** 批量补应用图标 / 展示名；失败静默（列表退回进程名 + 首字母色块） */
  async function ensureMeta(items: Array<{ path?: string }>): Promise<void> {
    const missing: string[] = []
    for (const item of items) {
      const path = item.path?.trim()
      if (!path || meta.value[path] || metaPending.has(path)) continue
      metaPending.add(path)
      missing.push(path)
    }
    if (!missing.length) return
    try {
      const result = await popupAppMeta(missing)
      meta.value = { ...meta.value, ...result }
    } catch {
      // 图标只是锦上添花，取不到就用进程名兜底
    } finally {
      for (const path of missing) metaPending.delete(path)
    }
  }

  async function tick(): Promise<void> {
    if (!polling) return
    if (tickBusy) {
      clearTimer()
      timer = setTimeout(() => void tick(), POLL_INTERVAL_MS)
      return
    }
    tickBusy = true
    try {
      await refreshStatus()
      await refreshRecent()
      await refreshRules()
    } finally {
      tickBusy = false
    }
    if (!polling) return
    clearTimer()
    timer = setTimeout(() => void tick(), POLL_INTERVAL_MS)
  }

  async function subscribe(): Promise<void> {
    if (unlisten) return
    try {
      unlisten = await listen<PopupHit>(POPUP_DETECTED_EVENT, (event) => {
        const hit = event.payload
        if (!hit) return
        localVersion += 1
        hits.value = [hit, ...hits.value.filter((item) => item.id !== hit.id)].slice(0, 300)
        if (hit.auto_action) {
          message.success(`已拦截 ${displayName(hit.path, hit.process)} 的弹窗`)
          void refreshRules()
        }
        void ensureMeta([hit])
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
        await Promise.all([refreshSettings(), refreshRecent(), refreshRules(), refreshIgnored()])
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
      if (next) {
        await refreshRecent()
        await refreshRules()
      }
      return true
    } catch (err) {
      errorMessage.value = errorText(err, next ? '开启弹窗拦截失败' : '关闭弹窗拦截失败')
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

  async function dismiss(id: number, action: PopupAction): Promise<boolean> {
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

  /** 改拦截设置：只传要改的字段，其余沿用当前值；返回后端最终生效的设置 */
  async function saveSettings(patch: Partial<PopupSettings>): Promise<boolean> {
    errorMessage.value = ''
    try {
      settings.value = await popupSettingsSet({ ...settings.value, ...patch })
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '保存拦截设置失败')
      await refreshSettings()
      return false
    }
  }

  async function ignore(process: string, next: boolean): Promise<boolean> {
    errorMessage.value = ''
    try {
      ignoredProcesses.value = await popupIgnore(process, next)
      await refreshRecent()
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '更新忽略名单失败')
      return false
    }
  }

  async function addRule(draft: PopupRuleDraft, applyNow: boolean): Promise<boolean> {
    if (busy.value) return false
    busy.value = true
    errorMessage.value = ''
    try {
      const result = await popupRuleAdd(draft, applyNow)
      rules.value = result.rules
      await refreshRecent()
      await refreshStatus()
      void ensureMeta(rules.value)
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '保存拦截规则失败')
      return false
    } finally {
      busy.value = false
    }
  }

  async function removeRule(id: string): Promise<boolean> {
    errorMessage.value = ''
    try {
      rules.value = await popupRuleRemove(id)
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '删除拦截规则失败')
      return false
    }
  }

  async function toggleRule(id: string, enabled: boolean): Promise<boolean> {
    errorMessage.value = ''
    try {
      rules.value = await popupRuleToggle(id, enabled)
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '更新拦截规则失败')
      return false
    }
  }

  /** 按应用整体启用 / 停用（每个软件一行的总开关） */
  async function toggleApp(process: string, enabled: boolean): Promise<boolean> {
    errorMessage.value = ''
    try {
      rules.value = await popupAppToggle(process, enabled)
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '更新应用拦截规则失败')
      return false
    }
  }

  /** 对已有规则立即执行一次，返回处理了几个窗口；失败返回 null */
  async function applyRule(id: string): Promise<number | null> {
    errorMessage.value = ''
    try {
      const applied = await popupRuleApply(id)
      await refreshRecent()
      await refreshRules()
      await refreshStatus()
      return applied
    } catch (err) {
      errorMessage.value = errorText(err, '执行拦截规则失败')
      return null
    }
  }

  async function loadWindows(): Promise<void> {
    if (!supported.value || windowsLoading.value) return
    windowsLoading.value = true
    try {
      windows.value = await popupWindows()
      void ensureMeta(windows.value)
    } catch (err) {
      errorMessage.value = errorText(err, '读取窗口列表失败')
    } finally {
      windowsLoading.value = false
    }
  }

  /** 展示名：优先 exe 版本信息，取不到退回进程名（去掉 .exe） */
  function displayName(path: string, process: string): string {
    const name = path ? meta.value[path]?.name : ''
    if (name) return name
    return process.replace(/\.exe$/i, '') || process
  }

  function iconOf(path: string): string | null {
    return (path && meta.value[path]?.icon) || null
  }

  return reactive({
    status,
    hits,
    rules,
    ignoredProcesses,
    settings,
    windows,
    meta,
    windowsLoading,
    loading,
    busy,
    errorMessage,
    supported,
    running,
    blockedHits,
    pendingHits,
    enabledRules,
    lastBlockedAt,
    totalHits,
    appCount,
    lastHitAt,
    load,
    startPolling,
    stopPolling,
    toggle,
    clear,
    dismiss,
    ignore,
    addRule,
    removeRule,
    toggleRule,
    toggleApp,
    applyRule,
    loadWindows,
    saveSettings,
    refreshRecent,
    refreshRules,
    displayName,
    iconOf,
  })
}

/** 「弹窗拦截」面板与外部共享的接口类型 */
export type PopupApi = ReturnType<typeof usePopup>
