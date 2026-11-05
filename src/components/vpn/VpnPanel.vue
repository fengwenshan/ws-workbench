<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Modal } from 'ant-design-vue'
import type { VpnApi } from '@/composables/useVpn'
import type {
  NewVpnProfileInput,
  VpnPolicy,
  VpnProfile,
  VpnProxyMode,
  VpnRule,
  VpnRuleKind,
  VpnRuleSet,
} from '@/types/vpn'
import { POLICY_LABELS, RULE_KIND_LABELS } from '@/types/vpn'
import { platform } from '@/platform'
import VpnInstallGuide from './VpnInstallGuide.vue'
import VpnLearnCard from './VpnLearnCard.vue'
import VpnProfileDialog from './VpnProfileDialog.vue'
import VpnProfileForm from './VpnProfileForm.vue'
import VpnRoutePreview from './VpnRoutePreview.vue'
import VpnRuleSetDialog from './VpnRuleSetDialog.vue'
import VpnStatusDot from './VpnStatusDot.vue'

const props = defineProps<{
  vpn: VpnApi
  /** 当前页签是否处于激活状态，用于切回来时刷新连接器 */
  active: boolean
  /** 点击左侧「代理」导航时收起/展开配置列表 */
  menuCollapsed?: boolean
}>()

const vpn = props.vpn

const confirmDelete = ref<VpnProfile | null>(null)
const confirmDeleteRuleSet = ref<VpnRuleSet | null>(null)
const editingRuleSetId = ref('')
const profileDialogOpen = ref(false)
const editingProfile = ref<VpnProfile | null>(null)
const savingProfile = ref(false)
const importHint = ref('')

const MODE_OPTIONS: Array<{
  value: VpnProxyMode
  label: string
  description: string
}> = [
  {
    value: 'rule',
    label: '规则',
    description: '基于预设规则智能判断流量走向，只有规则列表里命中的流量才按列表中的走向处理。',
  },
  {
    value: 'global',
    label: '全局',
    description: '所有流量都通过 VPN 隧道，适用于需要整个系统走 VPN 的场景；规则列表不参与分流。',
  },
  {
    value: 'direct',
    label: '直连',
    description: '所有流量都不进入 VPN 隧道，只保留 VPN 连接本身；适用于临时绕过 VPN 的场景。',
  },
]

// ============================ 连接操作 ============================

const connectLabel = computed(() => {
  if (vpn.phaseBusy) return '处理中…'
  if (vpn.isConnected) return '断开 VPN'
  return '连接 VPN'
})

// ============================ 状态点 ============================

type ProfileDotState = 'off' | 'on' | 'busy'

/** 正在连接/已连接的配置；连接阶段后端还没写 profile_id 时回退到当前选中项 */
const activeProfileId = computed(() => vpn.status.profile_id || vpn.selectedProfileId)

function profileDotState(profile: VpnProfile): ProfileDotState {
  if (activeProfileId.value !== profile.id) return 'off'
  if (vpn.isConnected) return 'on'
  if (vpn.phaseBusy || vpn.connecting) return 'busy'
  return 'off'
}

function dotLabel(state: ProfileDotState) {
  if (state === 'on') return '已连接'
  if (state === 'busy') return '连接中'
  return '未连接'
}

const selectedDotState = computed<ProfileDotState>(() =>
  vpn.selectedProfile ? profileDotState(vpn.selectedProfile) : 'off',
)

const canConnect = computed(() => {
  const profile = vpn.selectedProfile
  if (!profile || vpn.connecting) return false
  if ((profile.proxy_mode ?? 'rule') === 'rule' && !vpn.selectedRuleSet) return false
  return true
})

// ============================ 代理模式 ============================

const proxyMode = computed<VpnProxyMode>(() => vpn.selectedProfile?.proxy_mode ?? 'rule')
const currentMode = computed(
  () => MODE_OPTIONS.find((item) => item.value === proxyMode.value) ?? MODE_OPTIONS[0],
)
const modeLocked = computed(() => vpn.isConnected || vpn.phaseBusy || vpn.connecting)
const connectBlockedReason = computed(() =>
  proxyMode.value === 'rule' && !vpn.selectedRuleSet
    ? '规则模式需要先选择一个规则列表'
    : '',
)

