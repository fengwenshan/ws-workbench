import { ref } from 'vue'

/**
 * 局域网分享「消息提示音 + 免打扰」单例（设计 §4）。
 *
 * 左侧右键菜单和设置页开关共享同一份状态：`dnd` 存在 localStorage，
 * 开关都调用 `setDnd/toggleDnd`，两边通过这个模块级的 ref 自动同步。
 * 提示音用 WebAudio 现场合成（880Hz → 660Hz 两声短音），不引音频资源文件；
 * 浏览器要求音频上下文在用户手势后才能出声，所以首次 pointerdown / keydown 时 resume。
 *
 * 只在「收到对方的新消息」时响（useLanShareChat 的 chat 事件里判断
 * kind='message' && direction='in'）；自己发的、状态更新、已读、删除都不响。
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

let audioContext: AudioContext | null = null
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

/** 懒创建 AudioContext（Safari / WKWebView 需要 webkit 前缀） */
function ensureContext(): AudioContext | null {
  if (typeof window === 'undefined') return null
  const Ctor =
    window.AudioContext ??
    (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
  if (!Ctor) return null
  if (!audioContext) audioContext = new Ctor()
  return audioContext
}

function unlockAudio() {
  if (!audioContext) return
  void audioContext.resume().catch(() => {
    /* 还没有用户手势时 resume 会失败，等下一次手势再试 */
  })
}

/** 用户第一次点击 / 按键时解锁音频上下文（只绑一次） */
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

/** 单声短音：指数包络避免爆音 */
function playTone(ctx: AudioContext, frequency: number, startAt: number, duration: number) {
  const osc = ctx.createOscillator()
  const gain = ctx.createGain()
  osc.type = 'sine'
  osc.frequency.setValueAtTime(frequency, startAt)
  gain.gain.setValueAtTime(0.0001, startAt)
  gain.gain.exponentialRampToValueAtTime(0.16, startAt + 0.01)
  gain.gain.exponentialRampToValueAtTime(0.0001, startAt + duration)
  osc.connect(gain)
  gain.connect(ctx.destination)
  osc.start(startAt)
  osc.stop(startAt + duration + 0.02)
}

/** 收到对方消息时播放「叮咚」双音；免打扰或环境不支持时静默跳过 */
function playMessageSound() {
  if (dnd.value) return
  try {
    const ctx = ensureContext()
    if (!ctx) return
    bindUnlockOnce()
    unlockAudio()
    // 上下文还没解锁时 currentTime 不动，排两声短音即可；解锁后会正常出声
    const startAt = ctx.currentTime + 0.01
    playTone(ctx, 880, startAt, 0.08)
    playTone(ctx, 660, startAt + 0.1, 0.09)
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
