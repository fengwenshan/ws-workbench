<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useConfig } from '@/composables/useConfig'
import { useVpn } from '@/composables/useVpn'
import { useKeepAwake } from '@/composables/useKeepAwake'
import { isDevBuild } from '@/api/app'
import type { VpnProxyMode } from '@/types/vpn'

/**
 * 托盘快捷面板（独立窗口 /quick.html），布局参考腾讯柠檬的悬浮面板：
 * 顶部两个页签（VPN / 休眠），下面是紧凑卡片，点面板外部或再点一次托盘图标收起。
 *
 * 这里只做「快速查看 + 快速动作」：卸载、残留清理、规则编辑仍然回主窗口。
 */
const { config, saveConfig } = useConfig()
const vpn = useVpn(config, saveConfig)
const keepAwake = useKeepAwake()

type Tab = 'vpn' | 'sleep'
const tab = ref<Tab>('vpn')

/** 开发构建时在页签右侧挂一个 dev 标记，和安装版的面板区分开 */
const devBuild = ref(false)
void isDevBuild().then((dev) => {
  devBuild.value = dev
})

// ============================== 休眠 ==============================

/** 只在面板里用到的时长快捷项；主窗口页签还有 2 小时等更多选项 */
const SLEEP_PRESETS: Array<{ label: string; seconds: number | null }> = [
  { label: '15 分钟', seconds: 15 * 60 },
  { label: '30 分钟', seconds: 30 * 60 },
  { label: '1 小时', seconds: 60 * 60 },
  { label: '不限', seconds: null },
]

const sleepPreset = ref<number | null>(15 * 60)
const sleepBusy = computed(() => keepAwake.starting || keepAwake.stopping)

/** 最近一次拿到后端状态的本地时刻：把剩余时长加回来还原会话总时长 */
const statusAtMs = ref(Date.now())
watch(
  () => keepAwake.status,
  () => {
    statusAtMs.value = Date.now()
  },
)

/** 保持中时反推会话开始时长（主窗口里选的是 1 小时，这里也要高亮 1 小时） */
const activeSeconds = computed<number | null>(() => {
  const status = keepAwake.status
  if (!status?.active) return null
  if (status.remaining_secs == null || status.started_at_ms == null) return null
  return status.remaining_secs + (statusAtMs.value - status.started_at_ms) / 1000
})

const unlimitedActive = computed(
  () => keepAwake.active && keepAwake.status?.remaining_secs == null,
)

/** 未开启时高亮面板里选中的时长；保持中时高亮真正在跑的时长 */
function presetSelected(seconds: number | null): boolean {
  if (!keepAwake.active) return sleepPreset.value === seconds
  if (seconds === null) return unlimitedActive.value
  return activeSeconds.value != null && Math.abs(activeSeconds.value - seconds) <= 5
}