async function handleModeChange(mode: VpnProxyMode) {
  if (mode === proxyMode.value || modeLocked.value) return
  try {
    await vpn.setProxyMode(mode)
    if (mode !== 'rule') editingRuleSetId.value = ''
  } catch (err: any) {
    vpn.pushLog('error', `切换代理模式失败: ${err}`)
  }
}

// ============================ 规则列表 ============================

const editingRuleSet = computed(
  () => vpn.ruleSets.find((set) => set.id === editingRuleSetId.value) ?? null,
)

function enabledRules(set: VpnRuleSet): VpnRule[] {
  return set.rules.filter((rule) => rule.enabled)
}

function previewRules(set: VpnRuleSet): VpnRule[] {
  return enabledRules(set).slice(0, 3)
}

function hiddenRuleCount(set: VpnRuleSet): number {
  return Math.max(0, enabledRules(set).length - 3)
}

async function handleNewRuleSet() {
  if (!vpn.selectedProfile) return
  try {
    const set = await vpn.createRuleSet()
    editingRuleSetId.value = set.id
  } catch (err: any) {
    vpn.pushLog('error', `新建规则列表失败: ${err}`)
  }
}

async function handleSelectRuleSet(set: VpnRuleSet) {
  try {
    await vpn.selectRuleSet(set.id)
  } catch (err: any) {
    vpn.pushLog('error', `切换规则列表失败: ${err}`)
  }
}

function openRuleSet(set: VpnRuleSet) {
  editingRuleSetId.value = set.id
}

function closeRuleDialog() {
  editingRuleSetId.value = ''
}

async function handleRenameRuleSet(name: string) {
  if (!editingRuleSet.value) return
  await vpn.updateRuleSet(editingRuleSet.value.id, { name })
}

async function handleAddRule(
  kind: VpnRuleKind,
  value: string,
  policy: VpnPolicy,
  remark: string,
) {
  if (!editingRuleSet.value) return
  await vpn.addRule(editingRuleSet.value.id, kind, value, policy, remark)
}

async function handleUpdateRule(ruleId: string, patch: Partial<VpnRule>) {
  if (!editingRuleSet.value) return
  await vpn.updateRule(editingRuleSet.value.id, ruleId, patch)
}

async function handleRemoveRule(ruleId: string) {
  if (!editingRuleSet.value) return
  await vpn.deleteRule(editingRuleSet.value.id, ruleId)
}

async function handleDefaultPolicy(policy: VpnPolicy) {
  if (!editingRuleSet.value) return
  await vpn.updateRuleSet(editingRuleSet.value.id, { default_policy: policy })
}

async function handleRuleSetDefaultPolicy(event: Event) {
  if (!vpn.selectedRuleSet) return
  const policy = (event.target as HTMLSelectElement).value as VpnPolicy
  await vpn.updateRuleSet(vpn.selectedRuleSet.id, { default_policy: policy })
}

function askDeleteRuleSet(set: VpnRuleSet) {
  confirmDeleteRuleSet.value = set
}

async function confirmDeleteSelectedRuleSet() {
  const set = confirmDeleteRuleSet.value
  if (!set) return
  if (editingRuleSetId.value === set.id) editingRuleSetId.value = ''
  await vpn.deleteRuleSet(set.id)
  confirmDeleteRuleSet.value = null
}

// ============================ 交互 ============================

