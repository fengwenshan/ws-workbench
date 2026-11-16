import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

/**
 * 局域网分享（LocalSend 风格）命令封装。
 *
 * 字段统一用后端的 snake_case（与设计文档 4.2 的 Rust 结构同名对齐），
 * 只有 invoke 的参数名按 Tauri 约定用 camelCase。
 */

/** 后端推送整份快照的事件（peer 变化 / 待接收变化 / 收到文件 / 启停 / 设置变更） */
export const EVENT_SNAPSHOT = 'lan-share://snapshot'
/** 发送端进度事件：每个分块 / 文件完成 / 失败 */
export const EVENT_SEND_PROGRESS = 'lan-share://send-progress'
/** 聊天数据变化事件：新消息 / 已读 / 会话被删除（payload 只带 peer_id/kind/direction，正文靠重新拉取） */
export const EVENT_CHAT = 'lan-share://chat'
/** 从托盘消息列表点某条会话：主窗口切到局域网分享并定位到该会话 */
export const EVENT_OPEN_CHAT = 'lan-share://open-chat'

/** 局域网内发现到的另一台设备 */
export interface LanPeer {
  fingerprint: string
  alias: string
  /** 当前选定、也确实连得上的地址（可能是 VPN / 其它网卡） */
  ip: string
  port: number
  device_type: string
  /** 对端自报的全部 IPv4，第一项是 announce 来源地址 */
  addresses: string[]
  /** 最后一次收到 announce 的毫秒时间戳 */
  last_seen_ms: number
}

/** 待接收请求里的一个文件 */
export interface OfferFile {
  id: string
  file_name: string
  size: number
}

/** 别人发来、等待本机确认的分享请求 */
export interface PendingOffer {
  id: string
  sender_alias: string
  sender_fingerprint: string
  sender_ip: string
  total_size: number
  files: OfferFile[]
  created_at_ms: number
  expires_in_ms: number
}

/** 发送进度（事件 payload） */
export interface SendProgress {
  transfer_id: string
  peer_alias: string
  file_name: string
  file_index: number
  file_total: number
  sent_bytes: number
  total_bytes: number
  done: boolean
  error: string | null
}

/** 页面初始化 / 手动刷新用的完整状态 */
export interface LanShareSnapshot {
  enabled: boolean
  running: boolean
  alias: string
  fingerprint: string
  port: number | null
  /** 实际生效的接收目录（空白时是默认目录） */
  download_dir: string
  download_dir_custom: boolean
  local_addresses: string[]
  peers: LanPeer[]
  pending_offers: PendingOffer[]
  /** 接收文件前是否需要本机确认（默认 false：直接接收落盘，不产生待接收请求） */
  require_accept: boolean
  /** 本机给其它设备起的备注：fingerprint -> 备注（只在本机显示，不广播给对端） */
  peer_remarks: Record<string, string>
  /** 致命错误：服务没起来，界面显示「启动失败」 */
  last_error: string | null
  /** 非致命问题：服务在跑，只是设置没能存住（重启后可能回退） */
  warning: string | null
}

/** 聊天方向：`in` 收到 / `out` 发出 */
export type ChatDirection = 'in' | 'out'
/** 聊天内容类型：文本 / 文件 */
export type ChatKind = 'text' | 'file'
/** 消息状态：发送中 / 已发送 / 发送失败 / 已保存（接收端落盘成功）/ 已删除（本机收到的文件被清掉） */
export type ChatStatus = 'sending' | 'sent' | 'failed' | 'received' | 'deleted'

/** 一条聊天消息（字段与后端 `ChatMessage` 同名，后端是 snake_case） */
export interface ChatMessage {
  id: string
  peer_id: string
  direction: ChatDirection
  kind: ChatKind
  /** 文本正文；文件消息为空串 */
  text: string
  file_name: string | null
  file_size: number | null
  /** 落盘路径：接收方是保存位置；发送方是本机源路径（失败重试用） */
  file_path: string | null
  status: ChatStatus
  error: string | null
  /** 文件消息与 `lan-share://send-progress` 事件对应 */
  transfer_id: string | null
  file_index: number | null
  file_total: number | null
  created_at_ms: number
}

/** 持久化的一个会话（后端已合并运行期 peer 的在线状态与最新别名） */
export interface LanShareConversation {
  id: string
  alias: string
  online: boolean
  last_activity_ms: number
  last_preview: string
  unread: number
  created_at_ms: number
}

/** `lan-share://chat` 事件 payload：kind 决定要不要额外刷新消息 */
export interface ChatEvent {
  peer_id: string
  kind: 'message' | 'read' | 'deleted' | 'updated'
  /** 新消息的方向（仅 kind='message' 时带）：用于「收到对方消息」时才响提示音 */
  direction?: 'in' | 'out' | null
}

/** `lan-share://open-chat` 事件 payload：托盘消息列表要定位的会话 */
export interface OpenChatEvent {
  peer_id: string
}

/** 拖拽进来的文件：内容先在前端转 base64 交给后端落盘发送（后端 serde camelCase） */
export interface DroppedFileInput {
  name: string
  dataBase64: string
}

/** 拉取当前快照（页面初始化 / 切回页面时刷新） */
export function getLanShareSnapshot(): Promise<LanShareSnapshot> {
  return invoke<LanShareSnapshot>('lan_share_snapshot')
}

/** 开启 / 关闭：开启会起 UDP 组播发现 + HTTP 接收服务，返回最新快照 */
export function setLanShareEnabled(enabled: boolean): Promise<LanShareSnapshot> {
  return invoke<LanShareSnapshot>('lan_share_set_enabled', { enabled })
}

