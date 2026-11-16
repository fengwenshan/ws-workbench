import type { LanPeer } from '@/api/lanshare'

/**
 * 左侧列表项：持久化会话 ∪ 当前在线 peer 合并后的视图模型。
 *
 * 合并规则见设计文档 §2：
 * - 有会话的用会话的别名 / 摘要 / 最近活动时间；
 * - 同一台设备仍在线时，在线状态与最新别名以运行期 peer 为准；
 * - 从没聊过、但此刻在线的设备也进列表，摘要用固定提示语；
 * - 离线且没有会话的 peer 不显示（后端 15 秒后也会 prune）。
 */
export interface MergedPeer {
  /** 会话 id = peer 的 fingerprint（主网卡 MAC，取不到时为回退 UUID） */
  id: string
  /** 设备名：最新 announce 别名 / 会话别名 / 「未知设备」 */
  alias: string
  /** 本机备注（snapshot.peer_remarks[id]，空串表示没起备注）；展示时优先于 alias */
  remark: string
  online: boolean
  /** 有持久化会话：决定是否显示「删除会话」、空态文案等 */
  hasConversation: boolean
  /** 组内排序用的最近活动时间：有会话用 last_activity_ms，仅在线 peer 用 last_seen_ms */
  sortTimeMs: number
  /** 最后一条消息摘要；在线但无会话时是「在线 · 发消息或文件」 */
  preview: string
  unread: number
  /** 对方发来、还在等本机确认的文件请求数（>0 时左栏显示「待接收」提醒） */
  offerCount: number
  /** 运行期 peer 详情（IP:端口、地址等）；离线历史会话为 null */
  peer: LanPeer | null
}
