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

/** 局域网内发现到的另一台设备 */
export interface LanPeer {
  fingerprint: string
  alias: string
  ip: string
  port: number
  device_type: string
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

/** 最近接收记录（只记元信息，路径指向下载目录里的真实文件） */
export interface ReceivedFile {
  file_name: string
  path: string
  size: number
  from_alias: string
  received_at_ms: number
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
  recent_received: ReceivedFile[]
  last_error: string | null
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

/** 清空最近接收记录（只清记录，不删文件） */
export function clearLanShareReceived(): Promise<void> {
  return invoke<void>('lan_share_clear_received')
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
