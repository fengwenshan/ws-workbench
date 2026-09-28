<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { VpnApi } from '@/composables/useVpn'
import type { VpnProfile } from '@/types/vpn'
import { MODE_LABELS, PHASE_LABELS } from '@/types/vpn'
import LogPanel from '../LogPanel.vue'
import VpnInstallGuide from './VpnInstallGuide.vue'
import VpnLearnCard from './VpnLearnCard.vue'
import VpnProfileForm from './VpnProfileForm.vue'
import VpnRuleTable from './VpnRuleTable.vue'
import VpnRoutePreview from './VpnRoutePreview.vue'

const props = defineProps<{
  vpn: VpnApi
  /** 当前页签是否处于激活状态，用于切回来时刷新连接器 */
  active: boolean
}>()

const vpn = props.vpn

const confirmDelete = ref<VpnProfile | null>(null)
const importHint = ref('')
const overlaps = ref<string[]>([])

// ============================ 状态条 ============================

const phaseLabel = computed(() => PHASE_LABELS[vpn.status.phase] ?? vpn.status.phase)

const dotClass = computed(() => {
  switch (vpn.status.phase) {
    case 'connected':
      return 'dot-ok'
    case 'failed':
      return 'dot-error'
    case 'disconnected':
      return 'dot-idle'
    default:
      return 'dot-busy'
  }
})

const connectLabel = computed(() => {
  if (vpn.phaseBusy) return '处理中…'
  if (vpn.isConnected) return '断开 VPN'
  return '连接 VPN'
})

const statusSubtitle = computed(() => {
  const status = vpn.status
  const parts: string[] = []
  if (status.message) parts.push(status.message)
  if (status.profile_name) parts.push(status.profile_name)
  if (vpn.isConnected && status.vpn_ip) parts.push(`隧道 ${status.vpn_ip}`)
  return parts.join(' · ')
})

const canConnect = computed(() => !!vpn.selectedProfile && !vpn.connecting)
const connectorSummary = computed(() => {
  if (vpn.noConnector) return '没有可用连接器'
  return vpn.activeConnectors.map((c: any) => `${c.name}${c.version ? ' ' + c.version : ''}`).join(' / ')
})

// ============================ 规则覆盖检查 ============================

async function refreshOverlaps() {
  const rules = vpn.selectedRuleSet?.rules ?? []
  const enabled = rules.filter((rule: any) => rule.enabled).map((rule: any) => rule.cidr)
  overlaps.value = enabled.length > 1 ? await vpn.checkOverlaps(enabled) : []
}

watch(
  () => [vpn.selectedRuleSet?.id, (vpn.selectedRuleSet?.rules ?? []).map((r: any) => r.cidr + r.enabled).join(',')],
  refreshOverlaps,
  { immediate: true },
)

// ============================ 交互 ============================

async function handleConnect() {
  await vpn.connect()
  await refreshOverlaps()
}

async function handleImport() {
  try {
    await vpn.importOpenconnect()
    importHint.value = '导入完成，连接器列表已刷新'
  } catch (err: any) {
    importHint.value = `导入失败: ${err}`
  }
  setTimeout(() => (importHint.value = ''), 4000)
}

async function handleAddProfile() {
  await vpn.addProfile('新建 VPN 配置', '')
}

function askDelete(profile: VpnProfile) {
  confirmDelete.value = profile
}

async function confirmDeleteProfile() {
  if (!confirmDelete.value) return
  await vpn.deleteProfile(confirmDelete.value.id)
  confirmDelete.value = null
}

async function handleUpdate(patch: Partial<VpnProfile>) {
  const profile = vpn.selectedProfile
  if (!profile) return
  await vpn.updateProfile(profile.id, patch)
}

async function handleSavePassword(plain: string) {
  const profile = vpn.selectedProfile
  if (!profile) return
  try {
    await vpn.savePassword(profile.id, plain)
    vpn.pushLog('success', `已加密保存「${profile.name}」的密码`)
  } catch (err: any) {
    vpn.pushLog('error', `保存密码失败: ${err}`)
  }
}

