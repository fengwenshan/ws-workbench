import { ref, computed, onMounted, onUnmounted, reactive, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { AppConfig, LogEntry, LogLevel } from '@/types'
import { pickDirectory } from '@/utils/dialog'
import { nowIso, timeLabel } from '@/utils/time'
import type {
  ConnectorInfo,
  ConnectResult,
  ElevationInfo,
  InstallGuide,
  NewVpnProfileInput,
  SessionDirInfo,
  VpnPolicy,
  VpnProfile,
  VpnProxyMode,
  VpnRule,
  VpnRuleKind,
  VpnSnapshotPayload,
  VpnStatus,
} from '@/types/vpn'
import { BUSY_PHASES } from '@/types/vpn'

/** 后端事件名，与 Rust 侧 vpn/commands.rs 保持一致 */
const EVENT_STATUS = 'vpn-status'
const EVENT_LOG = 'vpn-log'
const EVENT_TRAY = 'vpn-tray'

function emptyStatus(): VpnStatus {
  return {
    phase: 'disconnected',
    message: '',
    profile_id: '',
    profile_name: '',
    conn_identity: '',
    connector: '',
    connector_name: '',
    tun_dev: '',
    tun_index: 0,
    vpn_ip: '',
    vpn_gateway: '',
    dns: [],
    mtu: 0,
    routes: [],
    snapshot: null,
    error: '',
    pid: 0,
    started_at: '',
    updated_at: '',
  }
}

/** 从日志文本里猜一个级别，让日志面板有颜色层次 */
function guessLevel(line: string): LogLevel {
  const lower = line.toLowerCase()
  if (lower.includes('失败') || lower.includes('错误') || lower.includes('failed') || lower.includes('error')) {
    return 'error'
  }
  if (lower.includes('超时') || lower.includes('警告') || lower.includes('warn')) return 'warn'
  if (lower.includes('已连接') || lower.includes('done') || lower.includes('完成')) return 'success'
  return 'info'
}

export function useVpn(config: Ref<AppConfig>, saveConfig: () => Promise<void>) {
  const connectors = ref<ConnectorInfo[]>([])
  const status = ref<VpnStatus>(emptyStatus())
  const elevation = ref<ElevationInfo | null>(null)
  const guide = ref<InstallGuide | null>(null)
  const logs = ref<LogEntry[]>([])
  /** 会话目录实际落在哪儿（Roaming 不可写时会回退） */
  const sessionDir = ref<SessionDirInfo | null>(null)
  const selectedProfileId = ref<string>('')
  const detecting = ref(false)
  const connecting = ref(false)

  let logId = 0
  const unlisteners: UnlistenFn[] = []

  // ============================ 派生状态 ============================

  const profiles = computed(() => config.value.vpn_profiles ?? [])

  const selectedProfile = computed<VpnProfile | null>(() => {
    if (!selectedProfileId.value) return null
    return profiles.value.find((p) => p.id === selectedProfileId.value) ?? null
  })

  const proxyMode = computed<VpnProxyMode>(
    () => selectedProfile.value?.proxy_mode ?? 'rule',
  )

  /** 运行期「当前生效配置」；同通道切换后指向切过去的那份 */
  const policyProfileId = ref('')
  /** 配置 id -> 通道身份哈希，用于渲染「共用通道」 */
  const identities = ref<Record<string, string>>({})

  /** 与当前通道参数相同、但不是当前生效的配置 */
  function sharesTunnel(profileId?: string | null): boolean {
    if (!profileId) return false
    const active = status.value.conn_identity
    if (!active || profiles.value.length === 0) return false
    if (profileId === policyProfileId.value) return false
    return identities.value[profileId] === active
  }

  async function refreshIdentities() {
    const list = profiles.value
    if (list.length === 0) {
      identities.value = {}
      return
    }
    try {
      const hashes = await invoke<string[]>('vpn_conn_identities', { profiles: list })
      identities.value = Object.fromEntries(list.map((profile, index) => [profile.id, hashes[index]]))
    } catch {
      // 旧后端没有这个命令时忽略，界面退化成不显示「共用通道」
    }
  }

  // 配置内容（服务器、账号、连接器参数等）一变，通道身份就跟着变
  watch(profiles, () => { void refreshIdentities() }, { deep: true, immediate: true })

  const activeConnectors = computed(() => connectors.value.filter((c) => c.available))
  const bestConnector = computed<ConnectorInfo | null>(
    () => activeConnectors.value.find((c) => c.kind === 'openconnect') ?? null,
  )
  const noConnector = computed(() => activeConnectors.value.length === 0)
  const phaseBusy = computed(() => BUSY_PHASES.includes(status.value.phase))
  const isConnected = computed(() => status.value.phase === 'connected')

  // ============================ 数据加载 ============================

  async function refreshConnectors() {
    detecting.value = true
    try {
      const profile = selectedProfile.value
      connectors.value = await invoke<ConnectorInfo[]>('vpn_detect_connectors', {
        customOpenconnect: profile?.connector_path || null,
      })
      guide.value = await invoke<InstallGuide>('vpn_install_guide', {
        candidates: connectors.value,
      })
    } finally {
      detecting.value = false
    }
  }

  async function refreshElevation() {
    try {
      elevation.value = await invoke<ElevationInfo>('vpn_elevation_info')
    } catch {
      elevation.value = null
    }
  }

  async function setupElevation() {
    const message = await invoke<string>('vpn_setup_elevation')
    await refreshElevation()
    pushLog('success', message)
  }

  async function removeElevation() {
    const message = await invoke<string>('vpn_remove_elevation')
    await refreshElevation()
    pushLog('info', message)
  }

  async function importOpenconnect() {
    const picked = await pickDirectory()
    if (!picked) return
    const message = await invoke<string>('vpn_import_openconnect', {
      sourceDir: picked,
    })
    pushLog('success', message)
    await refreshConnectors()
  }

  async function removeManagedOpenconnect() {
    const message = await invoke<string>('vpn_remove_managed_openconnect')
    pushLog('info', message)
    await refreshConnectors()
  }

  /** 拉取已有日志（切回 VPN 页时用），避免刷新后日志空白 */
  async function loadExistingLogs() {
    try {
      const lines = await invoke<string[]>('vpn_read_logs', { maxLines: 500 })
      lines.forEach((line) => pushLog(guessLevel(line), line, false))
    } catch {
      // 会话还没建立时读不到日志是正常的
    }
  }

  async function refreshSnapshot() {
    try {
      const payload = await invoke<VpnSnapshotPayload>('vpn_get_snapshot')
      status.value = payload.status
      policyProfileId.value = payload.policy_profile_id || payload.status.profile_id || ''
      if (payload.elevation) {
        elevation.value = { ...(elevation.value ?? ({} as ElevationInfo)), hint: payload.elevation } as ElevationInfo
      }
    } catch {
      // 忽略
    }
  }

  function pushLog(level: LogLevel, message: string, withTime = true) {
    logs.value.push({
      id: logId++,
      timestamp: withTime ? timeLabel() : '',
      level,
      message,
    })
    // 防止长时间连接把内存撑爆
    if (logs.value.length > 3000) {
      logs.value.splice(0, logs.value.length - 3000)
    }
  }

  function clearLogs() {
    logs.value = []
    invoke('vpn_clear_logs').catch(() => {})
  }

  // ============================ 连接 / 断开 ============================

  async function connect(profile?: VpnProfile | null) {
    const target = profile ?? selectedProfile.value
    if (!target) {
      pushLog('warn', '请先选择一个 VPN 配置')
      return
    }
    if (connecting.value || phaseBusy.value) return

    connecting.value = true
    try {
      const mode = target.proxy_mode ?? 'rule'
      // 规则来自配置自身；全局/直连模式不把规则交给后端参与判定
      const rules = mode === 'rule' ? (target.rules ?? []) : []
      // 规则模式没命中就走直连；全局模式全部走隧道，直连模式全部直连
      const defaultPolicy = mode === 'global' ? 'VPN' : 'DIRECT'
      if (mode === 'rule' && !rules.some((rule) => rule.enabled)) {
        pushLog('warn', '这个配置还没有启用的规则，所有流量都会直连')
      }
      const result = await invoke<ConnectResult>('vpn_connect', {
        profile: target,
        rules,
        defaultPolicy,
        customOpenconnect: target.connector_path || null,
      })
      pushLog(
        'info',
        `开始连接：连接器 ${result.connector_name}，提权方式 ${result.elevation}`,
      )
      await refreshElevation()
      await refreshSnapshot()
    } catch (err: any) {
      pushLog('error', `连接失败: ${err}`)
      await refreshSnapshot()
    } finally {
      connecting.value = false
    }
  }

  async function disconnect() {
    try {
      const message = await invoke<string>('vpn_disconnect')
      pushLog('info', message)
    } catch (err: any) {
      pushLog('error', `断开失败: ${err}`)
    } finally {
      await refreshSnapshot()
    }
  }

  /**
   * 同通道切换：隧道不动，只把运行期策略换成这份配置的规则与模式。
   * 返回 'shared' 表示已切换；'reconnect' 表示通道参数不同，需要断开重连。
   */
  async function switchToProfile(
    profile: VpnProfile,
  ): Promise<'shared' | 'reconnect' | 'none'> {
    if (!isConnected.value) return 'none'
    const shared = await invoke<boolean>('vpn_switch_profile', {
      profile,
      rules: profile.rules ?? [],
      defaultPolicy: profile.rules_default_policy ?? 'DIRECT',
    })
    if (!shared) return 'reconnect'
    policyProfileId.value = profile.id
    config.value.vpn_active_profile_id = profile.id
    selectedProfileId.value = profile.id
    await saveConfig()
    pushLog('info', `已切换到配置「${profile.name}」：通道复用，规则已生效`)
    return 'shared'
  }

  async function toggleConnection() {
    if (isConnected.value || phaseBusy.value) {
      await disconnect()
    } else {
      await connect()
    }
  }

  // ============================ 配置增删改 ============================

  function newRule(
    kind: VpnRuleKind = 'DOMAIN',
    value = '',
    policy: VpnPolicy = 'DIRECT',
  ): VpnRule {
    return { id: crypto.randomUUID(), kind, value, policy, no_resolve: true, remark: '', enabled: true }
  }

  async function addProfile(input: NewVpnProfileInput) {
    const name = input.name.trim() || '新建 VPN 配置'
    const server = input.server.trim()
    // 新配置默认带一条 corp 规则：域名里含 corp 的请求走 VPN，其余直连。
    // 用户仍可在规则表里改顺序、加网段或删掉这条。
    const corp = newRule('DOMAIN-KEYWORD', 'corp', 'VPN')
    corp.remark = 'corp 相关域名走 VPN'

    const now = nowIso()
    const profile: VpnProfile = {
      id: crypto.randomUUID(),
      name,
      server,
      username: input.username?.trim() ?? '',
      password: input.password ?? '',
      password_encrypted: false,
      protocol: input.protocol?.trim() || 'anyconnect',
      connector: 'auto',
      proxy_mode: 'rule',
      rules: [corp],
      rules_default_policy: 'DIRECT',
      fake_ip_range: input.fake_ip_range?.trim() || '198.18.0.0/16',
      fake_ip_filter: input.fake_ip_filter ?? [],
      extra_args: input.extra_args ?? [],
      connector_path: '',
      created_at: now,
      updated_at: now,
    }
    config.value.vpn_profiles.push(profile)
    config.value.vpn_active_profile_id = profile.id
    selectedProfileId.value = profile.id
    await saveConfig()
    return profile
  }

  async function updateProfile(id: string, patch: Partial<VpnProfile>) {
    const profile = profiles.value.find((p) => p.id === id)
    if (!profile) return
    Object.assign(profile, patch, { updated_at: nowIso() })
    await saveConfig()
  }

  async function deleteProfile(id: string) {
    const profile = profiles.value.find((p) => p.id === id)
    if (!profile) return
    config.value.vpn_profiles = profiles.value.filter((p) => p.id !== id)
    if (config.value.vpn_active_profile_id === id) {
      config.value.vpn_active_profile_id = config.value.vpn_profiles[0]?.id ?? ''
    }
    if (selectedProfileId.value === id) {
      selectedProfileId.value = config.value.vpn_profiles[0]?.id ?? ''
    }
    await saveConfig()
  }

  async function setProxyMode(mode: VpnProxyMode) {
    const profile = selectedProfile.value
    if (!profile || profile.proxy_mode === mode) return
    await updateProfile(profile.id, { proxy_mode: mode })
    const label = mode === 'rule' ? '规则' : mode === 'global' ? '全局' : '直连'
    if (isConnected.value) {
      try {
        await invoke('vpn_set_proxy_mode', { mode })
        pushLog(
          'info',
          `代理模式已切换为${label}：新连接按新模式，已建立的连接保持原路径`,
        )
      } catch (err: any) {
        pushLog('error', `切换代理模式失败: ${err}`)
      }
      return
    }
    pushLog('info', `代理模式已切换为${label}，下次连接生效`)
  }

  /**
   * 规则变更后推给正在运行的 worker（防抖 300ms）。
   *
   * 只推「当前连接中的那个配置」的规则：运行时只有一份规则表，
   * 连着 A 却在编辑 B 时推送，会把 A 的运行期规则覆盖成 B 的。
   */
  let rulesPushTimer: number | undefined
  async function pushRulesIfConnected() {
    const profile = selectedProfile.value
    if (!profile) return
    // 运行期的规则表属于「当前生效配置」，不是隧道拥有者。
    // 同通道切换后两者会不同，推错对象会把别人的规则顶上去。
    if (!isConnected.value || policyProfileId.value !== profile.id) return
    window.clearTimeout(rulesPushTimer)
    rulesPushTimer = window.setTimeout(async () => {
      try {
        await invoke('vpn_push_rules', {
          rules: profile.rules ?? [],
          defaultPolicy: profile.rules_default_policy ?? 'DIRECT',
        })
        pushLog('info', '规则已热更新，只重置受影响的连接')
      } catch (err: any) {
        pushLog('error', `规则未生效: ${err}`)
      }
    }, 300)
  }

  /**
   * 一次性保存整份规则表与兜底走向（规则弹窗关闭时调用）。
   *
   * 弹窗里所有改动都只在草稿上，关掉时才会走到这里：
   * 写配置一次，已连接且是当前生效配置时再热推送一次。
   */
  async function saveRules(rules: VpnRule[], defaultPolicy: VpnPolicy) {
    const profile = selectedProfile.value
    if (!profile) return
    profile.rules = rules.map((rule) => ({ ...rule }))
    profile.rules_default_policy = defaultPolicy
    await saveConfig()
    await pushRulesIfConnected()
  }

  async function selectProfile(id: string) {
    selectedProfileId.value = id
    config.value.vpn_active_profile_id = id
    await saveConfig()
    // 不同配置可能指定了不同的连接器路径，切换后重新探测
    await refreshConnectors()
  }

  /** 校验单条规则，返回规范化后的匹配值；失败时 reject，交由调用方展示 */
  async function validateRule(kind: VpnRuleKind, value: string): Promise<string> {
    return await invoke<string>('vpn_validate_rule', { kind, value })
  }

  /** 用给定规则集与一条输入预览命中结果 */
  async function previewRule(
    rules: VpnRule[],
    defaultPolicy: VpnPolicy,
    probe: { domain?: string; ip?: string; port?: number; process?: string },
  ): Promise<VpnPolicy> {
    const verdict = await invoke<string>('vpn_preview_rules', {
      rules,
      defaultPolicy,
      domain: probe.domain ?? null,
      ip: probe.ip ?? null,
      port: probe.port ?? null,
      process: probe.process ?? null,
    })
    return verdict as VpnPolicy
  }

  async function openSessionDir() {
    try {
      await invoke('vpn_open_session_dir')
    } catch (err: any) {
      pushLog('error', `打开目录失败: ${err}`)
    }
  }

  /**
   * 读回会话目录的实际位置。
   *
   * 会话目录首选 Roaming，但那里可能因受控文件夹访问或企业策略不可写，
   * 此时会回退到本地数据目录或临时目录。回退后用户按默认路径是找不到日志的，
   * 所以要把实际位置显示出来，而不是让他去猜。
   */
  async function loadSessionDir() {
    try {
      sessionDir.value = await invoke<SessionDirInfo>('vpn_session_dir_info')
    } catch (err: any) {
      sessionDir.value = null
    }
  }

  // ============================ 生命周期 ============================

  // 配置是异步从后端读回来的，useVpn 初始化时列表可能还是空的。
  // 这里用 watch 兜住：等列表到了再选中「上次用的」那条，否则首次进入会一个都选不中。
  watch(
    () => profiles.value.map((p) => p.id).join(','),
    () => {
      const current = selectedProfileId.value
      if (current && profiles.value.some((p) => p.id === current)) return
      const remembered = config.value.vpn_active_profile_id
      selectedProfileId.value = profiles.value.some((p) => p.id === remembered)
        ? remembered
        : (profiles.value[0]?.id ?? '')
    },
    { immediate: true },
  )

  // 连接器路径是配置级的，换配置或改了路径都要重新探测
  watch(
    () => selectedProfile.value?.connector_path ?? '',
    () => {
      refreshConnectors()
    },
  )

  onMounted(async () => {
    await refreshSnapshot()
    await refreshElevation()
    await refreshConnectors()
    await loadExistingLogs()
    await loadSessionDir()

    unlisteners.push(
      await listen<VpnSnapshotPayload>(EVENT_STATUS, (event) => {
        status.value = event.payload.status
        policyProfileId.value =
          event.payload.policy_profile_id || event.payload.status.profile_id || ''
        if (event.payload.elevation) {
          elevation.value = { ...(elevation.value ?? ({} as ElevationInfo)), hint: event.payload.elevation }
        }
      }),
    )
    unlisteners.push(
      await listen<string[]>(EVENT_LOG, (event) => {
        event.payload.forEach((line) => pushLog(guessLevel(line), line))
      }),
    )
  })

  onUnmounted(() => {
    unlisteners.forEach((unlisten) => unlisten())
    unlisteners.length = 0
  })

  // 用 reactive 包一层：这样在模板里写 vpn.status.phase 就能直接拿到值，
  // 不需要到处写 .value（ref 在 reactive 对象里会被自动解包）。
  return reactive({
    // 状态
    connectors,
    activeConnectors,
    bestConnector,
    noConnector,
    status,
    elevation,
    guide,
    logs,
    detecting,
    connecting,
    phaseBusy,
    isConnected,
    selectedProfileId,
    selectedProfile,
    proxyMode,
    policyProfileId,
    identities,
    sharesTunnel,
    switchToProfile,
    profiles,
    // 行为
    refreshConnectors,
    refreshElevation,
    refreshSnapshot,
    setupElevation,
    removeElevation,
    importOpenconnect,
    removeManagedOpenconnect,
    connect,
    disconnect,
    toggleConnection,
    clearLogs,
    openSessionDir,
    sessionDir,
    addProfile,
    updateProfile,
    deleteProfile,
    setProxyMode,
    saveRules,
    selectProfile,
    validateRule,
    previewRule,
    pushLog,
  })
}

/** VPN 面板与 App 之间共享的接口类型 */
export type VpnApi = ReturnType<typeof useVpn>
