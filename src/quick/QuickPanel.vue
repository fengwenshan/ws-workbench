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

/** 面板里只显示「大概还剩多久」，精确到分钟，避免每秒刷一遍 */
const sleepRemaining = computed(() => {
  const status = keepAwake.status
  if (!status?.active) return ''
  if (status.remaining_secs == null) return '不限时，到点手动停止'
  const minutes = Math.max(1, Math.ceil(status.remaining_secs / 60))
  return `剩余约 ${minutes} 分钟`
})

async function toggleSleep() {
  if (keepAwake.active) {
    await keepAwake.stop()
  } else {
    await keepAwake.start(sleepPreset.value, false)
  }
}

function sleepHint(): string {
  const hint = keepAwake.status?.platform_hint
  return hint || '开启后系统不会因空闲睡眠，屏幕保持常亮。'
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
const vpnPaneEl = ref<HTMLElement | null>(null)
const sleepPaneEl = ref<HTMLElement | null>(null)
const activePaneEl = computed(() => (tab.value === 'vpn' ? vpnPaneEl.value : sleepPaneEl.value))

let paneObserver: ResizeObserver | null = null
let heightScheduled = false
let lastHeight = 0

/** 头部 + 内边距 + 当前页签内容 + 上下边框，和 `.qp` 的盒模型一一对应 */
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
  return Math.ceil(head.offsetHeight + pane.offsetHeight + padding + border)
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
  // 面板隐藏期间状态可能变化（比如托盘点连接后提示条变长），
  // 重新显示拿到焦点时再量一次，避免按旧高度弹出。
  window.addEventListener('focus', schedulePanelHeightSync)
  document.addEventListener('visibilitychange', handleVisibilityChange)
})

function handleVisibilityChange(): void {
  if (document.visibilityState === 'visible') schedulePanelHeightSync()
}

watch(tab, () => {
  observeActivePane()
  schedulePanelHeightSync()
})

onUnmounted(() => {
  paneObserver?.disconnect()
  window.removeEventListener('focus', schedulePanelHeightSync)
  document.removeEventListener('visibilitychange', handleVisibilityChange)
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
            <p>{{ keepAwake.active ? sleepRemaining : '屏幕保持常亮，不做登录级保活' }}</p>
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
            :class="{ active: sleepPreset === preset.seconds }"
            role="radio"
            :aria-checked="sleepPreset === preset.seconds"
            :disabled="keepAwake.active"
            @click="sleepPreset = preset.seconds"
          >
            {{ preset.label }}
          </button>
        </div>

        <p class="qp-tip">{{ sleepHint() }}</p>
      </section>
    </div>
  </div>
</template>

<style>
/* 窗口本身是透明的，圆角与描边都画在这一层上。
   这里不能加外投影：窗口正好和卡片一样大，投影会被窗口矩形裁掉，只在
   四个圆角外侧留下一圈灰色直角，看起来像面板的四个角没擦干净。 */
html,
body,
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

.qp-muted {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 400;
}
</style>
