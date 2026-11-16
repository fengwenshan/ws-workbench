import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { message } from 'ant-design-vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import {
  getLanShareConversations,
  getLanShareMessages,
  getLanShareSnapshot,
  markLanShareRead,
  onLanShareChat,
  onLanShareOpenChat,
  onLanShareSendProgress,
  onLanShareSnapshot,
  type ChatEvent,
  type ChatMessage,
  type LanPeer,
  type LanShareConversation,
  type LanShareSnapshot,
  type OpenChatEvent,
  type PendingOffer,
  type SendProgress,
} from '@/api/lanshare'
import { playMessageSound } from './useLanShareNotify'
import { useLanShareListMenu } from './useLanShareListMenu'
import { createLanShareActions, errorText } from './useLanShareActions'
import type { MergedPeer } from './types'

/**
 * 局域网分享的「状态 + 事件」编排，和组件壳（LanSharePanel）分开：
 * - 壳只负责两栏排版与把 props / 事件接上；
 * - 这里维护 snapshot / 会话 / 消息 / 进度并订阅后端事件，事件驱动刷新只有一处；
 * - 发送 / 重试 / 接受拒绝 / 删除这些写操作在 useLanShareActions 里，通过 ctx 拿状态。
 *
 * 面板是 v-show 常驻挂载的：监听在 onMounted 注册、onUnmounted 清理；页面隐藏时
 * 也继续收消息（未读数、列表排序不停），激活状态由 false 变 true 时补一次全量刷新。
 *
 * @param isActive 当前工具页是否激活（App.vue 的 view === 'lanshare'）
 */
