import { ref } from 'vue'
import chimeUrl from '@/assets/sounds/chime-done.mp3'

/**
 * 局域网分享「消息提示音 + 免打扰」单例（设计 §4 / LANLink 刷新 §3）。
 *
 * 提示音用 dsh-chime 插件的「任务完成」音效 `chime-done.mp3`（MIT，732B），
 * 由 Vite 处理成资源 URL，不引第三方依赖。左侧右键菜单和设置页开关共享同一份
 * `dnd`（localStorage），只在「收到对方的新消息」时播放；浏览器要求音频在用户
 * 手势后才能出声，所以首次 pointerdown / keydown 时静默 play 一次解锁。
 */

/** 免打扰持久化 key（设计 §4 约定） */
const STORAGE_KEY = 'ws-workbench:lanshare:dnd'

function loadDnd(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === '1'
  } catch {
    // 隐私模式 / 存储不可用时按「不静音」处理
    return false
  }
}

/** 免打扰（true = 不播放提示音） */
const dnd = ref(loadDnd())

let audio: HTMLAudioElement | null = null
let unlockBound = false

function persist(value: boolean) {
  try {
    window.localStorage.setItem(STORAGE_KEY, value ? '1' : '0')
  } catch {
    /* 存不下就只影响下次启动的默认值，不影响当前会话 */
  }
}

function setDnd(value: boolean) {
  dnd.value = value
  persist(value)
}

function toggleDnd() {
  setDnd(!dnd.value)
}

/** 懒创建 audio 元素（同一个实例复用，避免每条消息都 new） */
function ensureAudio(): HTMLAudioElement | null {
  if (typeof window === 'undefined') return null
  if (!audio) {
    audio = new Audio(chimeUrl)
    audio.preload = 'auto'
    audio.volume = 0.7
  }
  return audio
}

/** 首次用户手势时静音播一下再暂停：解锁 WebView 的自动播放限制，且不发声 */
function unlockAudio() {
  const player = ensureAudio()
  if (!player) return
  player.muted = true
  void player
    .play()
    .then(() => {
      player.pause()
      player.currentTime = 0
      player.muted = false
    })
    .catch(() => {
      /* 还没有用户手势时 play 会被拒绝，等下一次手势再试 */
      player.muted = false
    })
}

/** 用户第一次点击 / 按键时解锁音频（只绑一次） */
function bindUnlockOnce() {
  if (unlockBound || typeof window === 'undefined') return
  unlockBound = true
  const unlock = () => {
    unlockAudio()
    window.removeEventListener('pointerdown', unlock, true)
    window.removeEventListener('keydown', unlock, true)
  }
  window.addEventListener('pointerdown', unlock, true)
  window.addEventListener('keydown', unlock, true)
}

/** 收到对方消息时播放完成音；免打扰或环境不支持时静默跳过 */
function playMessageSound() {
  if (dnd.value) return
  try {
    const player = ensureAudio()
    if (!player) return
    bindUnlockOnce()
    player.currentTime = 0
    void player.play().catch(() => {
      /* 未解锁 / 播放被拒：等用户下一次手势后自然恢复 */
    })
  } catch (err) {
    console.warn('[lanshare] 播放提示音失败', err)
  }
}

/** 组件里用：拿到同一份 dnd ref 与操作函数 */
export function useLanShareNotify() {
  bindUnlockOnce()
  return { dnd, setDnd, toggleDnd, playMessageSound }
}

export { dnd, setDnd, toggleDnd, playMessageSound }
