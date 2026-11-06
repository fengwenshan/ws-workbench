<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useConfig } from '@/composables/useConfig'
import { useVpn } from '@/composables/useVpn'
import { useCleaner } from '@/composables/useCleaner'
import AppIcon from '@/components/cleaner/AppIcon.vue'
import { formatMemory } from '@/components/cleaner/format'
import type { VpnProxyMode } from '@/types/vpn'

/**
 * 托盘快捷面板（独立窗口 /quick.html），布局参考腾讯柠檬的悬浮面板：
 * 顶部两个页签（内存 / VPN），下面是紧凑卡片，点面板外部或按 Esc 收起。
 *
 * 这里只做「快速查看 + 快速动作」：卸载、残留清理、规则编辑仍然回主窗口。
 */
const { config, saveConfig } = useConfig()
const vpn = useVpn(config, saveConfig)
const cleaner = useCleaner()

type Tab = 'memory' | 'vpn'
const tab = ref<Tab>('memory')

// ============================== 内存 ==============================

/** 占用率和主窗口一致：优先 used/total，拿不到再退回后端给的 used_percent */
const usedPercent = computed(() => {
  const snapshot = cleaner.memory
  if (!snapshot) return 0
  const raw =
    snapshot.total > 0 && snapshot.used > 0
      ? (snapshot.used / snapshot.total) * 100
      : Number(snapshot.used_percent ?? 0)
  if (!Number.isFinite(raw)) return 0
  return Math.min(100, Math.max(0, raw))
})

const level = computed(() => {
  if (usedPercent.value >= 80) return 'danger'
  if (usedPercent.value >= 60) return 'warn'
  return 'primary'
})

const topApps = computed(() => (cleaner.memory?.apps ?? []).slice(0, 7))
const releaseTip = ref('')
const releaseTipLevel = ref<'ok' | 'warn'>('ok')

async function handleRelease() {
  releaseTip.value = ''
  const outcome = await cleaner.release()
  if (!outcome) {
    releaseTip.value = cleaner.actionError || '深度释放失败'
    releaseTipLevel.value = 'warn'
    return
  }
  if (!outcome.ok) {
    // 用户点了「取消」也算正常路径，不当错误提示
    releaseTip.value = outcome.message || '已取消授权'
    releaseTipLevel.value = 'warn'
    return
  }
  releaseTip.value =
    outcome.freed > 0
      ? `已回收 ${formatMemory(outcome.freed)}（${formatMemory(outcome.before_used)} → ${formatMemory(outcome.after_used)}）`
      : '系统暂无可回收内存'
  releaseTipLevel.value = 'ok'
}

function startMemory(): void {
  cleaner.startMemoryPolling()
}

function stopMemory(): void {
  cleaner.stopMemoryPolling()
}

// 页签切到内存才开始 2 秒采样；切走就停，面板隐藏时由 document.hidden 兜底
watch(
  tab,
  (value) => {
    if (value === 'memory') startMemory()
    else stopMemory()
  },
  { immediate: true },
)

// ============================== VPN ==============================

const MODES: Array<{ value: VpnProxyMode; label: string }> = [
  { value: 'rule', label: '规则' },
  { value: 'global', label: '全局' },
  { value: 'direct', label: '直连' },
]

const MODE_HINT: Record<VpnProxyMode, string> = {
  rule: '按规则列表分流，未命中的流量直连。',
  global: '所有流量都走 VPN 隧道，规则不参与分流。',
  direct: '只保持隧道连接，所有流量直连。',
}

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

// ============================== 窗口动作 ==============================

function openMain(target?: 'cleaner' | 'proxy' | 'settings') {
  void invoke('quick_panel_open_main', { target: target ?? null })
}