/** 切回 VPN 页时刷新连接器与快照，保证看到的是当前机器状态 */
watch(
  () => props.active,
  async (isActive) => {
    if (!isActive) return
    await vpn.refreshSnapshot()
    await vpn.refreshElevation()
    await vpn.refreshConnectors()
  },
)
</script>

<template>
  <div class="vpn-layout">
    <!-- 状态条 -->
    <div class="status-bar">
      <div class="status-left">
        <span class="dot" :class="dotClass"></span>
        <div class="status-text">
          <div class="status-line">
            <strong>{{ phaseLabel }}</strong>
            <span v-if="vpn.status.connector_name" class="status-chip">
              {{ vpn.status.connector_name }}
            </span>
            <span class="status-chip status-chip-mode">{{ MODE_LABELS[vpn.status.mode] }}</span>
          </div>
          <span class="status-sub">{{ statusSubtitle || connectorSummary }}</span>
        </div>
      </div>

      <div class="status-actions">
        <button
          class="btn"
          :class="vpn.isConnected || vpn.phaseBusy ? 'btn-danger' : 'btn-primary'"
          :disabled="!canConnect && !vpn.isConnected"
          @click="handleConnect"
        >
          {{ connectLabel }}
        </button>
        <button class="btn btn-secondary" :disabled="vpn.detecting" @click="vpn.refreshConnectors()">
          {{ vpn.detecting ? '检测中…' : '重新检测连接器' }}
        </button>
        <button class="btn btn-secondary" @click="vpn.openSessionDir()">打开日志目录</button>
      </div>
    </div>

    <div class="vpn-body">
      <!-- 左侧：配置列表 -->
      <aside class="profile-list">
        <div class="list-head">
          <span>VPN 配置</span>
          <button class="list-add" @click="handleAddProfile">+ 新建</button>
        </div>
        <div
          v-for="profile in vpn.profiles"
          :key="profile.id"
          class="list-item"
          :class="{ 'list-item-active': profile.id === vpn.selectedProfileId }"
          @click="vpn.selectProfile(profile.id)"
        >
          <div class="item-main">
            <span class="item-name">{{ profile.name }}</span>
            <span class="item-sub mono">{{ profile.server || '未填写服务器' }}</span>
          </div>
          <span class="item-mode">{{ MODE_LABELS[profile.mode] }}</span>
        </div>
        <div v-if="vpn.profiles.length === 0" class="list-empty">
          还没有配置，点右上角新建一个
        </div>

        <!-- 提权状态 -->
        <div v-if="vpn.elevation" class="elevation-box">
          <div class="elevation-title">提权方式</div>
          <p class="elevation-hint">{{ vpn.elevation.hint }}</p>
          <div class="elevation-actions">
            <button
              v-if="vpn.elevation.platform === 'windows' && !vpn.elevation.task_ready && !vpn.elevation.elevated"
              class="btn btn-secondary btn-xs"
              @click="vpn.setupElevation()"
            >
              注册免 UAC 任务
            </button>
            <button
              v-if="vpn.elevation.platform === 'windows' && vpn.elevation.task_ready"
              class="btn btn-secondary btn-xs"
              @click="vpn.removeElevation()"
            >
              移除任务
            </button>
          </div>
        </div>
      </aside>

      <!-- 右侧：详情 -->
      <section class="detail">
        <VpnInstallGuide
          v-if="vpn.noConnector"
          :guide="vpn.guide"
          :can-import="true"
          @import="handleImport"
          @copy-command="(cmd: string) => vpn.pushLog('info', `已复制命令: ${cmd}`)"
        />
        <p v-if="importHint" class="import-hint">{{ importHint }}</p>

        <template v-if="vpn.selectedProfile">
          <div class="card">
            <div class="card-head">
              <h4>连接配置</h4>
              <span class="card-sub">修改会自动保存</span>
            </div>
            <VpnProfileForm
              :profile="vpn.selectedProfile"
              :busy="vpn.isConnected || vpn.phaseBusy"
              @update="handleUpdate"
              @save-password="handleSavePassword"
              @remove="askDelete(vpn.selectedProfile!)"
            />
          </div>

          <VpnRuleTable
            :rule-set="vpn.selectedRuleSet"
            :mode="vpn.selectedProfile.mode"
            :overlaps="overlaps"
            @add="(cidr: string, remark: string) => vpn.addRule(vpn.selectedProfile!.rule_set_id, cidr, remark)"
            @update="(ruleId: string, patch: any) => vpn.updateRule(vpn.selectedProfile!.rule_set_id, ruleId, patch)"
            @remove="(ruleId: string) => vpn.deleteRule(vpn.selectedProfile!.rule_set_id, ruleId)"
          />

          <VpnLearnCard :vpn="vpn" />

          <VpnRoutePreview :status="vpn.status" />

          <!-- 连接器明细：出问题时便于排查到底用了哪份二进制 -->
          <div class="card">
            <div class="card-head">
              <h4>连接器</h4>
              <button
                v-if="vpn.activeConnectors.some((c: any) => c.source === 'managed')"
                class="btn btn-secondary btn-xs"
                @click="vpn.removeManagedOpenconnect()"
              >
                移除已导入的 OpenConnect
              </button>
            </div>
            <div v-for="item in vpn.connectors" :key="item.kind + item.path" class="connector-row">
              <span class="connector-dot" :class="item.available ? 'dot-ok' : 'dot-idle'"></span>
              <div class="connector-main">
                <span class="connector-name">
                  {{ item.name }}
                  <em v-if="item.version">{{ item.version }}</em>
                </span>
                <span class="connector-path mono">{{ item.path || '—' }}</span>
                <span v-if="item.note" class="connector-note">{{ item.note }}</span>
              </div>
            </div>
          </div>
        </template>

        <div v-else class="detail-empty">
          先在左边新建一个 VPN 配置
        </div>
      </section>
    </div>

    <!-- 会话目录回退时按默认路径是找不到日志的，必须把实际位置说出来 -->
    <p v-if="vpn.sessionDir?.fallback" class="session-dir-hint">
      会话目录已回退到「{{ vpn.sessionDir.origin }}」：<span class="mono">{{ vpn.sessionDir.path }}</span>。
      首选的用户数据目录不可写（受控文件夹访问或企业策略），日志与连接状态文件都写在这里。
    </p>

    <!-- 日志 -->
    <div class="log-section">
      <LogPanel :logs="vpn.logs" @clear="vpn.clearLogs()" />
    </div>

    <!-- 删除确认 -->
    <div v-if="confirmDelete" class="dialog-overlay" @click.self="confirmDelete = null">
      <div class="dialog">
        <h3 class="dialog-title">删除配置</h3>
        <p class="dialog-message">
          确定要删除「{{ confirmDelete.name }}」吗？绑定的规则集如果没有别的配置在用，也会一起删掉。
        </p>
        <div class="dialog-actions">
          <button class="btn btn-secondary" @click="confirmDelete = null">取消</button>
          <button class="btn btn-danger" @click="confirmDeleteProfile">确定删除</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.vpn-layout {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

/* 状态条 */
.status-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 12px 20px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-primary);
  flex-shrink: 0;
}
.status-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex-shrink: 0;
}
.dot-ok {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.15);
}
.dot-error {
  background: var(--danger);
  box-shadow: 0 0 0 3px rgba(239, 68, 68, 0.15);
}
.dot-idle {
  background: var(--text-muted);
}
.dot-busy {
  background: var(--warning);
  animation: pulse 1.2s ease-in-out infinite;
}
@keyframes pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}
.status-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.status-line {
  display: flex;
  align-items: center;
  gap: 8px;
}
.status-line strong {
  font-size: 14px;
  color: var(--text-primary);
}
.status-chip {
  font-size: 11px;
  color: var(--text-secondary);
  background: var(--bg-tag);
  border-radius: 9px;
  padding: 1px 8px;
}
.status-chip-mode {
  color: var(--primary);
  background: rgba(59, 130, 246, 0.1);
}
.status-sub {
  font-size: 12px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 560px;
}
.status-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}