/** 秒数转「x 小时 y 分 / x 分钟」 */
function formatSleepDuration(totalSecs: number): string {
  const secs = Math.max(0, Math.round(totalSecs))
  const hours = Math.floor(secs / 3600)
  const minutes = Math.round((secs % 3600) / 60)
  if (hours > 0) return minutes > 0 ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`
  return `${Math.max(1, minutes)} 分钟`
}

/** 面板里只显示「大概还剩多久」，精确到分钟，避免每秒刷一遍 */
const sleepRemaining = computed(() => {
  const status = keepAwake.status
  if (!status?.active) return ''
  if (status.remaining_secs == null) return '不限时，到点手动停止'
  const minutes = Math.max(1, Math.ceil(status.remaining_secs / 60))
  const total = activeSeconds.value
  return total == null
    ? `剩余约 ${minutes} 分钟`
    : `剩余约 ${minutes} 分钟 · 共 ${formatSleepDuration(total)}`
})

async function toggleSleep() {
  if (keepAwake.active) {
    // 停止后把面板选中的时长设为刚才那场，下次开启沿用
    const current = SLEEP_PRESETS.find((preset) => presetSelected(preset.seconds))
    const ok = await keepAwake.stop()
    if (ok && current) sleepPreset.value = current.seconds
    return
  }
  await keepAwake.start(sleepPreset.value, false)
}

/**
 * 点时长快捷项：未开启时只是选中（点「开启」才生效）；
 * 保持中点其它时长等于切换会话时长，和主窗口的交互一致。
 */
async function chooseSleepPreset(seconds: number | null): Promise<void> {
  if (sleepBusy.value) return
  if (!keepAwake.active) {
    sleepPreset.value = seconds
    return
  }
  // 点当前正在跑的时长不重置计时
  if (presetSelected(seconds)) return
  await keepAwake.start(seconds, keepAwake.status?.allow_display_sleep ?? false)
}

function sleepHint(): string {
  const hint = keepAwake.status?.platform_hint
  return hint || '开启后系统不会因空闲睡眠，显示器保持常亮。'
}

// ============================== VPN ==============================

const MODES: Array<{ value: VpnProxyMode; label: string }> = [
  { value: 'rule', label: '规则' },
  { value: 'global', label: '全局' },
  { value: 'direct', label: '直连' },
]

const PHASE_TEXT: Record<string, string> = {
  disconnected: '未连接',
  preparing: '准备中…',
  elevating: '等待系统授权…',
  connecting: '连接中…',
  configuring_routes: '配置路由中…',
  connected: '已连接',
  disconnecting: '断开中…',
  failed: '连接失败',
}

const activeMode = computed<VpnProxyMode>(() => (vpn.proxyMode as VpnProxyMode) ?? 'rule')
const modeLocked = computed(() => vpn.phaseBusy || vpn.connecting)
const statusText = computed(
  () => PHASE_TEXT[vpn.status.phase as string] ?? (vpn.isConnected ? '已连接' : '未连接'),
)
const profileName = computed(
  () => vpn.selectedProfile?.name || vpn.status.profile_name || '未选择配置',
)
const vpnBusy = computed(() => vpn.phaseBusy || vpn.connecting)
const modeError = ref('')

async function toggleVpn() {
  modeError.value = ''
  await vpn.toggleConnection()
}

async function pickMode(mode: VpnProxyMode) {
  if (modeLocked.value || mode === activeMode.value) return
  modeError.value = ''
  const ok = await vpn.setProxyMode(mode)
  if (!ok) modeError.value = '模式切换失败，请稍后重试'
}

// ============================== 面板高度 ==============================

/** 窗口高度跟着当前页签的内容走，两个页签都不留一大片空白 */
const panelEl = ref<HTMLElement | null>(null)
const footEl = ref<HTMLElement | null>(null)
const vpnPaneEl = ref<HTMLElement | null>(null)
const sleepPaneEl = ref<HTMLElement | null>(null)
const activePaneEl = computed(() => (tab.value === 'vpn' ? vpnPaneEl.value : sleepPaneEl.value))

let paneObserver: ResizeObserver | null = null
let heightScheduled = false
let lastHeight = 0

/** 头部 + 内边距 + 当前页签内容 + 底部操作栏 + 上下边框，和 `.qp` 的盒模型一一对应 */
function measurePanelHeight(): number {
  const panel = panelEl.value
  const head = panel?.querySelector<HTMLElement>('.qp-head')
  const body = panel?.querySelector<HTMLElement>('.qp-body')
  const pane = activePaneEl.value
  if (!panel || !head || !body || !pane) return 0
  const panelStyle = getComputedStyle(panel)
  const bodyStyle = getComputedStyle(body)
  const border =
    parseFloat(panelStyle.borderTopWidth) + parseFloat(panelStyle.borderBottomWidth)
  const padding = parseFloat(bodyStyle.paddingTop) + parseFloat(bodyStyle.paddingBottom)
  const foot = footEl.value?.offsetHeight ?? 0
  return Math.ceil(head.offsetHeight + pane.offsetHeight + padding + foot + border)
}

// ============================== 底部操作栏 ==============================

/** 收起面板并唤出主窗口（和左键点托盘图标同一个入口） */
async function openMainWindow(): Promise<void> {
  try {
    await invoke('quick_panel_show_main')
  } catch {
    // 窗口已经收起 / 命令失败时不做兜底提示：面板马上就不在了，弹错反而打扰
  }
}

/** 退出整个应用（先停 VPN worker，再退出主进程） */
async function quitApp(): Promise<void> {
  try {
    await invoke('quick_panel_quit')
  } catch {
    // 同上：应用正在退出，不弹任何错误
  }
}

function syncPanelHeight(): void {
  const height = measurePanelHeight()
  // 同一高度不重复发 IPC：切页签、提示条出现/消失都会走到这里
  if (!height || Math.abs(height - lastHeight) < 1) return
  lastHeight = height
  void invoke('quick_panel_resize', { height })
}

function schedulePanelHeightSync(): void {
  if (heightScheduled) return
  heightScheduled = true
  void nextTick(() => {
    heightScheduled = false
    syncPanelHeight()
  })
}

/** 只观察当前页签：隐藏页签的 offsetHeight 是 0，观察它没有意义 */
function observeActivePane(): void {
  paneObserver?.disconnect()
  const pane = activePaneEl.value
  if (pane) paneObserver?.observe(pane)
}

onMounted(() => {
  if (typeof ResizeObserver !== 'undefined') {
    paneObserver = new ResizeObserver(() => schedulePanelHeightSync())
  }
  observeActivePane()
  schedulePanelHeightSync()
  // 主窗口里也能开启/切换/停止保持唤醒，托盘面板必须自己轮询才能跟上；
  // 页面隐藏时 useKeepAwake 会暂停节拍，重新显示后补拉。
  keepAwake.startPolling()
  // 面板隐藏期间状态可能变化（比如托盘点连接后提示条变长），
  // 重新显示拿到焦点时再量一次，避免按旧高度弹出。
  window.addEventListener('focus', handlePanelFocus)
  document.addEventListener('visibilitychange', handleVisibilityChange)
  document.addEventListener('keydown', handleKeydown)
})

/** 面板重新弹出：立刻补一次状态，避免先闪一下旧状态再等 5 秒轮询 */
function handlePanelFocus(): void {
  schedulePanelHeightSync()
  keepAwake.wake()
}

/** Esc 收起面板：除了点外面/再点托盘，给键盘留一条确定的关闭路径 */
function handleKeydown(event: KeyboardEvent): void {
  if (event.key !== 'Escape') return
  event.preventDefault()
  void invoke('quick_panel_hide').catch(() => {
    // 已经开始退出等异常情况：收不起来也不影响别的操作
  })
}

function handleVisibilityChange(): void {
  if (document.visibilityState !== 'visible') return
  schedulePanelHeightSync()
  // visibilitychange 恢复时 useKeepAwake 会自己排下一次轮询，这里再唤醒一次
  keepAwake.wake()
}

watch(tab, () => {
  observeActivePane()
  schedulePanelHeightSync()
  // 切到休眠页签时可能刚在别处开关过会话，先拉一次再展示
  if (tab.value === 'sleep') keepAwake.wake()
})

onUnmounted(() => {
  paneObserver?.disconnect()
  window.removeEventListener('focus', handlePanelFocus)
  document.removeEventListener('visibilitychange', handleVisibilityChange)
  document.removeEventListener('keydown', handleKeydown)
  keepAwake.stopPolling()
})
</script>

<template>
  <div ref="panelEl" class="qp">
    <header class="qp-head" data-tauri-drag-region>
      <div class="qp-tabs" role="tablist" aria-label="快捷面板">
        <button
          type="button"
          class="qp-tab"
          :class="{ active: tab === 'vpn' }"
          role="tab"
          :aria-selected="tab === 'vpn'"
          @click="tab = 'vpn'"
        >
          VPN
          <span class="qp-dot" :class="vpn.isConnected ? 'on' : 'off'" aria-hidden="true" />
        </button>
        <button
          type="button"
          class="qp-tab"
          :class="{ active: tab === 'sleep' }"
          role="tab"
          :aria-selected="tab === 'sleep'"
          @click="tab = 'sleep'"
        >
          休眠
          <span class="qp-dot" :class="keepAwake.active ? 'on' : 'off'" aria-hidden="true" />
        </button>
      </div>
      <span v-if="devBuild" class="qp-dev-tag" title="开发构建">dev</span>
    </header>

    <div class="qp-body">
      <!-- VPN：状态 + 连接/断开 + 规则/全局/直连 -->
      <section v-show="tab === 'vpn'" ref="vpnPaneEl" class="qp-pane">
        <div class="vpn-card">
          <span class="vpn-status-dot" :class="{ on: vpn.isConnected }" aria-hidden="true" />
          <div class="vpn-meta">
            <strong>{{ statusText }}</strong>
            <p :title="profileName">{{ profileName }}</p>
          </div>
          <button
            type="button"
            class="qp-conn"
            :class="{ connected: vpn.isConnected }"
            :disabled="vpnBusy"
            @click="toggleVpn"
          >
            {{ vpn.isConnected ? '断开' : '连接' }}
          </button>
        </div>

        <p v-if="vpn.status.error" class="qp-tip warn">{{ vpn.status.error }}</p>

        <div class="qp-section">
          <span>代理模式</span>
          <span class="qp-muted">{{ vpn.isConnected ? '切换立即生效' : '下次连接生效' }}</span>
        </div>

        <div class="mode-group" role="radiogroup" aria-label="代理模式">
          <button
            v-for="mode in MODES"
            :key="mode.value"
            type="button"
            class="mode-btn"
            :class="{ active: mode.value === activeMode }"
            role="radio"
            :aria-checked="mode.value === activeMode"
            :disabled="modeLocked"
            @click="pickMode(mode.value)"
          >
            {{ mode.label }}
          </button>
        </div>

        <p v-if="modeError" class="qp-tip warn">{{ modeError }}</p>
      </section>

      <!-- 休眠：一键保持唤醒 + 时长快捷项 -->
      <section v-show="tab === 'sleep'" ref="sleepPaneEl" class="qp-pane">
        <div class="vpn-card">
          <span class="vpn-status-dot" :class="{ on: keepAwake.active }" aria-hidden="true" />
          <div class="vpn-meta">
            <strong>{{ keepAwake.active ? '保持唤醒中' : '未开启' }}</strong>
            <p>{{ keepAwake.active ? sleepRemaining : '显示器保持常亮，不做登录级保活' }}</p>
          </div>
          <button
            type="button"
            class="qp-conn"
            :class="{ connected: keepAwake.active }"
            :disabled="sleepBusy"
            @click="toggleSleep"
          >
            {{ keepAwake.active ? '停止' : '开启' }}
          </button>
        </div>

        <p v-if="keepAwake.statusError || keepAwake.actionError" class="qp-tip warn">
          {{ keepAwake.statusError || keepAwake.actionError }}
        </p>

        <div class="qp-section">
          <span>保持时长</span>
          <span class="qp-muted">{{ keepAwake.active ? '停止后重新选' : '到点自动释放' }}</span>
        </div>

        <div class="mode-group" role="radiogroup" aria-label="保持唤醒时长">
          <button
            v-for="preset in SLEEP_PRESETS"
            :key="preset.label"
            type="button"
            class="mode-btn"
            :class="{ active: presetSelected(preset.seconds) }"
            role="radio"
            :aria-checked="presetSelected(preset.seconds)"
            :disabled="sleepBusy"
            @click="chooseSleepPreset(preset.seconds)"
          >
            {{ preset.label }}
          </button>
        </div>

        <p class="qp-tip">{{ sleepHint() }}</p>
      </section>
    </div>

    <!-- 底部操作栏：右键托盘图标弹出面板后，主界面和退出都从这里走 -->
    <footer ref="footEl" class="qp-foot">
      <button type="button" class="qp-foot-btn" @click="openMainWindow">显示主界面</button>
      <button type="button" class="qp-foot-btn qp-foot-danger" @click="quitApp">退出</button>
    </footer>
  </div>
</template>

<style>
/* 窗口本身是透明的，圆角与描边都画在这一层上。
   这里不能加外投影：窗口正好和卡片一样大，投影会被窗口矩形裁掉，只在
   四个圆角外侧留下一圈灰色直角，看起来像面板的四个角没擦干净。

   选择器要比 `main.css` 的 `body { background: ... }` 更具体：全局样式后
   加载，权重相同会盖掉 transparent，圆角外侧就露出一块白底。 */
html,
html body,
#quick-app {
  margin: 0;
  padding: 0;
  width: 100%;
  height: 100%;
  background: transparent;
  overflow: hidden;
}

.qp {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: 14px;
  background: var(--bg-primary);
  color: var(--text-primary);
  font-size: 13px;
}

/* ============================== 头部页签 ============================== */

.qp-head {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 46px;
  padding: 0 14px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-primary);
  flex-shrink: 0;
}

.qp-tabs {
  display: flex;
  align-items: center;
  gap: 4px;
  flex: 1;
  min-width: 0;
}

.qp-tab {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 46px;
  padding: 0 8px;
  border: 0;
  background: transparent;
  color: var(--text-secondary);
  font-size: 13px;
  cursor: pointer;
}

.qp-tab.active {
  color: var(--text-primary);
  font-weight: 600;
}

.qp-tab.active::after {
  content: '';
  position: absolute;
  left: 8px;
  right: 8px;
  bottom: 0;
  height: 2px;
  border-radius: 2px;
  background: var(--primary);
}

/* 开发构建标记：小圆角标签，固定在页签右侧 */
.qp-dev-tag {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  height: 18px;
  padding: 0 7px;
  border-radius: 9px;
  background: var(--bg-tag);
  color: var(--text-tag);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.02em;
  user-select: none;
}

.qp-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-muted);
}

.qp-dot.on {
  background: var(--success);
}


/* ============================== 内容区 ============================== */

.qp-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 14px;
}

.qp-pane {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.qp-tip {
  margin: 0;
  padding: 7px 10px;
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.5;
}

.qp-tip.ok {
  background: #ecfdf5;
  color: #047857;
}

.qp-tip.warn {
  background: #fffbeb;
  color: #b45309;
}

.qp-section {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 4px;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 600;
}


/* ============================== VPN ============================== */

.vpn-card {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-secondary);
}

.vpn-status-dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: var(--text-muted);
  flex-shrink: 0;
}

.vpn-status-dot.on {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.16);
}

.vpn-meta {
  flex: 1;
  min-width: 0;
}

.vpn-meta strong {
  display: block;
  font-size: 14px;
}

.vpn-meta p {
  margin: 2px 0 0;
  overflow: hidden;
  color: var(--text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
}

.qp-conn {
  flex-shrink: 0;
  height: 30px;
  padding: 0 16px;
  border: 0;
  border-radius: 8px;
  background: var(--primary);
  color: #fff;
  font-size: 13px;
  cursor: pointer;
}

.qp-conn.connected {
  background: var(--bg-tag);
  color: var(--text-primary);
}

.qp-conn:hover:not(:disabled) {
  filter: brightness(0.96);
}

.qp-conn:disabled {
  opacity: 0.6;
  cursor: default;
}

.mode-group {
  display: flex;
  gap: 6px;
}

.mode-btn {
  flex: 1;
  height: 32px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}

.mode-btn.active {
  border-color: var(--primary);
  background: var(--bg-active);
  color: var(--text-active);
  font-weight: 600;
}

.mode-btn:disabled {
  opacity: 0.6;
  cursor: default;
}
/* 保持中时「当前时长」不可再点，但仍然要看清是它 */
.mode-btn.active:disabled {
  opacity: 1;
}

.qp-muted {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 400;
}


/* ============================== 底部操作栏 ============================== */

/* 托盘右键弹出的面板底部固定两条入口：显示主界面 / 退出。
   做成两条纯文本入口而不是通栏按钮：面板本身很窄，两个等宽大按钮会显得比
   上面的内容还重；文本入口更像菜单项，也和上方页签的轻量观感一致。
   左右各放一个（显示主界面靠左、退出靠右），比挤在中间更符合「底部工具条」。 */
.qp-foot {
  display: flex;
  flex-shrink: 0;
  justify-content: space-between;
  gap: 16px;
  padding: 6px 14px 10px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-secondary);
}

/* 纯文本按钮：不要边框和底色，只留文字；hover 只变色，不影响布局高度 */
.qp-foot-btn {
  padding: 2px 0;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 20px;
  cursor: pointer;
  transition: color 0.15s ease;
}

.qp-foot-btn:hover {
  color: var(--primary);
}

.qp-foot-btn:active {
  opacity: 0.7;
}

/* 退出是破坏性动作：只在悬浮时用红色提示，平时和其它文本入口一样 */
.qp-foot-danger:hover {
  color: var(--danger);
}

</style>