/** 修改设备名（空串回退主机名），返回最新快照 */
export function setLanShareAlias(alias: string): Promise<LanShareSnapshot> {
  return invoke<LanShareSnapshot>('lan_share_set_alias', { alias })
}

/** 修改接收目录；传 null 回退默认目录，返回最新快照 */
export function setLanShareDownloadDir(dir: string | null): Promise<LanShareSnapshot> {
  return invoke<LanShareSnapshot>('lan_share_set_download_dir', { dir })
}

/** 接受 / 拒绝一条待接收请求 */
export function respondLanShareOffer(offerId: string, accept: boolean): Promise<void> {
  return invoke<void>('lan_share_respond_offer', { offerId, accept })
}

/** 选好文件后立即返回 transfer_id，实际发送在后台线程，进度走事件 */
export function sendLanShareFiles(peerFingerprint: string, paths: string[]): Promise<string> {
  return invoke<string>('lan_share_send_files', { peerFingerprint, paths })
}

/** 拉取会话列表（后端已按最近活动排好序，前端仍会再做一次合并排序） */
export function getLanShareConversations(): Promise<LanShareConversation[]> {
  return invoke<LanShareConversation[]>('lan_share_conversations')
}

/** 拉取某个会话最近的消息（时间正序） */
export function getLanShareMessages(peerId: string): Promise<ChatMessage[]> {
  return invoke<ChatMessage[]>('lan_share_messages', { peerId })
}

/**
 * 发送一条文本消息：后端先本地落库（status=sending）再后台投递，
 * 返回的消息可直接插进列表，最终的 sent / failed 由 `lan-share://chat` 事件刷新。
 */
export function sendLanShareChat(peerId: string, text: string): Promise<ChatMessage> {
  return invoke<ChatMessage>('lan_share_send_chat', { peerId, text })
}

/** 把某个会话的未读清零 */
export function markLanShareRead(peerId: string): Promise<void> {
  return invoke<void>('lan_share_mark_read', { peerId })
}

/**
 * 删除本机的某个会话及其消息（对方不受影响）。
 * `deleteFiles` 为 true 时同时把本机收到的文件移入废纸篓（只删 in 文件，不删我发出去的原文件）。
 */
export function deleteLanShareConversation(
  peerId: string,
  deleteFiles: boolean,
): Promise<void> {
  return invoke<void>('lan_share_delete_conversation', { peerId, deleteFiles })
}

/** 删除本机收到的文件（移入废纸篓），聊天记录保留；返回实际删除数量 */
export function deleteLanShareAttachments(peerId: string): Promise<number> {
  return invoke<number>('lan_share_delete_attachments', { peerId })
}

/** 查询会话里本机收到的文件数量（右键菜单「删除附件」是否可用 / 删除弹窗文案） */
export function getLanShareAttachmentCount(peerId: string): Promise<number> {
  return invoke<number>('lan_share_conversation_attachment_count', { peerId })
}

/**
 * 把系统剪贴板里的文件 / 图片导入到本机暂存目录，返回落盘路径。
 * 剪贴板里没有可发送内容时后端返回错误，前端提示即可。
 */
export function importLanShareClipboardFiles(): Promise<string[]> {
  return invoke<string[]>('lan_share_import_clipboard_files')
}

/** 发送拖拽进来的文件（base64 内容由后端先落盘到暂存目录再走 send_files） */
export function sendLanShareDroppedFiles(
  peerId: string,
  files: DroppedFileInput[],
): Promise<string> {
  return invoke<string>('lan_share_send_dropped_files', { peerId, files })
}

/**
 * 保存本机给某台设备起的备注（空串表示删除并恢复设备名），返回最新快照。
 * 备注只存在本机 lan_share.json 里，不会写进 announce / chat，对端看不到。
 */
export function setLanSharePeerRemark(
  peerId: string,
  remark: string,
): Promise<LanShareSnapshot> {
  return invoke<LanShareSnapshot>('lan_share_set_peer_remark', { peerId, remark })
}

/** 设置「接收文件前是否需要确认」（默认 false：直接接收），返回最新快照 */
export function setLanShareRequireAccept(requireAccept: boolean): Promise<LanShareSnapshot> {
  return invoke<LanShareSnapshot>('lan_share_set_require_accept', { requireAccept })
}

/** 打开文件所在目录（目录不存在时后端退到最近存在的上级） */
export function openLanShareFileDir(path: string): Promise<void> {
  return invoke<void>('lan_share_open_file_dir', { path })
}

/** 订阅整份快照事件，返回取消订阅函数 */
export function onLanShareSnapshot(
  handler: (snapshot: LanShareSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<LanShareSnapshot>(EVENT_SNAPSHOT, (event) => handler(event.payload))
}

/** 订阅发送进度事件，返回取消订阅函数 */
export function onLanShareSendProgress(
  handler: (progress: SendProgress) => void,
): Promise<UnlistenFn> {
  return listen<SendProgress>(EVENT_SEND_PROGRESS, (event) => handler(event.payload))
}

/** 订阅聊天变化事件，返回取消订阅函数 */
export function onLanShareChat(handler: (event: ChatEvent) => void): Promise<UnlistenFn> {
  return listen<ChatEvent>(EVENT_CHAT, (event) => handler(event.payload))
}

/** 订阅「托盘消息列表定位会话」事件，返回取消订阅函数 */
export function onLanShareOpenChat(handler: (event: OpenChatEvent) => void): Promise<UnlistenFn> {
  return listen<OpenChatEvent>(EVENT_OPEN_CHAT, (event) => handler(event.payload))
}