async function handleConnect() {
  // 已连接或正在收尾时，同一个按钮要执行断开，否则会出现「点断开反而重连」
  await vpn.toggleConnection()
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

function openCreateProfile() {
  editingProfile.value = null
  profileDialogOpen.value = true
}

function openEditProfile(profile: VpnProfile) {
  editingProfile.value = profile
  profileDialogOpen.value = true
}

async function handleProfileSubmit(input: NewVpnProfileInput) {
  if (savingProfile.value) return
  savingProfile.value = true
  const isEdit = !!editingProfile.value
  try {
    if (editingProfile.value) {
      const patch: Partial<VpnProfile> = {
        name: input.name,
        server: input.server,
        username: input.username ?? '',
        protocol: input.protocol ?? 'anyconnect',
        extra_args: input.extra_args ?? [],
      }
      if (input.password) {
        patch.password = input.password
        patch.password_encrypted = false
      }
      await vpn.updateProfile(editingProfile.value.id, patch)
      vpn.pushLog('info', `已更新 VPN 配置: ${patch.name}`)
    } else {
      await vpn.addProfile(input)
    }
    profileDialogOpen.value = false
    editingProfile.value = null
  } catch (err: any) {
    vpn.pushLog('error', `${isEdit ? '更新' : '新建'} VPN 配置失败: ${err}`)
  } finally {
    savingProfile.value = false
  }
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
    <div class="vpn-body">
      <!-- 左侧：配置列表（再次点击左侧「代理」导航可收起） -->
      <aside id="vpn-profile-menu" v-show="!menuCollapsed" class="profile-list">
        <div class="list-head">
          <span>VPN 配置</span>
          <button class="list-add" @click="openCreateProfile">+ 新建</button>
        </div>
        <div class="list-body">
          <div
            v-for="profile in vpn.profiles"
            :key="profile.id"
            class="list-item"
            :class="{ active: profile.id === vpn.selectedProfileId }"
            @click="vpn.selectProfile(profile.id)"
          >
            <div class="item-main">
              <VpnStatusDot
                class="item-dot"
                :state="profileDotState(profile)"
                :label="dotLabel(profileDotState(profile))"
              />
              <span class="item-name">{{ profile.name }}</span>
              <span class="item-sub" :class="{ mono: profile.server }">{{ profile.server || '未填写服务器' }}</span>
            </div>
            <button
              class="item-edit"
              type="button"
              title="编辑配置"
              aria-label="编辑配置"
              :disabled="vpn.isConnected || vpn.phaseBusy"
              @click.stop="openEditProfile(profile)"
            >
              <svg viewBox="64 64 896 896" aria-hidden="true">
                <path
                  d="M257.7 752c2 0 4-.2 6-.5L431.9 722c2-.4 3.9-1.3 5.3-2.8l423.9-423.9a9.96 9.96 0 0 0 0-14.1L694.9 114.6c-1.9-1.9-4.4-2.9-7.1-2.9s-5.2 1-7.1 2.9L256.8 538.8c-1.5 1.5-2.4 3.3-2.8 5.3l-29.5 168.2a33.5 33.5 0 0 0 9.4 29.8c6.6 6.4 14.9 9.9 23.8 9.9zm67.4-174.4L687.8 215l73.3 73.3-362.7 362.6-88.9 15.7 15.6-89zM880 836H144c-17.7 0-32 14.3-32 32v36c0 4.4 3.6 8 8 8h784c4.4 0 8-3.6 8-8v-36c0-17.7-14.3-32-32-32z"
                />
              </svg>
            </button>
          </div>
          <div v-if="vpn.profiles.length === 0" class="list-empty">
            还没有配置，点上方「+ 新建」创建一个
          </div>
        </div>

        <!-- 提权状态：固定在列表底部，不跟配置列表一起滚动 -->
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
        <template v-if="vpn.selectedProfile">
          <div class="detail-toolbar">
            <div class="profile-identity">
              <div class="identity-name-row">
                <VpnStatusDot
                  :state="selectedDotState"
                  :label="dotLabel(selectedDotState)"
                  size="md"
                />
                <span class="identity-name">{{ vpn.selectedProfile.name }}</span>
                <button
                  class="identity-edit"
                  type="button"
                  title="编辑配置"
                  aria-label="编辑配置"
                  :disabled="vpn.isConnected || vpn.phaseBusy"
                  @click="openEditProfile(vpn.selectedProfile!)"
                >
                  <svg viewBox="64 64 896 896" aria-hidden="true">
                    <path
                      d="M257.7 752c2 0 4-.2 6-.5L431.9 722c2-.4 3.9-1.3 5.3-2.8l423.9-423.9a9.96 9.96 0 0 0 0-14.1L694.9 114.6c-1.9-1.9-4.4-2.9-7.1-2.9s-5.2 1-7.1 2.9L256.8 538.8c-1.5 1.5-2.4 3.3-2.8 5.3l-29.5 168.2a33.5 33.5 0 0 0 9.4 29.8c6.6 6.4 14.9 9.9 23.8 9.9zm67.4-174.4L687.8 215l73.3 73.3-362.7 362.6-88.9 15.7 15.6-89zM880 836H144c-17.7 0-32 14.3-32 32v36c0 4.4 3.6 8 8 8h784c4.4 0 8-3.6 8-8v-36c0-17.7-14.3-32-32-32z"
                    />
                  </svg>
                </button>
              </div>
              <span class="identity-server" :class="{ mono: vpn.selectedProfile.server }">
                {{ vpn.selectedProfile.server || '未填写服务器' }}
              </span>
            </div>

            <div class="status-actions">
              <button
                class="btn"
                :class="vpn.isConnected || vpn.phaseBusy ? 'btn-danger' : 'btn-primary'"
                :disabled="!canConnect && !vpn.isConnected"
                :title="connectBlockedReason"
                @click="handleConnect"
              >
                {{ connectLabel }}
              </button>
              <button
                class="btn btn-danger-ghost"
                :disabled="vpn.isConnected || vpn.phaseBusy"
                @click="askDelete(vpn.selectedProfile!)"
              >
                删除
              </button>
              <button
                class="btn btn-secondary"
                :disabled="vpn.detecting"
                @click="vpn.refreshConnectors()"
              >
                {{ vpn.detecting ? '检测中…' : '重新检测连接器' }}
              </button>
              <button class="btn btn-secondary" @click="vpn.openSessionDir()">打开日志目录</button>
            </div>
          </div>

          <VpnInstallGuide
            v-if="vpn.noConnector"
            :guide="vpn.guide"
            :can-import="!platform.is_windows"
            @import="handleImport"
            @copy-command="(cmd: string) => vpn.pushLog('info', `已复制命令: ${cmd}`)"
          />
          <p v-if="importHint" class="import-hint">{{ importHint }}</p>

          <!-- 代理模式：和 Clash 一样在规则/全局/直连之间切换 -->
          <section class="proxy-mode-card">
            <div class="proxy-mode-head">
              <span class="proxy-mode-icon">
                <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
                  <rect x="3" y="8" width="14" height="7" rx="2" />
                  <path d="M6 8V5.5h8V8M7 13h.01M10.5 13h.01" stroke-linecap="round" />
                  <path d="M16 4.5l1.5-1.5M17.5 4.5L16 3" stroke-linecap="round" />
                </svg>
              </span>
              <div class="proxy-mode-title">
                <h4>代理模式</h4>
                <p>决定连接后流量从 VPN 隧道还是本机出口走</p>
              </div>
              <span class="proxy-mode-current">{{ currentMode.label }}</span>
            </div>

            <div class="proxy-mode-options" role="radiogroup" aria-label="代理模式">
              <button
                v-for="mode in MODE_OPTIONS"
                :key="mode.value"
                type="button"
                class="proxy-mode-option"
                :class="{ active: mode.value === proxyMode }"
                role="radio"
                :aria-checked="mode.value === proxyMode"
                :disabled="modeLocked"
                :title="modeLocked ? '当前连接正在生效，断开 VPN 后可切换代理模式' : mode.description"
                @click="handleModeChange(mode.value)"
              >
                <span class="mode-icon">
                  <svg v-if="mode.value === 'rule'" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
                    <path d="M3 6h4m2 0h8M3 14h7m2 0h5" stroke-linecap="round" />
                    <circle cx="9" cy="6" r="2" />
                    <circle cx="12" cy="14" r="2" />
                  </svg>
                  <svg v-else-if="mode.value === 'global'" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
                    <circle cx="10" cy="10" r="7" />
                    <path d="M3 10h14M10 3c2.4 2.2 2.4 11.8 0 14M10 3c-2.4 2.2-2.4 11.8 0 14" />
                  </svg>
                  <svg v-else viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
                    <path d="M10 2.5l7.5 7.5-7.5 7.5L2.5 10z" />
                    <path d="M7 10l2 2 4-4" stroke-linecap="round" stroke-linejoin="round" />
                  </svg>
                </span>
                <span>{{ mode.label }}</span>
              </button>
            </div>

            <p class="proxy-mode-desc">{{ currentMode.description }}</p>
            <p v-if="modeLocked" class="proxy-mode-lock">当前连接正在生效，断开 VPN 后才能切换代理模式。</p>
          </section>

          <!-- 规则模式：页面保留多份列表，但同一时刻只有点选的那一份生效 -->
          <section v-if="proxyMode === 'rule'" class="card rules-card">
            <div class="rules-head">
              <div>
                <h4>规则列表</h4>
                <p>
                  页面可以有多份列表，但同一时刻只有一份生效。点击列表切换，规则内容在弹窗里编辑。
                </p>
              </div>
              <button class="btn btn-secondary btn-xs" @click="handleNewRuleSet">
                + 新建列表
              </button>
            </div>

            <div v-if="vpn.ruleSets.length" class="rule-set-list" role="radiogroup" aria-label="规则列表">
              <div
                v-for="set in vpn.ruleSets"
                :key="set.id"
                class="rule-set-item"
                :class="{ active: set.id === vpn.selectedRuleSet?.id }"
                role="radio"
                :aria-checked="set.id === vpn.selectedRuleSet?.id"
                tabindex="0"
                @click="handleSelectRuleSet(set)"
                @keydown.enter="handleSelectRuleSet(set)"
              >
                <span class="rule-set-radio"></span>
                <div class="rule-set-main">
                  <div class="rule-set-title">
                    <span class="rule-set-name">{{ set.name }}</span>
                    <span v-if="set.id === vpn.selectedRuleSet?.id" class="rule-set-badge">
                      当前生效
                    </span>
                    <span class="rule-set-count">
                      {{ enabledRules(set).length }} / {{ set.rules.length }} 条启用
                    </span>
                  </div>
                  <div class="rule-set-preview">
                    <span v-for="rule in previewRules(set)" :key="rule.id" class="rule-chip">
                      <span class="mono">{{ RULE_KIND_LABELS[rule.kind] }}</span>
                      <span class="rule-chip-value">{{ rule.value }}</span>
                      <em :class="rule.policy === 'VPN' ? 'policy-vpn' : 'policy-direct'">
                        {{ POLICY_LABELS[rule.policy] }}
                      </em>
                    </span>
                    <span v-if="hiddenRuleCount(set)" class="rule-more">
                      +{{ hiddenRuleCount(set) }}
                    </span>
                    <span v-if="enabledRules(set).length === 0" class="rule-set-none">
                      还没有启用的规则
                    </span>
                  </div>
                </div>
                <div class="rule-set-actions" @click.stop>
                  <button type="button" @click="openRuleSet(set)">编辑</button>
                  <button type="button" class="danger" @click="askDeleteRuleSet(set)">删除</button>
                </div>
              </div>
            </div>

            <div v-else class="rule-set-empty">
              还没有规则列表。新建一份后，在弹窗里填写规则。
              <button class="link-btn" @click="handleNewRuleSet">新建一份</button>
            </div>

            <div v-if="vpn.selectedRuleSet" class="rule-fallback">
              <span class="rule-fallback-label">全部规则不命中时</span>
              <select class="rule-fallback-select" :value="vpn.selectedRuleSet.default_policy" @change="handleRuleSetDefaultPolicy">
                <option value="DIRECT">直连</option>
                <option value="VPN">走 VPN</option>
              </select>
              <span class="rule-fallback-hint">这份列表的 MATCH 走向</span>
            </div>
          </section>

          <div v-else class="mode-off-note">
            当前是{{ currentMode.label }}模式，规则列表不参与分流；切换到「规则」模式后可编辑和选择列表。
          </div>

          <div class="card">
            <div class="card-head">
              <h4>连接配置</h4>
              <span class="card-sub">修改会自动保存</span>
            </div>
            <VpnProfileForm
              :profile="vpn.selectedProfile"
              :busy="vpn.isConnected || vpn.phaseBusy"
              @update="handleUpdate"
            />
          </div>

          <VpnLearnCard :vpn="vpn" />

          <VpnRoutePreview :status="vpn.status" />
        </template>

        <div v-else class="detail-empty detail-empty-create">
          <template v-if="vpn.profiles.length === 0">
            <div class="empty-title">还没有 VPN 配置</div>
            <p class="empty-sub">新建一个配置，填写服务器和账号后即可连接。</p>
            <button class="btn btn-primary" @click="openCreateProfile">新增 VPN</button>
          </template>
          <span v-else>请选择左侧的 VPN 配置</span>
        </div>
      </section>
    </div>

    <!-- 新建 / 编辑配置：同一个表单，传 profile 即为编辑 -->
    <VpnProfileDialog
      v-if="profileDialogOpen"
      :profile="editingProfile"
      :busy="savingProfile"
      @close="profileDialogOpen = false"
      @submit="handleProfileSubmit"
    />

    <!-- 删除确认 -->
    <Modal
      v-if="confirmDelete"
      :open="true"
      title="删除配置"
      ok-text="确定删除"
      cancel-text="取消"
      ok-type="danger"
      :mask-closable="false"
      @ok="confirmDeleteProfile"
      @cancel="confirmDelete = null"
    >
      <p class="dialog-message">
        确定要删除「{{ confirmDelete.name }}」吗？规则列表会保留，之后可以在规则列表里单独删除。
      </p>
    </Modal>

    <!-- 规则列表删除确认 -->
    <Modal
      v-if="confirmDeleteRuleSet"
      :open="true"
      title="删除规则列表"
      ok-text="确定删除"
      cancel-text="取消"
      ok-type="danger"
      :mask-closable="false"
      @ok="confirmDeleteSelectedRuleSet"
      @cancel="confirmDeleteRuleSet = null"
    >
      <p class="dialog-message">
        确定要删除「{{ confirmDeleteRuleSet.name }}」吗？列表里的规则会一起删除。
        <template v-if="confirmDeleteRuleSet.id === vpn.selectedRuleSet?.id">
          它当前正在生效，删除后需要重新选择一份列表。
        </template>
      </p>
    </Modal>

    <!-- 规则编辑弹窗：页面只展示列表和摘要，完整规则在这里填 -->
    <VpnRuleSetDialog
      v-if="editingRuleSet"
      :rule-set="editingRuleSet"
      :validate-rule="vpn.validateRule"
      @close="closeRuleDialog"
      @rename="handleRenameRuleSet"
      @add="handleAddRule"
      @update="handleUpdateRule"
      @remove="handleRemoveRule"
      @update-default-policy="handleDefaultPolicy"
    />
  </div>
</template>

<style scoped>
.vpn-layout {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

/* 详情顶部：左侧配置信息，右侧连接操作 */
.detail-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px 16px;
  flex-wrap: wrap;
  padding: 2px 0 2px;
}
.profile-identity {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}
.identity-name {
  min-width: 0;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.identity-name-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.identity-edit {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: none;
  border-radius: 5px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.identity-edit svg {
  width: 14px;
  height: 14px;
  fill: currentColor;
}
.identity-edit:hover:not(:disabled) {
  background: rgba(59, 130, 246, 0.1);
  color: var(--primary);
}
.identity-edit:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.identity-server {
  font-size: 12px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.status-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
  flex-wrap: wrap;
  justify-content: flex-end;
  margin-left: auto;
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
  overflow: hidden;
}
.list-head {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: var(--panel-header-height);
  padding: 0 16px;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.2px;
  color: #000;
  border-bottom: 1px solid var(--border-color);
}
.list-add {
  padding: 0;
  border: none;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  font-weight: 500;
  line-height: 1;
  cursor: pointer;
}
.list-add:hover {
  color: var(--primary-hover);
  text-decoration: underline;
  text-underline-offset: 2px;
}
.list-add:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: 1px;
}
.list-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 6px 8px 8px;
}
.list-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 8px 7px 10px;
  margin-bottom: 2px;
  border-radius: 8px;
  color: var(--text-secondary);
  user-select: none;
  cursor: pointer;
  transition: background 0.12s, color 0.12s, box-shadow 0.12s;
}
.list-item:hover {
  background: var(--bg-hover);
}
.list-item.active {
  background: var(--bg-active);
  box-shadow: inset 0 0 0 1px rgba(59, 130, 246, 0.18);
}
.item-main {
  flex: 1;
  min-width: 0;
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  align-items: center;
  column-gap: 7px;
  row-gap: 2px;
}
.item-dot {
  grid-column: 1;
  grid-row: 1;
  align-self: center;
}
.item-name {
  grid-column: 2;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 14px;
  font-weight: 500;
  color: var(--text-primary);
}
.list-item.active .item-name {
  color: var(--text-active);
  font-weight: 600;
}
.item-sub {
  grid-column: 2;
  font-size: 12px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.list-item.active .item-sub {
  color: var(--text-secondary);
}
.item-edit {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.12s, background 0.12s, color 0.12s;
}
.item-edit svg {
  width: 14px;
  height: 14px;
  fill: currentColor;
}
.list-item:hover .item-edit,
.list-item.active .item-edit,
.item-edit:focus-visible {
  opacity: 1;
}
.item-edit:hover:not(:disabled) {
  background: rgba(59, 130, 246, 0.1);
  color: var(--primary);
}
.item-edit:disabled {
  opacity: 0;
  cursor: not-allowed;
}
.list-empty {
  padding: 18px 8px;
  font-size: 12px;
  color: var(--text-muted);
  text-align: center;
}
/* 提权状态固定在列表底部，像一条状态栏而不是一块老式设置卡片 */
.elevation-box {
  flex-shrink: 0;
  margin: 0;
  padding: 12px 16px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-primary);
}
.elevation-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 6px;
}
.elevation-title::before {
  content: '';
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--warning);
}
.elevation-hint {
  font-size: 12px;
  line-height: 1.55;
  color: var(--text-muted);
}
.elevation-actions {
  margin-top: 10px;
}
.elevation-actions .btn {
  height: 28px;
  padding: 0 10px;
  font-size: 12px;
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
.rule-empty {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  text-align: center;
  font-size: 13px;
  color: var(--text-muted);
  background: var(--bg-primary);
}
.card-sub {
  font-size: 12px;
  color: var(--text-muted);
}

/* 代理模式 */
.proxy-mode-card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  background: var(--bg-primary);
}
.proxy-mode-head {
  display: flex;
  align-items: center;
  gap: 10px;
}
.proxy-mode-icon {
  width: 34px;
  height: 34px;
  display: grid;
  place-items: center;
  flex-shrink: 0;
  border-radius: 8px;
  background: color-mix(in srgb, var(--primary) 14%, transparent);
  color: var(--primary);
}
.proxy-mode-icon svg {
  width: 20px;
  height: 20px;
}
.proxy-mode-title {
  flex: 1;
  min-width: 0;
}
.proxy-mode-title h4 {
  margin: 0 0 2px;
  font-size: 14px;
  color: var(--text-primary);
}
.proxy-mode-title p {
  margin: 0;
  font-size: 11px;
  color: var(--text-muted);
}
.proxy-mode-current {
  flex-shrink: 0;
  padding: 2px 9px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--primary) 12%, transparent);
  color: var(--primary);
  font-size: 11px;
}
.proxy-mode-options {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px;
  margin-top: 12px;
}
.proxy-mode-option {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  height: 40px;
  border: 1px solid var(--border-color);
  border-radius: 7px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 13px;
  cursor: pointer;
}
.proxy-mode-option:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--text-primary);
}
.proxy-mode-option.active {
  border-color: var(--primary);
  background: var(--primary);
  color: #fff;
  box-shadow: 0 4px 12px color-mix(in srgb, var(--primary) 22%, transparent);
}
.proxy-mode-option:disabled {
  cursor: not-allowed;
  opacity: 0.72;
}
.mode-icon {
  display: grid;
  place-items: center;
}
.mode-icon svg {
  width: 18px;
  height: 18px;
  display: block;
}
.proxy-mode-desc {
  margin: 10px 0 0;
  padding: 8px 10px;
  border: 1px solid color-mix(in srgb, var(--primary) 40%, var(--border-color));
  border-radius: 5px;
  background: color-mix(in srgb, var(--primary) 5%, transparent);
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.6;
}
.proxy-mode-lock {
  margin: 6px 0 0;
  font-size: 11px;
  color: var(--warning);
}

