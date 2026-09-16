import { ref, computed } from 'vue'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

export type UpdateStatus =
  | 'idle'
  | 'checking'
  | 'up-to-date'
  | 'available'
  | 'downloading'
  | 'installed'
  | 'error'

export interface UpdateInfo {
  version: string
  currentVersion: string
  notes: string
  date: string
}

type PendingUpdate = NonNullable<Awaited<ReturnType<typeof check>>>

/** 每日自动校验时间点（时, 分） */
export const DAILY_CHECK_TIMES: ReadonlyArray<readonly [number, number]> = [
  [10, 0],
  [15, 0],
]

/** 已忽略的版本，避免重启后重复打扰 */
const DISMISS_KEY = 'dist-cli:dismissed-update-version'

/**
 * 距离下一个校验时间点还有多少毫秒。
 * 今天的时间点已过（或正好到点）时顺延到明天。
 */
export function msUntilNextCheck(
  now: Date = new Date(),
  times: ReadonlyArray<readonly [number, number]> = DAILY_CHECK_TIMES
): number {
  let best = Number.POSITIVE_INFINITY
  for (const [hour, minute] of times) {
    const target = new Date(now)
    target.setHours(hour, minute, 0, 0)
    if (target.getTime() <= now.getTime()) {
      target.setDate(target.getDate() + 1)
    }
    best = Math.min(best, target.getTime() - now.getTime())
  }
  return best
}

export function useUpdater() {
  const status = ref<UpdateStatus>('idle')
  const info = ref<UpdateInfo | null>(null)
  const errorMessage = ref('')
  const downloadedBytes = ref(0)
  const totalBytes = ref(0)
  const percentage = ref(0)
  const dismissedVersion = ref<string>(
    typeof localStorage !== 'undefined' ? localStorage.getItem(DISMISS_KEY) || '' : ''
  )

  let pending: PendingUpdate | null = null
  let scheduleTimer: ReturnType<typeof setTimeout> | null = null

  /** 是否有需要提示给用户的新版本（已忽略的版本不再提示） */
  const hasUpdatePrompt = computed(
    () => status.value === 'available' && info.value?.version !== dismissedVersion.value
  )

  /** 忽略当前提示的版本（记入本地，重启后不再提示该版本） */
  function dismissCurrent() {
    if (!info.value) return
    dismissedVersion.value = info.value.version
    try {
      localStorage.setItem(DISMISS_KEY, info.value.version)
    } catch {
      // localStorage 不可用时忽略，仅内存生效
    }
  }

  async function checkUpdate(): Promise<UpdateInfo | null> {
    status.value = 'checking'
    errorMessage.value = ''
    info.value = null
    try {
      const update = await check()
      if (!update) {
        pending = null
        status.value = 'up-to-date'
        return null
      }
      pending = update
      info.value = {
        version: update.version,
        currentVersion: update.currentVersion,
        notes: update.body ?? '',
        date: update.date ?? '',
      }
      status.value = 'available'
      return info.value
    } catch (err: any) {
      pending = null
      status.value = 'error'
      errorMessage.value = String(err)
      return null
    }
  }

  async function installUpdate(): Promise<boolean> {
    if (!pending) {
      await checkUpdate()
      if (!pending) return false
    }
    status.value = 'downloading'
    downloadedBytes.value = 0
    totalBytes.value = 0
    percentage.value = 0
    try {
      await pending.downloadAndInstall((event) => {
        switch (event.event) {
          case 'Started':
            totalBytes.value = event.data.contentLength ?? 0
            break
          case 'Progress':
            downloadedBytes.value += event.data.chunkLength
            if (totalBytes.value > 0) {
              percentage.value = Math.min(100, (downloadedBytes.value / totalBytes.value) * 100)
            }
            break
          case 'Finished':
            percentage.value = 100
            break
        }
      })
      status.value = 'installed'
      return true
    } catch (err: any) {
      status.value = 'error'
      errorMessage.value = String(err)
      return false
    }
  }

  async function restartApp() {
    await relaunch()
  }

  /** 启动每日定时校验：到点自动检查，检查完自动排下一次 */
  function startDailyCheck() {
    stopDailyCheck()
    const wait = msUntilNextCheck()
    scheduleTimer = setTimeout(async () => {
      // 正在检查或下载时不打断当前流程
      if (status.value !== 'checking' && status.value !== 'downloading') {
        await checkUpdate()
      }
      startDailyCheck()
    }, wait)
  }

  function stopDailyCheck() {
    if (scheduleTimer) {
      clearTimeout(scheduleTimer)
      scheduleTimer = null
    }
  }

  function reset() {
    status.value = 'idle'
    info.value = null
    errorMessage.value = ''
    downloadedBytes.value = 0
    totalBytes.value = 0
    percentage.value = 0
  }

  return {
    status,
    info,
    errorMessage,
    downloadedBytes,
    totalBytes,
    percentage,
    dismissedVersion,
    hasUpdatePrompt,
    dismissCurrent,
    checkUpdate,
    installUpdate,
    restartApp,
    startDailyCheck,
    stopDailyCheck,
    reset,
  }
}
