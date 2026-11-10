import { ref, computed } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { isDevBuild } from '@/api/app'
import { safeGet, safeSet } from './safeStorage'

export type UpdateStatus =
  | 'idle'
  | 'checking'
  | 'up-to-date'
  | 'available'
  | 'downloading'
  | 'preparing'
  | 'installed'
  | 'error'

export interface UpdateInfo {
  version: string
  currentVersion: string
  notes: string
  date: string
}

/** 后端断点续传下载命令的进度消息（字段名与 Rust 侧约定一致，勿改） */
export interface UpdateProgress {
  /** 已写入的字节数：后端正常发「含续传起点的累计值」，只发本次会话字节时前端会补上起点 */
  downloaded: number
  /** 安装包总大小；未知时为 null */
  total: number | null
  /** 本次下载从哪个字节续传（0 表示从头开始） */
  resumedFrom: number
}

/** 后端断点续传下载命令的返回值（字段名与 Rust 侧约定一致，勿改） */
export interface UpdateDownloadResult {
  version: string
  path: string
  total: number
  resumedFrom: number
}

type PendingUpdate = NonNullable<Awaited<ReturnType<typeof check>>>

/** 每日自动校验时间点（时, 分）：上午 10:00、下午 16:00 */
export const DAILY_CHECK_TIMES: ReadonlyArray<readonly [number, number]> = [
  [10, 0],
  [16, 0],
]

/** 已忽略的版本，避免重启后重复打扰 */
const DISMISS_KEY = 'ws-workbench:dismissed-update-version'

/** 设置页打开时，距离上次检查超过这个时间就静默补一次，让「最新版本」有值 */
export const AUTO_CHECK_INTERVAL_MS = 5 * 60 * 1000

/**
 * 单个更新端点的请求超时（毫秒）。
 * 端点是「Gitee 清单优先、GitHub 清单兜底」两个，插件按顺序逐个试：
 * 不设超时的话第一个端点被网络卡住就永远轮不到第二个，「检查更新」会一直挂着。
 * 清单只有几百字节，10 秒够了；下载的超时由 Rust 侧自己控制，不受这个影响。
 */
const CHECK_TIMEOUT_MS = 10_000

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

// ========== 模块级单例状态 ==========
// App.vue（标题栏按钮）与 SettingsPanel（设置页「检查更新」卡片）必须共享同一份状态，
// 否则两边各查各的、进度对不上；所以 refs 与流程都放在模块作用域，useUpdater() 只返回同一份引用。

const status = ref<UpdateStatus>('idle')
const info = ref<UpdateInfo | null>(null)
const errorMessage = ref('')
const downloadedBytes = ref(0)
const totalBytes = ref(0)
const percentage = ref(0)
/** 本次下载从哪个字节续传（进度提示「已从 xx% 继续」用） */
const resumedFrom = ref(0)
/** 最近一次检查完成的时间戳（毫秒）；0 表示本次运行还没检查过 */
const lastCheckedAt = ref(0)
/** 最近一次检查的结论：true = 已是最新。silent 检查也记录，供设置页展示 */
const upToDate = ref(false)
/** 开发构建：允许检查版本，但安装更新会被 Rust 侧拒绝，UI 不提供安装入口 */
const devBuild = ref(false)
const dismissedVersion = ref<string>('')

/** JS 侧 check() 返回的 Update 对象；下载失败时保留，供「重试」断点续传 */
let pending: PendingUpdate | null = null
let scheduleTimer: ReturnType<typeof setTimeout> | null = null

// 忽略版本与「是否开发构建」只在模块初始化时读一次
void (async () => {
  dismissedVersion.value = (await safeGet(DISMISS_KEY)) || ''
})()
void isDevBuild().then((value) => {
  devBuild.value = value
})

/** 释放旧的 Update 资源（rid 在 Rust 侧对应一份下载上下文），失败不影响流程 */
function closePending() {
  if (!pending) return
  const old = pending
  pending = null
  void old.close().catch(() => {
    // 资源已失效时忽略
  })
}

/** 是否有需要提示给用户的新版本（已忽略的版本不再提示） */
const hasUpdatePrompt = computed(
  () => status.value === 'available' && info.value?.version !== dismissedVersion.value
)

/** 忽略当前提示的版本（记入本地，重启后不再提示该版本） */
function dismissCurrent() {
  if (!info.value) return
  dismissedVersion.value = info.value.version
  void safeSet(DISMISS_KEY, info.value.version)
}

/**
 * 检查更新。
 * silent 用于启动时、每日定时检查与设置页自动补查：没有新版本或检查失败都回到 idle，
 * 不把「已是最新 / 重试」这类瞬态文案留在标题栏上；设置页仍可通过 upToDate 判断结果。
 */
async function checkUpdate(options: { silent?: boolean } = {}): Promise<UpdateInfo | null> {
  const silent = options.silent === true
  status.value = 'checking'
  errorMessage.value = ''
  upToDate.value = false
  downloadedBytes.value = 0
  totalBytes.value = 0
  percentage.value = 0
  resumedFrom.value = 0
  try {
    const update = await check({ timeout: CHECK_TIMEOUT_MS })
    if (!update) {
      closePending()
      info.value = null
      upToDate.value = true
      status.value = silent ? 'idle' : 'up-to-date'
      return null
    }
    closePending()
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
    // 失败时保留 pending 与 info：网络恢复后点「重试」还能接着断点续传，
    // 设置页也能把「检查失败」和已查到的版本一起显示出来。
    status.value = silent ? 'idle' : 'error'
    errorMessage.value = String(err)
    return null
  } finally {
    lastCheckedAt.value = Date.now()
  }
}

