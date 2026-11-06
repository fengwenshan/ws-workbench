<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Modal } from 'ant-design-vue'
import type { VpnApi } from '@/composables/useVpn'
import type {
  NewVpnProfileInput,
  VpnProfile,
  VpnProxyMode,
} from '@/types/vpn'
import { platform } from '@/platform'
import VpnInstallGuide from './VpnInstallGuide.vue'
import VpnLogDrawer from './VpnLogDrawer.vue'
import VpnProfileDialog from './VpnProfileDialog.vue'
import VpnRoutePreview from './VpnRoutePreview.vue'
import VpnRuleDialog from './VpnRuleDialog.vue'
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
/** 规则编辑弹窗：入口在动作栏「规则」按钮 */
const ruleDialogOpen = ref(false)
/** 实时日志抽屉 */
const logDrawerOpen = ref(false)
/** 需要断开重连的切换目标（等用户在弹窗里确认） */
const reconnectTarget = ref<VpnProfile | null>(null)
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

const connectionBusy = computed(
  () => vpn.connecting || vpn.disconnecting || vpn.phaseBusy,
)

const connectLabel = computed(() => {
  if (vpn.disconnecting) return '断开中…'
  if (vpn.connecting || vpn.phaseBusy) return '处理中…'
  if (!vpn.isConnected) return '连接'
  if (isPolicyOwner.value) return '断开'
  return vpn.sharesTunnel(vpn.selectedProfile?.id) ? '切换到该配置' : '切换（需断开重连）'
})

// ============================ 状态点 ============================

type ProfileDotState = 'off' | 'on' | 'busy'

/** 正在连接/已连接的配置；连接阶段后端还没写 profile_id 时回退到当前选中项 */
const activeProfileId = computed(() => vpn.status.profile_id || vpn.selectedProfileId)
/** 运行期「当前生效配置」：同通道共享隧道时，规则来自它 */
const policyOwnerId = computed(() => vpn.policyProfileId || vpn.status.profile_id)
const isPolicyOwner = computed(
  () => !!vpn.selectedProfile && vpn.selectedProfile.id === policyOwnerId.value,
)