function hidePanel() {
  void invoke('quick_panel_hide')
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') hidePanel()
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  stopMemory()
  window.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div class="qp">
    <header class="qp-head" data-tauri-drag-region>
      <div class="qp-tabs" role="tablist" aria-label="快捷面板">
        <button
          type="button"
          class="qp-tab"
          :class="{ active: tab === 'memory' }"
          role="tab"
          :aria-selected="tab === 'memory'"
          @click="tab = 'memory'"
        >
          内存
          <span class="qp-badge" :class="`level-${level}`">{{ Math.round(usedPercent) }}%</span>
        </button>
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
      </div>
      <button type="button" class="qp-close" title="收起（Esc）" @click="hidePanel">×</button>
    </header>

    <div class="qp-body">
      <!-- 内存：总览 + 占用排行 + 深度释放 -->
      <section v-show="tab === 'memory'" class="qp-pane">
        <div class="mem-hero">
          <div class="mem-ring" :class="`level-${level}`">
            <svg viewBox="0 0 64 64" aria-hidden="true">
              <circle class="ring-bg" cx="32" cy="32" r="27" />
              <circle
                class="ring-fg"
                cx="32"
                cy="32"
                r="27"
                :stroke-dasharray="`${(usedPercent * 1.696).toFixed(1)} 999`"
                transform="rotate(-90 32 32)"
              />
            </svg>
            <span class="mem-pct">{{ Math.round(usedPercent) }}%</span>
          </div>
          <div class="mem-text">
            <strong>内存占用</strong>
            <p>
              已用 {{ formatMemory(cleaner.memory?.used ?? 0) }} /
              {{ formatMemory(cleaner.memory?.total ?? 0) }}
            </p>
            <p v-if="cleaner.memory?.swap_used" class="qp-muted">
              交换 {{ formatMemory(cleaner.memory?.swap_used ?? 0) }}
            </p>
          </div>
          <button
            type="button"
            class="qp-action"
            :disabled="cleaner.releasing"
            @click="handleRelease"
          >
            {{ cleaner.releasing ? '释放中…' : '释放' }}
          </button>
        </div>

        <p v-if="releaseTip" class="qp-tip" :class="releaseTipLevel">{{ releaseTip }}</p>

        <div class="qp-section">
          <span>占用排行</span>
          <span v-if="cleaner.memory" class="qp-muted">
            {{ cleaner.memory.apps.length }} 个应用
          </span>
        </div>

        <ul class="app-list">
          <li v-for="app in topApps" :key="app.id" class="app-row">
            <AppIcon :icon="app.icon" :name="app.name" :size="22" />
            <span class="app-name" :title="app.name">{{ app.name }}</span>
            <span class="app-mem">{{ formatMemory(app.memory) }}</span>
          </li>
          <li v-if="!topApps.length" class="app-empty">
            {{ cleaner.memoryError || '正在采样…' }}
          </li>
        </ul>

        <button type="button" class="qp-more" @click="openMain('cleaner')">
          查看全部应用与卸载 →
        </button>
      </section>

      <!-- VPN：状态 + 连接/断开 + 规则/全局/直连 -->
      <section v-show="tab === 'vpn'" class="qp-pane">
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

        <p class="qp-tip">{{ MODE_HINT[activeMode] }}</p>
        <p v-if="modeError" class="qp-tip warn">{{ modeError }}</p>

        <button type="button" class="qp-more" @click="openMain('proxy')">
          打开代理页管理配置与规则 →
        </button>
      </section>
    </div>

    <footer class="qp-foot">
      <button type="button" class="qp-foot-btn" @click="openMain()">打开主窗口</button>
      <span class="qp-muted">Esc 收起</span>
      <button type="button" class="qp-foot-btn" @click="hidePanel">隐藏</button>
    </footer>
  </div>
</template>

<style>
/* 窗口本身是透明的，圆角与投影都画在这一层上 */
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
  border: 1px solid rgba(15, 23, 42, 0.1);
  border-radius: 14px;
  background: var(--bg-primary);
  box-shadow:
    0 18px 42px rgba(15, 23, 42, 0.22),
    0 2px 8px rgba(15, 23, 42, 0.08);
  color: var(--text-primary);
  font-size: 13px;
}

/* ============================== 头部页签 ============================== */

.qp-head {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 46px;
  padding: 0 8px 0 14px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-secondary);
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

.qp-badge {
  padding: 0 6px;
  border-radius: 999px;
  background: var(--bg-hover);
  color: var(--text-secondary);
  font-size: 11px;
  font-weight: 500;
  line-height: 17px;
}

.qp-badge.level-warn {
  background: #fef3c7;
  color: #b45309;
}

.qp-badge.level-danger {
  background: #fee2e2;
  color: #b91c1c;
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

.qp-close {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--text-muted);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
}

.qp-close:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
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

.mem-hero {
  display: flex;
  align-items: center;
  gap: 14px;
}

.mem-ring {
  position: relative;
  width: 64px;
  height: 64px;
  flex-shrink: 0;
}

.mem-ring svg {
  display: block;
  width: 64px;
  height: 64px;
}

.mem-ring .ring-bg {
  fill: none;
  stroke: var(--bg-hover);
  stroke-width: 6;
}

.mem-ring .ring-fg {
  fill: none;
  stroke: var(--primary);
  stroke-width: 6;
  stroke-linecap: round;
  transition: stroke-dasharray 0.4s ease;
}

.mem-ring.level-warn .ring-fg {
  stroke: var(--warning);
}

.mem-ring.level-danger .ring-fg {
  stroke: var(--danger);
}

.mem-pct {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  font-size: 15px;
  font-weight: 700;
}

.mem-text {
  flex: 1;
  min-width: 0;
}

.mem-text strong {
  display: block;
  font-size: 14px;
}

.mem-text p {
  margin: 3px 0 0;
  color: var(--text-secondary);
  font-size: 12px;
}

.qp-action {
  flex-shrink: 0;
  height: 30px;
  padding: 0 14px;
  border: 0;
  border-radius: 8px;
  background: var(--primary);
  color: #fff;
  font-size: 13px;
  cursor: pointer;
}

.qp-action:hover:not(:disabled) {
  background: var(--primary-hover);
}

.qp-action:disabled {
  opacity: 0.6;
  cursor: default;
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

/* ============================== 应用列表 ============================== */

.app-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.app-row {
  display: flex;
  align-items: center;
  gap: 9px;
  height: 32px;
  padding: 0 6px;
  border-radius: 7px;
}

.app-row:hover {
  background: var(--bg-hover);
}

.app-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.app-mem {
  flex-shrink: 0;
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
  font-size: 12px;
}

.app-empty {
  padding: 10px 6px;
  color: var(--text-muted);
  font-size: 12px;
}

.qp-more {
  margin-top: 4px;
  padding: 7px 0;
  border: 0;
  border-top: 1px dashed var(--border-color);
  background: transparent;
  color: var(--text-active);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}

.qp-more:hover {
  color: var(--primary-hover);
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

/* ============================== 底部 ============================== */

.qp-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  height: 44px;
  padding: 0 12px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-secondary);
  flex-shrink: 0;
}

.qp-foot-btn {
  border: 0;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}

.qp-foot-btn:hover {
  color: var(--text-primary);
}

.qp-muted {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 400;
}
</style>