export function useLanShareChat(isActive: () => boolean) {
  const snapshot = ref<LanShareSnapshot | null>(null)
  const conversations = ref<LanShareConversation[]>([])
  const messages = ref<ChatMessage[]>([])
  /** 发送进度：key = ${transfer_id}:${file_index}，文件消息渲染时按 key 取实时进度 */
  const progress = ref<Record<string, SendProgress>>({})

  /** 当前选中的设备 id（= peer.fingerprint = 会话 id）；null 时右栏显示空态 */
  const activePeerId = ref<string | null>(null)
  const pane = ref<'chat' | 'settings'>('chat')
  /** 输入框草稿放在壳里：发送成功才清空，失败时用户输入不丢 */
  const draft = ref('')

  const loading = ref(false)
  const loadError = ref('')

  const sendingText = ref(false)
  const sendingFiles = ref(false)
  const offerBusy = ref<string | null>(null)

  /** 1 秒一跳，给列表时间 / 待接收倒计时 / 在线判定提供刷新 */
  const now = ref(Date.now())

  /** 在线判定：心跳 3 秒一次，三跳没续上就把点标灰（后端 15 秒后才真正移除） */
  const PEER_ONLINE_WINDOW_MS = 10_000
  /** mark_read 与列表拉取可能并发：刚标记已读的会话在这个窗口内强制显示未读 0 */
  const READ_GRACE_MS = 2_000

  let unlisteners: UnlistenFn[] = []
  let tickTimer: ReturnType<typeof setInterval> | null = null
  let conversationsTimer: ReturnType<typeof setTimeout> | null = null
  let messagesTimer: ReturnType<typeof setTimeout> | null = null
  let disposed = false

  /** 列表 / 消息请求序号：慢响应回来时丢弃，避免覆盖新会话的数据 */
  let conversationsSeq = 0
  let messagesSeq = 0
  const recentlyReadAt = new Map<string, number>()

  const enabled = computed(() => snapshot.value?.enabled ?? false)
  const running = computed(() => snapshot.value?.running ?? false)
  const statusText = computed(() => {
    if (!snapshot.value) return '读取中…'
    if (snapshot.value.last_error) return '启动失败'
    if (running.value) return '运行中'
    if (enabled.value) return '启动中…'
    return '未开启'
  })
  const statusClass = computed(() => {
    // 还没读到快照时保持中性灰点，别一开始就红成「启动失败」
    if (!snapshot.value) return 'is-off'
    if (snapshot.value.last_error) return 'is-error'
    if (running.value) return 'is-on'
    if (enabled.value) return 'is-busy'
    return 'is-off'
  })

  /** 在线判定：last_seen_ms 在窗口内即绿点（与改造前 isPeerOnline 一致） */
  function isPeerOnline(peer: LanPeer): boolean {
    const delta = now.value - peer.last_seen_ms
    return !Number.isFinite(delta) || delta < PEER_ONLINE_WINDOW_MS
  }

  /**
   * 快照里的 peer 集合签名（id + 别名）。快照每秒都会推（last_seen 心跳），
   * 但只有设备增删 / 改名才需要重拉会话列表，否则等于每秒解密一遍会话预览。
   */
  function peerSignature(next: LanShareSnapshot | null): string {
    return (next?.peers ?? [])
      .map((peer) => `${peer.fingerprint}|${peer.alias}`)
      .sort()
      .join(';')
  }

  function applySnapshot(next: LanShareSnapshot) {
    snapshot.value = next
    loadError.value = ''
  }

  function handleSnapshot(next: LanShareSnapshot) {
    const changed = peerSignature(next) !== peerSignature(snapshot.value)
    applySnapshot(next)
    if (changed) scheduleConversationsRefresh()
  }

  /** 本机备注（只在本机生效）：有备注时优先于设备名展示 */
  function remarkFor(peerId: string): string {
    return snapshot.value?.peer_remarks?.[peerId]?.trim() ?? ''
  }

  /**
   * 左栏列表 = 会话 ∪ 在线 peer（按 id 去重，在线 peer 的最新别名优先）。
   * 排序：online 在前；组内按最近活动时间降序（会话用 last_activity_ms，
   * 仅在线无会话的 peer 用 last_seen_ms 参与排序）。
   */
  const mergedPeers = computed<MergedPeer[]>(() => {
    const byId = new Map<string, MergedPeer>()

    for (const conversation of conversations.value) {
      byId.set(conversation.id, {
        id: conversation.id,
        alias: conversation.alias?.trim() || '未知设备',
        remark: remarkFor(conversation.id),
        online: false,
        hasConversation: true,
        sortTimeMs: conversation.last_activity_ms,
        preview: conversation.last_preview,
        unread: conversation.unread,
        offerCount: 0,
        peer: null,
      })
    }

    for (const peer of snapshot.value?.peers ?? []) {
      const online = isPeerOnline(peer)
      const existing = byId.get(peer.fingerprint)
      if (existing) {
        existing.online = online
        existing.peer = peer
        // 对方改名后以 announce 的新别名为准；空别名不给覆盖
        const alias = peer.alias?.trim()
        if (alias) existing.alias = alias
        continue
      }
      // 离线且没有会话的设备不显示
      if (!online) continue
      byId.set(peer.fingerprint, {
        id: peer.fingerprint,
        alias: peer.alias?.trim() || '未知设备',
        remark: remarkFor(peer.fingerprint),
        online: true,
        hasConversation: false,
        sortTimeMs: peer.last_seen_ms,
        preview: '在线 · 发消息或文件',
        unread: 0,
        offerCount: 0,
        peer,
      })
    }

    // 待接收的文件请求也要在左栏可见：用户可能正在看别的会话，
    // 只靠右栏的待接收卡片会漏掉（列表项上加「待接收 N」提醒）。
    const offersBySender = new Map<string, number>()
    for (const offer of snapshot.value?.pending_offers ?? []) {
      offersBySender.set(offer.sender_fingerprint, (offersBySender.get(offer.sender_fingerprint) ?? 0) + 1)
    }
    if (offersBySender.size > 0) {
      for (const item of byId.values()) {
        const count = offersBySender.get(item.id) ?? 0
        if (count > 0) item.offerCount = count
      }
    }

    return [...byId.values()].sort((a, b) => {
      if (a.online !== b.online) return a.online ? -1 : 1
      return b.sortTimeMs - a.sortTimeMs
    })
  })

  const activePeer = computed<MergedPeer | null>(
    () => mergedPeers.value.find((item) => item.id === activePeerId.value) ?? null,
  )

  /** 当前会话相关的待接收请求（请求只按发送方匹配） */
  const activeOffers = computed<PendingOffer[]>(() =>
    (snapshot.value?.pending_offers ?? []).filter(
      (offer) => offer.sender_fingerprint === activePeerId.value,
    ),
  )

  function clearLocalUnread(peerId: string) {
    conversations.value = conversations.value.map((item) =>
      item.id === peerId ? { ...item, unread: 0 } : item,
    )
  }

  async function refreshConversations() {
    const seq = ++conversationsSeq
    try {
      const list = await getLanShareConversations()
      if (seq !== conversationsSeq) return
      const nowMs = Date.now()
      conversations.value = list.map((item) => {
        const readAt = recentlyReadAt.get(item.id)
        if (item.unread > 0 && readAt && nowMs - readAt < READ_GRACE_MS) {
          return { ...item, unread: 0 }
        }
        return item
      })
    } catch (err) {
      // 聊天存储不可用时快照里的 warning 会提示「聊天记录不可用」，这里不打断页面
      console.warn('[lanshare] 读取会话列表失败', err)
    }
  }

  async function refreshMessages(peerId: string) {
    const seq = ++messagesSeq
    try {
      const list = await getLanShareMessages(peerId)
      // 慢响应 / 已切到别的会话：丢弃
      if (seq !== messagesSeq || activePeerId.value !== peerId) return
      messages.value = list
    } catch (err) {
      console.warn('[lanshare] 读取聊天消息失败', err)
    }
  }

  async function refreshAll() {
    loading.value = true
    try {
      const [next] = await Promise.all([getLanShareSnapshot(), refreshConversations()])
      applySnapshot(next)
      if (activePeerId.value) await refreshMessages(activePeerId.value)
    } catch (err) {
      loadError.value = `读取局域网分享状态失败：${errorText(err)}`
    } finally {
      loading.value = false
    }
  }

  /** 聊天事件 → 会话列表防抖刷新（100ms 内的批量事件只拉一次） */
  function scheduleConversationsRefresh() {
    if (conversationsTimer) return
    conversationsTimer = setTimeout(() => {
      conversationsTimer = null
      void refreshConversations()
    }, 100)
  }

  /** 当前会话的消息防抖刷新；页面隐藏时不拉，等切回来由 refreshAll 兜底 */
  function scheduleMessagesRefresh() {
    const peerId = activePeerId.value
    if (!peerId) return
    if (messagesTimer) clearTimeout(messagesTimer)
    messagesTimer = setTimeout(() => {
      messagesTimer = null
      if (!isActive() || activePeerId.value !== peerId) return
      void refreshMessages(peerId)
    }, 100)
  }

  async function markRead(peerId: string) {
    // 先本地清零，避免角标在请求回来前闪一下
    recentlyReadAt.set(peerId, Date.now())
    clearLocalUnread(peerId)
    try {
      await markLanShareRead(peerId)
    } catch (err) {
      console.warn('[lanshare] 标记会话已读失败', err)
    }
  }

  /** 把 send_chat 返回的新消息插进列表（事件到达后会以服务端状态为准整体重拉） */
  function upsertMessage(next: ChatMessage) {
    const exists = messages.value.some((item) => item.id === next.id)
    const list = exists
      ? messages.value.map((item) => (item.id === next.id ? next : item))
      : [...messages.value, next]
    messages.value = list.sort((a, b) => a.created_at_ms - b.created_at_ms)
  }

  async function selectPeer(id: string) {
    pane.value = 'chat'
    if (activePeerId.value !== id) {
      activePeerId.value = id
      messages.value = []
      draft.value = ''
    }
    // 选中即已读；消息与已读并行请求
    await Promise.all([refreshMessages(id), markRead(id)])
  }

  function openSettings() {
    pane.value = 'settings'
  }

  const {
    handleSendText,
    handleSendFiles,
    handleSendPaths,
    handleSendDroppedFiles,
    handleRetry,
    respondOffer,
    handleSavePeerRemark,
  } = createLanShareActions({
    activePeerId,
    activePeer,
    messages,
    draft,
    sendingText,
    sendingFiles,
    offerBusy,
    applySnapshot,
    refreshConversations,
    refreshMessages,
    scheduleConversationsRefresh,
    upsertMessage,
  })

  // 左栏右键菜单 + 删除聊天记录 / 删除附件（弹窗与删除流程单独成文件维护）
  const listMenu = useLanShareListMenu({
    mergedPeers,
    activePeerId,
    messages,
    refreshConversations,
    refreshMessages,
  })

  function handleProgress(next: SendProgress) {
    const key = `${next.transfer_id}:${next.file_index}`
    const map = { ...progress.value, [key]: next }
    // 页面生命周期内只保留最近的进度；老记录对已完成的消息没有意义
    const keys = Object.keys(map)
    if (keys.length > 200) {
      for (const stale of keys.slice(0, keys.length - 200)) delete map[stale]
    }
    progress.value = map
    // 进度到终点后拉一次消息，让气泡状态从 sending 变成 sent / failed（事件缺失时兜底）
    if (next.done) scheduleMessagesRefresh()
  }

  function handleChatEvent(event: ChatEvent) {
    scheduleConversationsRefresh()
    // 只在「收到对方的新消息」时响提示音：自己发的、状态更新、已读、删除都不响；
    // 免打扰时 playMessageSound 内部直接跳过
    if (event.kind === 'message' && event.direction === 'in') playMessageSound()
    if (!isActive()) return
    if (event.peer_id !== activePeerId.value) return
    scheduleMessagesRefresh()
    // 只在聊天页可见时清未读：停在设置页说明用户还没看到新气泡
    if (event.kind !== 'deleted' && pane.value === 'chat') void markRead(event.peer_id)
  }

  /**
   * 托盘「消息」列表点了某条会话（设计 §7.4）：切页签、刷新后选中并标已读。
   * 设备理论上一定有会话；万一已被删掉，只提示不选中别的会话。
   */
  async function handleOpenChat(event: OpenChatEvent) {
    const peerId = event.peer_id
    if (!peerId) return
    pane.value = 'chat'
    await refreshAll()
    if (mergedPeers.value.some((item) => item.id === peerId)) {
      await selectPeer(peerId)
    } else {
      message.warning('该会话不在列表中，可能已被删除')
    }
  }

  onMounted(async () => {
    // 先注册事件再拉初始数据，缩小「启动瞬间漏事件」的窗口
    unlisteners.push(await onLanShareSnapshot(handleSnapshot))
    unlisteners.push(await onLanShareSendProgress(handleProgress))
    unlisteners.push(await onLanShareChat(handleChatEvent))
    unlisteners.push(await onLanShareOpenChat(handleOpenChat))

    if (disposed) {
      unlisteners.forEach((unlisten) => unlisten())
      unlisteners = []
      return
    }

    tickTimer = setInterval(() => {
      now.value = Date.now()
    }, 1000)

    await refreshAll()
  })

  onUnmounted(() => {
    disposed = true
    unlisteners.forEach((unlisten) => unlisten())
    unlisteners = []
    if (tickTimer) {
      clearInterval(tickTimer)
      tickTimer = null
    }
    if (conversationsTimer) {
      clearTimeout(conversationsTimer)
      conversationsTimer = null
    }
    if (messagesTimer) {
      clearTimeout(messagesTimer)
      messagesTimer = null
    }
  })

  // 页面由隐藏变可见：补一次全量刷新（事件在隐藏期间可能漏收）
  watch(isActive, (active) => {
    if (!active) return
    void refreshAll()
    if (activePeerId.value && pane.value === 'chat') void markRead(activePeerId.value)
  })

  // 选中的设备从合并列表消失（离线且无会话 / 会话被删后离线）：回到空态
  watch(mergedPeers, (items) => {
    if (!activePeerId.value) return
    if (items.some((item) => item.id === activePeerId.value)) return
    activePeerId.value = null
    messages.value = []
  })

  return {
    // 只读状态
    snapshot,
    conversations,
    messages,
    progress,
    activePeerId,
    activePeer,
    activeOffers,
    mergedPeers,
    pane,
    draft,
    loading,
    loadError,
    enabled,
    running,
    statusText,
    statusClass,
    now,
    sendingText,
    sendingFiles,
    offerBusy,
    // 动作
    applySnapshot,
    selectPeer,
    openSettings,
    handleSendText,
    handleSendFiles,
    handleSendPaths,
    handleSendDroppedFiles,
    handleRetry,
    respondOffer,
    handleSavePeerRemark,
    // 左栏右键菜单 + 删除聊天记录 / 删除附件
    ...listMenu,
  }
}