function profileDotState(profile: VpnProfile): ProfileDotState {
  if (vpn.isConnected) {
    // 共用同一条隧道但不是当前生效配置：灰点，点一下就能切过去
    return policyOwnerId.value === profile.id ? 'on' : 'off'
  }
  if (activeProfileId.value !== profile.id) return 'off'
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

const canConnect = computed(
  () => !!vpn.selectedProfile && !vpn.connecting && !vpn.disconnecting,
)

// ============================ 代理模式 ============================

/**
 * 乐观值：点下去立刻高亮，不等后端往返。
 * 成功就等实际状态追上后自动撤掉；失败由 handleModeChange 回选上一个。
 */
const optimisticMode = ref<VpnProxyMode | null>(null)
/** 上次点击的序号：连点时只让最新那一次决定要不要回滚 */
let switchSeq = 0

const proxyMode = computed<VpnProxyMode>(
  () => optimisticMode.value ?? vpn.selectedProfile?.proxy_mode ?? 'rule',
)
/** 已连接时以 worker 回写的实际模式为准：界面上的选择只代表「下次连接」 */
const appliedMode = computed<VpnProxyMode | null>(() => {
  if (!vpn.isConnected) return null
  const raw = String(vpn.status?.applied_mode ?? '').trim().toLowerCase()
  if (raw === 'rule' || raw === 'global' || raw === 'direct') return raw
  return null
})
/** 高亮：正在切换的目标 > worker 实际模式 > 配置里的模式 */
const activeMode = computed<VpnProxyMode>(
  () => optimisticMode.value ?? appliedMode.value ?? proxyMode.value,
)
const modeMismatch = computed(
  () =>
    !optimisticMode.value &&
    !!appliedMode.value &&
    appliedMode.value !== proxyMode.value,
)
const modeLabel = (mode: VpnProxyMode) =>
  MODE_OPTIONS.find((item) => item.value === mode)?.label ?? mode
const currentMode = computed(
  () => MODE_OPTIONS.find((item) => item.value === proxyMode.value) ?? MODE_OPTIONS[0],
)
const modeLocked = computed(
  () =>
    vpn.phaseBusy ||
    vpn.connecting ||
    vpn.disconnecting ||
    vpn.modeSwitching ||
    (vpn.isConnected && !isPolicyOwner.value),
)

const modeLockText = computed(() => {
  // 切换中这里不再重复：卡片标题已经写着「正在切换到「X」，等待 worker 确认…」
  if (vpn.modeSwitching) return ''
  if (vpn.disconnecting) return '正在断开连接，暂时不能切换代理模式。'
  if (vpn.phaseBusy || vpn.connecting) return '正在连接中，暂时不能切换代理模式。'
  if (vpn.isConnected && !isPolicyOwner.value) {
    return '这份配置当前没有生效，切过去之后才能改它的代理模式。'
  }
  return ''
})

async function handleModeChange(mode: VpnProxyMode) {
  if (mode === activeMode.value || modeLocked.value) return
  const previous = activeMode.value
  const seq = ++switchSeq
  // 先选中，再切换：点下去就有反应，不等 worker 回报
  optimisticMode.value = mode
  try {
    const ok = await vpn.setProxyMode(mode)
    if (seq !== switchSeq) return // 期间又点了别的，交给最新那次
    if (!ok) {
      optimisticMode.value = previous
      vpn.pushLog('warn', `切换没成功，已回到「${modeLabel(previous)}」`)
      return
    }
    settleOptimistic(mode)
  } catch (err: any) {
    if (seq === switchSeq) optimisticMode.value = previous
    vpn.pushLog('error', `切换代理模式失败: ${err}`)
  }
}

// 换了一份配置：乐观值属于上一份配置，直接丢掉
watch(
  () => vpn.selectedProfile?.id,
  () => {
    optimisticMode.value = null
  },
)

/**
 * 切换成功后撤掉乐观值，让高亮交回真实状态。
 *
 * 不能立刻撤：已连接时高亮取 worker 回写的 applied_mode，它要等状态轮询
 * 才追上来，立刻撤会看到「闪回旧模式」。全局模式要撤/装半默认路由和 DNS，
 * 现场约 4 秒，3 秒超时会在 worker 回报前先把高亮交回旧模式，等下一轮状态
 * 到了再跳回新模式——表现就是「切过去、又自动切回来、再切过去」。
 * 这里放宽到 15 秒，并且只认真正匹配；超时只告警，不再假装已到达。
 */
function settleOptimistic(mode: VpnProxyMode) {
  const deadline = Date.now() + 15_000
  const timer = window.setInterval(() => {
    if (optimisticMode.value !== mode) {
      // 用户又点了别的模式，这次不用管了
      window.clearInterval(timer)
      return
    }
    const reached = vpn.isConnected
      ? appliedMode.value === mode
      : vpn.selectedProfile?.proxy_mode === mode
    if (reached) {
      optimisticMode.value = null
      window.clearInterval(timer)
      return
    }
    if (Date.now() > deadline) {
      // 真实状态一直没追上：交回真实值，并在日志里留下一句，别无限高亮
      optimisticMode.value = null
      window.clearInterval(timer)
      vpn.pushLog('warn', `模式切换后 15 秒内没有等到 worker 确认，已按实际状态显示`)
    }
  }, 120)
}

// ============================ 交互 ============================

async function handleConnect() {
  if (vpn.connecting || vpn.disconnecting) return
  // 未连接或收尾中：走原来的连接/断开逻辑
  if (!vpn.isConnected || vpn.phaseBusy) {
    await vpn.toggleConnection()
    return
  }
  const profile = vpn.selectedProfile
  if (!profile) return
  // 选中的就是当前生效配置：同一个按钮负责断开
  if (isPolicyOwner.value) {
    await vpn.disconnect()
    return
  }
  // 同通道：隧道复用，秒级切换
  if (vpn.sharesTunnel(profile.id)) {
    try {
      const result = await vpn.switchToProfile(profile)
      if (result === 'reconnect') reconnectTarget.value = profile
    } catch (err: any) {
      vpn.pushLog('error', `切换配置失败: ${err}`)
    }
    return
  }
  // 不同通道：先让用户确认，再断开重连
  reconnectTarget.value = profile
}

async function confirmReconnect() {
  const profile = reconnectTarget.value
  if (!profile) return
  reconnectTarget.value = null
  await vpn.disconnect()
  await vpn.connect(profile)
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

/** 切回 VPN 页时刷新连接器与快照，保证看到的是当前机器状态 */
watch(
  () => props.active,
  async (isActive) => {
    if (!isActive) return
    await vpn.refreshSnapshot()
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
            :class="{
              active: profile.id === vpn.selectedProfileId,
              'list-item--shared': vpn.sharesTunnel(profile.id),
            }"
            @click="vpn.selectProfile(profile.id)"
          >
            <div class="item-main">
              <VpnStatusDot
                class="item-dot"
                :state="profileDotState(profile)"
                :label="vpn.sharesTunnel(profile.id) ? '共用通道' : dotLabel(profileDotState(profile))"
              />
              <span class="item-name">
                {{ profile.name }}
                <em v-if="vpn.sharesTunnel(profile.id)" class="item-shared">共用通道</em>
              </span>
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
                :class="[
                  vpn.isConnected || vpn.phaseBusy ? 'btn-danger' : 'btn-primary',
                  { 'is-loading': connectionBusy },
                ]"
                :disabled="vpn.disconnecting || (!canConnect && !vpn.isConnected)"
                :aria-busy="connectionBusy"
                @click="handleConnect"
              >
                <span v-if="connectionBusy" class="btn-spinner" aria-hidden="true" />
                {{ connectLabel }}
              </button>
              <button
                class="btn btn-secondary"
                :disabled="!vpn.selectedProfile || vpn.rulesApplying"
                :aria-busy="vpn.rulesApplying"
                @click="ruleDialogOpen = true"
              >
                <span v-if="vpn.rulesApplying" class="btn-spinner" aria-hidden="true" />
                {{ vpn.rulesApplying ? '应用中…' : '规则' }}
              </button>
              <button
                class="btn btn-secondary"
                :disabled="vpn.detecting"
                @click="vpn.refreshConnectors()"
              >
                {{ vpn.detecting ? '检测中…' : '检测' }}
              </button>
              <button class="btn btn-secondary" @click="logDrawerOpen = true">日志</button>
              <button
                class="btn btn-danger-ghost"
                :disabled="vpn.isConnected || vpn.phaseBusy"
                @click="askDelete(vpn.selectedProfile!)"
              >
                删除
              </button>
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
          <section
            class="proxy-mode-card"
            :class="{ 'is-switching': vpn.modeSwitching }"
            :aria-busy="vpn.modeSwitching"
          >
            <div class="proxy-mode-head">
              <span class="proxy-mode-icon">
                <!-- 切换期间这里转圈：比一颗安静的小圆点更容易被看见 -->
                <span
                  v-if="vpn.modeSwitching"
                  class="mode-spinner mode-spinner-lg"
                  aria-hidden="true"
                />
                <svg v-else viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
                  <rect x="3" y="8" width="14" height="7" rx="2" />
                  <path d="M6 8V5.5h8V8M7 13h.01M10.5 13h.01" stroke-linecap="round" />
                  <path d="M16 4.5l1.5-1.5M17.5 4.5L16 3" stroke-linecap="round" />
                </svg>
              </span>
              <div class="proxy-mode-title">
                <h4>代理模式</h4>
                <p v-if="vpn.modeSwitching">正在切换到「{{ modeLabel(activeMode) }}」，等待 worker 确认…</p>
                <p v-else>{{ currentMode.description }}</p>
              </div>
            </div>

            <div class="proxy-mode-options" role="radiogroup" aria-label="代理模式">
              <button
                v-for="mode in MODE_OPTIONS"
                :key="mode.value"
                type="button"
                class="proxy-mode-option"
                :class="{ active: mode.value === activeMode }"
                role="radio"
                :aria-checked="mode.value === activeMode"
                :disabled="modeLocked"
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
                <span class="mode-label">
                  <span
                    v-if="vpn.modeSwitching && mode.value === activeMode"
                    class="mode-spinner"
                    aria-hidden="true"
                  />
                  {{ mode.label }}
                </span>
              </button>
            </div>

            <p v-if="modeLockText" class="proxy-mode-lock">{{ modeLockText }}</p>
            <p v-else-if="modeMismatch" class="proxy-mode-lock">
              worker 里实际是「{{ modeLabel(activeMode) }}」，配置里是「{{ modeLabel(proxyMode) }}」——
              这次切换没有生效，可断开重连后再试。
            </p>
            <p v-else-if="vpn.isConnected" class="proxy-mode-hint">
              切换立即生效：规则/全局/直连都是本地布线（路由与 DNS），切换瞬间已建立的连接可能瞬断重连。
            </p>
          </section>

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

    <!-- 规则编辑：四个输入项 + 现有列表（可编辑），和 Clash 一个思路 -->
    <VpnRuleDialog v-if="ruleDialogOpen" :vpn="vpn" @close="ruleDialogOpen = false" />

    <!-- 实时日志抽屉：入口在动作栏「日志」 -->
    <VpnLogDrawer v-if="logDrawerOpen" :vpn="vpn" @close="logDrawerOpen = false" />

    <!-- 不同通道：确认后才断开重连，避免用户没意识到会断开 -->
    <Modal
      v-if="reconnectTarget"
      :open="true"
      title="需要断开重连"
      ok-text="断开并连接"
      cancel-text="取消"
      :mask-closable="false"
      :confirm-loading="vpn.connecting"
      @ok="confirmReconnect"
      @cancel="reconnectTarget = null"
    >
      <p class="dialog-message">
        「{{ reconnectTarget.name }}」与当前通道的连接参数不同（服务器、账号、密码、协议、
        连接器或额外参数有变化），需要先断开当前连接，再重新建立隧道。只是规则或代理模式
        不同的话可以直接切，不用断开。
      </p>
    </Modal>

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
        确定要删除「{{ confirmDelete.name }}」吗？这份配置里的规则也会一起删除。
      </p>
    </Modal>

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
  gap: 4px;
  min-width: 0;
}
.identity-name {
  min-width: 0;
  font-size: 16px;
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
  border-radius: 6px;
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
  padding: 8px 8px 8px 10px;
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
/* 共用同一条隧道但不是当前生效配置：灰掉，点一下即可秒切过去 */
.list-item--shared {
  color: var(--text-muted);
}
.list-item--shared .item-name {
  color: var(--text-muted);
}
.item-shared {
  margin-left: 6px;
  padding: 0 4px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  font-size: 10px;
  font-style: normal;
  color: var(--text-muted);
  white-space: nowrap;
}
.item-main {
  flex: 1;
  min-width: 0;
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  align-items: center;
  column-gap: 8px;
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
.rule-empty {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  text-align: center;
  font-size: 14px;
  color: var(--text-muted);
  background: var(--bg-primary);
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
  font-size: 12px;
  color: var(--text-muted);
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
  gap: 8px;
  height: 40px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 14px;
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
/* 选中项 hover 时不要再改文字颜色（上面那条 hover 会把白字压成深色，
   蓝底上很难看），轻微压暗表示可点即可 */
.proxy-mode-option.active:hover:not(:disabled) {
  color: #fff;
  opacity: 0.8;
}
.proxy-mode-option:disabled {
  cursor: not-allowed;
  opacity: 0.72;
}
.proxy-mode-card.is-switching {
  border-color: var(--primary);
}
.proxy-mode-card.is-switching .proxy-mode-icon {
  opacity: 1;
}
.mode-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.mode-icon {
  display: grid;
  place-items: center;
}

/* 操作按钮的加载圈：原生 button 里也能和文字并排 */
.btn-spinner,
.mode-spinner {
  display: inline-block;
  flex: none;
  width: 12px;
  height: 12px;
  border: 2px solid currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: ws-vpn-spin 0.7s linear infinite;
  vertical-align: -2px;
}
.btn-spinner {
  margin-right: 6px;
}
.mode-spinner {
  width: 12px;
  height: 12px;
}
/* 卡片标题旁那颗转圈：和 18px 的模式图标同尺寸占位 */
.mode-spinner-lg {
  width: 18px;
  height: 18px;
  border-width: 2px;
  color: var(--primary);
  vertical-align: 0;
}
@keyframes ws-vpn-spin {
  to {
    transform: rotate(360deg);
  }
}
.mode-icon svg {
  width: 18px;
  height: 18px;
  display: block;
}
.proxy-mode-lock {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--warning);
}
.proxy-mode-hint {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--text-muted);
}

.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.detail-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 14px;
  color: var(--text-muted);
}
.detail-empty-create {
  flex-direction: column;
  gap: 10px;
  text-align: center;
}
.empty-title {
  font-size: 16px;
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