/**
 * 下载并安装更新：走 Rust 的断点续传命令，不再用 plugin 的 pending.download()。
 *
 * 顺序固定：下载完成 -> vpn_prepare_update -> update_install_resumed。
 * Windows 上安装命令会直接退出进程；下载失败时保留 pending 与 info，
 * 用户点「重试」再次走同一流程，后端从 .part 续传。
 */
async function installUpdate(): Promise<boolean> {
  if (devBuild.value) {
    status.value = 'error'
    errorMessage.value = '开发构建无法安装更新，请使用发布版。'
    return false
  }
  if (!pending) {
    await checkUpdate()
    if (!pending) return false
  } else if (status.value === 'error') {
    // 失败后点「重试」：先确认服务端有没有更高版本。
    // 有就换成新的（下面按新版本重新下载，不复用旧版本的 .part；
    // 旧缓存会在新包下载成功后由 Rust 侧清掉）；
    // 这次检查如果只是网络失败，pending 会保留，继续从原来的断点续传。
    await checkUpdate({ silent: true })
    if (!pending) return false
  }
  // 先把引用固定下来：失败重试期间 pending 不能被其他检查替换
  const target = pending
  status.value = 'downloading'
  errorMessage.value = ''
  downloadedBytes.value = 0
  totalBytes.value = 0
  percentage.value = 0
  resumedFrom.value = 0
  try {
    // 进度通过 Channel 由 Rust 推过来（参数名 onProgress 与后端契约一致）
    const onProgress = new Channel<UpdateProgress>()
    onProgress.onmessage = (event: UpdateProgress) => {
      if (!event || typeof event !== 'object') return
      if (typeof event.resumedFrom === 'number' && event.resumedFrom > 0) {
        resumedFrom.value = event.resumedFrom
      }
      if (typeof event.total === 'number' && event.total > 0) {
        totalBytes.value = event.total
      }
      if (typeof event.downloaded === 'number' && event.downloaded >= 0) {
        // downloaded 应是含续传起点的累计值；若后端只发本次会话的字节，它会小于起点，
        // 这里补上续传起点，保证进度条不会从断点处掉回 0。
        const resumed = resumedFrom.value
        downloadedBytes.value =
          event.downloaded >= resumed ? event.downloaded : resumed + event.downloaded
        if (totalBytes.value > 0) {
          percentage.value = Math.min(100, (downloadedBytes.value / totalBytes.value) * 100)
        }
      }
    }

    const result = await invoke<UpdateDownloadResult>('update_download_resumed', {
      rid: target.rid,
      version: target.version,
      onProgress,
    })

    // 下载结束：以命令返回的总大小为准补齐进度，避免最后一条进度消息丢包
    if (result && typeof result.total === 'number' && result.total > 0) {
      totalBytes.value = result.total
    }
    if (result && typeof result.resumedFrom === 'number' && result.resumedFrom > 0) {
      resumedFrom.value = result.resumedFrom
    }
    if (totalBytes.value > 0) {
      downloadedBytes.value = totalBytes.value
    }
    percentage.value = 100

    // 下载完再断 VPN：安装阶段才需要 worker 退出（Windows 上它和主程序是同一个
    // exe，会挡住安装包替换；macOS 上旧 worker 会带着旧隧道继续跑）。
    // 下载期间用户中途又连上 VPN 的窗口，用这个顺序压到最小。
    status.value = 'preparing'
    await invoke('vpn_prepare_update')
    await invoke('update_install_resumed', { rid: target.rid, version: target.version })
    status.value = 'installed'
    return true
  } catch (err: any) {
    status.value = 'error'
    errorMessage.value = String(err)
    return false
  }
}

async function restartApp() {
  // 安装后用户可能又连上了 VPN：重启前尽量先断开。断不掉也要继续重启——
  // 旧 worker 会在主进程消失后靠 pid / 心跳兜底自行收尾，不会一直挂在后台。
  try {
    await invoke('vpn_prepare_update')
  } catch {
    // 忽略：这里只是尽量把状态收干净，不阻断「重启生效」
  }
  await relaunch()
}

/** 启动每日定时校验：到点自动检查，检查完自动排下一次 */
function startDailyCheck() {
  stopDailyCheck()
  const wait = msUntilNextCheck()
  scheduleTimer = setTimeout(async () => {
    // 正在检查或下载时不打断当前流程
    if (status.value !== 'checking' && status.value !== 'downloading') {
      await checkUpdate({ silent: true })
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
  resumedFrom.value = 0
  upToDate.value = false
  lastCheckedAt.value = 0
}

// 每次 useUpdater() 都返回同一份引用：App.vue 的标题栏按钮与设置页共享状态
const updaterApi = {
  status,
  info,
  errorMessage,
  downloadedBytes,
  totalBytes,
  percentage,
  resumedFrom,
  lastCheckedAt,
  upToDate,
  devBuild,
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

export function useUpdater() {
  return updaterApi
}