/* 规则列表：多份可选，但只有一份生效 */
.rules-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 10px;
}
.rules-head h4 {
  margin: 0 0 3px;
  font-size: 14px;
  color: var(--text-primary);
}
.rules-head p {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
.rules-head .btn {
  flex-shrink: 0;
  white-space: nowrap;
}
.rule-set-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.rule-set-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px;
  border: 1px solid var(--border-color);
  border-radius: 7px;
  background: var(--bg-secondary);
  cursor: pointer;
}
.rule-set-item:hover {
  background: var(--bg-hover);
}
.rule-set-item.active {
  border-color: var(--primary);
  background: color-mix(in srgb, var(--primary) 7%, var(--bg-primary));
}
.rule-set-radio {
  position: relative;
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  border: 1.5px solid var(--text-muted);
  border-radius: 50%;
}
.rule-set-item.active .rule-set-radio {
  border-color: var(--primary);
}
.rule-set-item.active .rule-set-radio::after {
  content: '';
  position: absolute;
  inset: 3px;
  border-radius: 50%;
  background: var(--primary);
}
.rule-set-main {
  flex: 1;
  min-width: 0;
}
.rule-set-title {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}
.rule-set-name {
  font-size: 13px;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.rule-set-badge {
  flex-shrink: 0;
  padding: 1px 6px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--primary) 14%, transparent);
  color: var(--primary);
  font-size: 10px;
}
.rule-set-count {
  margin-left: auto;
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-muted);
}
.rule-set-preview {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 5px;
  margin-top: 5px;
  min-height: 18px;
}
.rule-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 100%;
  padding: 1px 6px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--bg-primary);
  color: var(--text-muted);
  font-size: 10px;
}
.rule-chip .mono {
  color: var(--text-secondary);
}
.rule-chip-value {
  max-width: 150px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-secondary);
}
.rule-chip em {
  font-style: normal;
}
.policy-vpn {
  color: var(--primary);
}
.policy-direct {
  color: var(--success);
}
.rule-more,
.rule-set-none {
  font-size: 11px;
  color: var(--text-muted);
}
.rule-set-actions {
  display: flex;
  flex-shrink: 0;
  gap: 6px;
}
.rule-set-actions button {
  border: none;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  padding: 3px 4px;
  cursor: pointer;
}
.rule-set-actions button:hover {
  color: var(--primary);
}
.rule-set-actions button.danger:hover {
  color: var(--danger);
}
.rule-set-empty {
  padding: 16px 10px;
  border: 1px dashed var(--border-color);
  border-radius: 7px;
  text-align: center;
  font-size: 12px;
  color: var(--text-muted);
}
.link-btn {
  margin-left: 4px;
  border: none;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  cursor: pointer;
}
.rule-fallback {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--border-color);
  font-size: 12px;
  color: var(--text-muted);
}
.rule-fallback-select {
  height: 26px;
  padding: 0 6px;
  border: 1px solid var(--border-color);
  border-radius: 5px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: 12px;
}
.rule-fallback-hint {
  color: var(--text-muted);
}
.mode-off-note {
  padding: 10px 14px;
  border: 1px dashed var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.6;
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
.detail-empty-create {
  flex-direction: column;
  gap: 10px;
  text-align: center;
}
.empty-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
}
.empty-sub {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}
.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}

/* 弹窗正文 */
.dialog-message {
  margin: 0;
  font-size: 14px;
  color: var(--text-secondary);
  line-height: 1.6;
}
</style>