/* 主体 */
.vpn-body {
  flex: 1;
  display: flex;
  overflow: hidden;
}
.profile-list {
  width: 232px;
  border-right: 1px solid var(--border-color);
  background: var(--bg-primary);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  overflow-y: auto;
}
.list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  font-size: 12px;
  color: var(--text-muted);
  border-bottom: 1px solid var(--border-color);
}
.list-add {
  border: none;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  cursor: pointer;
}
.list-item {
  padding: 9px 12px;
  border-bottom: 1px solid var(--border-color);
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 8px;
}
.list-item:hover {
  background: var(--bg-hover);
}
.list-item-active {
  background: var(--bg-active);
}
.item-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.item-name {
  font-size: 13px;
  color: var(--text-primary);
}
.item-sub {
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.item-mode {
  font-size: 11px;
  color: var(--text-muted);
  flex-shrink: 0;
}
.list-empty {
  padding: 18px 12px;
  font-size: 12px;
  color: var(--text-muted);
  text-align: center;
}
.elevation-box {
  margin: 12px;
  padding: 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-secondary);
}
.elevation-title {
  font-size: 12px;
  color: var(--text-secondary);
  margin-bottom: 4px;
}
.elevation-hint {
  font-size: 11px;
  line-height: 1.6;
  color: var(--text-muted);
}
.elevation-actions {
  margin-top: 8px;
}

