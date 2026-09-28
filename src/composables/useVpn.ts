import { ref, computed, onMounted, onUnmounted, reactive, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'
import type { AppConfig, LogEntry, LogLevel } from '@/types'
import type {
  ConnectorInfo,
  ConnectResult,
  ElevationInfo,
  InstallGuide,
  LearnReport,
  SessionDirInfo,
  VpnMode,
  VpnProfile,
  VpnRule,
  VpnRuleSet,
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
    connector: '',
    connector_name: '',
    mode: 'customer_first',
    tun_dev: '',
    tun_index: 0,
    vpn_ip: '',
    vpn_gateway: '',
    dns: [],
    mtu: 0,
    routes: [],
    snapshot: null,
    learn: null,
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
  /** 学习模式明细，只在用户展开面板时才拉（几十 KB，不进 status.json） */
  const learnReport = ref<LearnReport | null>(null)
  const learnLoading = ref(false)

  let logId = 0
  const unlisteners: UnlistenFn[] = []

  // ============================ 派生状态 ============================

  const profiles = computed(() => config.value.vpn_profiles ?? [])
  const ruleSets = computed(() => config.value.vpn_rule_sets ?? [])

  const selectedProfile = computed<VpnProfile | null>(() => {
    if (!selectedProfileId.value) return null
    return profiles.value.find((p) => p.id === selectedProfileId.value) ?? null
  })

  const selectedRuleSet = computed<VpnRuleSet | null>(() => {
    const profile = selectedProfile.value
    if (!profile?.rule_set_id) return null
    return ruleSets.value.find((r) => r.id === profile.rule_set_id) ?? null
  })

  const activeConnectors = computed(() => connectors.value.filter((c) => c.available))
  const bestConnector = computed<ConnectorInfo | null>(
    () => activeConnectors.value.find((c) => c.kind === 'openconnect') ?? null,
  )
  const noConnector = computed(() => activeConnectors.value.length === 0)
  const phaseBusy = computed(() => BUSY_PHASES.includes(status.value.phase))
  const isConnected = computed(() => status.value.phase === 'connected')
  /** 当前配置是否开了学习模式 */
  const learnEnabled = computed(() => selectedProfile.value?.learn ?? false)

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
    const picked = await open({ directory: true, multiple: false })
    if (!picked) return
    const message = await invoke<string>('vpn_import_openconnect', {
      sourceDir: picked as string,
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
      timestamp: withTime ? new Date().toLocaleTimeString('zh-CN') : '',
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
      const rules = selectedRuleSet.value?.rules ?? []
      const result = await invoke<ConnectResult>('vpn_connect', {
        profile: target,
        rules,
        customOpenconnect: target.connector_path || null,
      })
      pushLog(
        'info',
        `开始连接：连接器 ${result.connector_name}，提权方式 ${result.elevation}`,
      )
      await refreshElevation()
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

  async function toggleConnection() {
    if (isConnected.value || phaseBusy.value) {
      await disconnect()
    } else {
      await connect()
    }
  }

  // ============================ 配置增删改 ============================

  function newRuleSet(name: string, rules: VpnRule[] = []): VpnRuleSet {
    return {
      id: crypto.randomUUID(),
      name,
      rules,
      created_at: new Date().toISOString(),
    }
  }

  function newRule(cidr: string, remark = ''): VpnRule {
    return { id: crypto.randomUUID(), cidr, remark, enabled: true }
  }

  async function addProfile(name = '新建 VPN 配置', server = '') {
    // 顺手建一个空的规则集，省得用户还要再点一次
    const ruleSet = newRuleSet(`${name} 的公司网段`)
    config.value.vpn_rule_sets.push(ruleSet)

    const now = new Date().toISOString()
    const profile: VpnProfile = {
      id: crypto.randomUUID(),
      name,
      server,
      username: '',
      password: '',
      password_encrypted: false,
      protocol: 'anyconnect',
      connector: 'auto',
      mode: 'customer_first',
      rule_set_id: ruleSet.id,
      use_vpn_dns: false,
      learn: false,
      extra_args: [],
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
    Object.assign(profile, patch, { updated_at: new Date().toISOString() })
    await saveConfig()
  }

  /** 保存密码：明文→密文由后端完成，前端不保存明文 */
  async function savePassword(id: string, plain: string) {
    if (!plain) return
    const cipher = await invoke<string>('vpn_encrypt_password', { key: id, plain })
    await updateProfile(id, { password: cipher, password_encrypted: true })
  }

  async function deleteProfile(id: string) {
    const profile = profiles.value.find((p) => p.id === id)
    if (!profile) return
    config.value.vpn_profiles = profiles.value.filter((p) => p.id !== id)
    // 规则集如果没有别的配置在用，就一起删掉，避免留下孤儿数据
    const stillUsed = config.value.vpn_profiles.some(
      (p) => p.rule_set_id === profile.rule_set_id,
    )
    if (!stillUsed && profile.rule_set_id) {
      config.value.vpn_rule_sets = ruleSets.value.filter(
        (r) => r.id !== profile.rule_set_id,
      )
    }
    if (config.value.vpn_active_profile_id === id) {
      config.value.vpn_active_profile_id = config.value.vpn_profiles[0]?.id ?? ''
    }
    if (selectedProfileId.value === id) {
      selectedProfileId.value = config.value.vpn_profiles[0]?.id ?? ''
    }
    await saveConfig()
  }

  async function updateRuleSet(id: string, patch: Partial<VpnRuleSet>) {
    const ruleSet = ruleSets.value.find((r) => r.id === id)
    if (!ruleSet) return
    Object.assign(ruleSet, patch)
    await saveConfig()
  }

  async function addRule(ruleSetId: string, cidr: string, remark = '') {
    const ruleSet = ruleSets.value.find((r) => r.id === ruleSetId)
    if (!ruleSet) return
    ruleSet.rules.push(newRule(cidr, remark))
    await saveConfig()
  }

  async function updateRule(ruleSetId: string, ruleId: string, patch: Partial<VpnRule>) {
    const ruleSet = ruleSets.value.find((r) => r.id === ruleSetId)
    const rule = ruleSet?.rules.find((r) => r.id === ruleId)
    if (!rule) return
    Object.assign(rule, patch)
    await saveConfig()
  }

  async function deleteRule(ruleSetId: string, ruleId: string) {
    const ruleSet = ruleSets.value.find((r) => r.id === ruleSetId)
    if (!ruleSet) return
    ruleSet.rules = ruleSet.rules.filter((r) => r.id !== ruleId)
    await saveConfig()
  }

  async function selectProfile(id: string) {
    selectedProfileId.value = id
    config.value.vpn_active_profile_id = id
    await saveConfig()
    // 不同配置可能指定了不同的连接器路径，切换后重新探测
    await refreshConnectors()
  }

  async function setMode(mode: VpnMode) {
    const profile = selectedProfile.value
    if (!profile) return
    await updateProfile(profile.id, { mode })
  }

  /** 校验 CIDR 并顺手给出规范化结果 */
  async function validateCidr(cidr: string): Promise<string | null> {
    try {
      return await invoke<string>('vpn_validate_cidr', { cidr })
    } catch {
      return null
    }
  }

  async function checkOverlaps(cidrs: string[]): Promise<string[]> {
    try {
      return await invoke<string[]>('vpn_check_rule_overlaps', { cidrs })
    } catch {
      return []
    }
  }

  // ============================ 学习模式 ============================

  /** 开关学习模式。只写配置，开关本身不影响正在进行的连接 */
  async function setLearn(enabled: boolean) {
    const profile = selectedProfile.value
    if (!profile) return
    await updateProfile(profile.id, { learn: enabled })
    pushLog('info', enabled ? '已开启学习模式，下次连接开始采集' : '已关闭学习模式')
  }

  /** 拉取学习明细。明细单独存放在 learn.json，不会跟着状态事件推过来 */
  async function loadLearnReport() {
    learnLoading.value = true
    try {
      learnReport.value = await invoke<LearnReport | null>('vpn_read_learn_report')
    } catch {
      learnReport.value = null
    } finally {
      learnLoading.value = false
    }
  }

  /**
   * 清空采集结果。
   *
   * 后端只是放一个标志文件，真正清空由 worker 完成 —— 报告本体在 worker 内存里，
   * 主进程直接把 learn.json 删掉的话，下一轮采样会立刻把它写回来。
   */
  async function clearLearnReport() {
    try {
      await invoke('vpn_clear_learn_report')
      learnReport.value = null
      pushLog('info', '已清空学习采集结果')
    } catch (err: any) {
      pushLog('error', `清空学习结果失败: ${err}`)
    }
  }

  /**
   * 把一条候选网段采纳进当前规则集。
   *
   * 只动规则表，不碰其它配置；采纳动作本身也不会自动开连接。
   * 每一步都显式由用户点，避免自动往里塞网段之后没人知道流量被引到哪儿去了。
   */
  async function adoptLearnedCidr(cidr: string, remark = '') {
    const ruleSet = selectedRuleSet.value
    if (!ruleSet) {
      pushLog('warn', '这个配置还没有规则集，请先在分流规则里新建一个')
      return
    }
    if (ruleSet.rules.some((rule) => rule.cidr === cidr)) {
      pushLog('info', `${cidr} 已经在规则表里了`)
      return
    }
    ruleSet.rules.push(newRule(cidr, remark))
    await saveConfig()
    pushLog('success', `已把 ${cidr} 加入规则表`)
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
    learnEnabled,
    learnReport,
    learnLoading,
    selectedProfileId,
    selectedProfile,
    selectedRuleSet,
    profiles,
    ruleSets,
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
    savePassword,
    updateRuleSet,
    addRule,
    updateRule,
    deleteRule,
    selectProfile,
    setMode,
    validateCidr,
    checkOverlaps,
    setLearn,
    loadLearnReport,
    clearLearnReport,
    adoptLearnedCidr,
    pushLog,
  })
}

/** VPN 面板与 App 之间共享的接口类型 */
export type VpnApi = ReturnType<typeof useVpn>