.detail {
  flex: 1;
  overflow-y: auto;
  padding: 16px 20px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.import-hint {
  font-size: 12px;
  color: var(--primary);
}
.card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  background: var(--bg-primary);
}
.card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
}
.card-head h4 {
  font-size: 14px;
  color: var(--text-primary);
}
.card-sub {
  font-size: 12px;
  color: var(--text-muted);
}
.connector-row {
  display: flex;
  gap: 8px;
  align-items: flex-start;
  padding: 7px 0;
  border-top: 1px solid var(--border-color);
}
.connector-row:first-of-type {
  border-top: none;
}
.connector-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  margin-top: 5px;
  flex-shrink: 0;
}
.connector-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.connector-name {
  font-size: 13px;
  color: var(--text-primary);
}
.connector-name em {
  font-style: normal;
  font-size: 11px;
  color: var(--text-muted);
  margin-left: 6px;
}
.connector-path {
  font-size: 11px;
  color: var(--text-muted);
  word-break: break-all;
}
.connector-note {
  font-size: 11px;
  color: var(--warning);
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.detail-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  color: var(--text-muted);
}
.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}

/* 会话目录回退提示：默认路径找不到日志，所以要显眼但别抢主体 */
.session-dir-hint {
  margin: 0;
  padding: 8px 12px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 10%, transparent);
  border-top: 1px solid var(--border-color);
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
  word-break: break-all;
}
.session-dir-hint .mono {
  color: inherit;
  font-weight: 600;
}

/* 日志 */
.log-section {
  height: 180px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-primary);
  flex-shrink: 0;
}

/* 弹窗 */
.dialog-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.3);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}
.dialog {
  background: var(--bg-primary);
  border-radius: 12px;
  padding: 24px;
  width: 380px;
  max-width: 90vw;
}
.dialog-title {
  font-size: 17px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 12px;
}
.dialog-message {
  font-size: 14px;
  color: var(--text-secondary);
  line-height: 1.6;
}
.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 24px;
}
</style>
