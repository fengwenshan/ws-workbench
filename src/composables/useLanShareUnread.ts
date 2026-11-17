import { onMounted, onUnmounted, ref } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { getLanShareUnreadTotal, onLanShareUnread } from '@/api/lanshare'

/**
 * LANLink 总未读数订阅（补充需求 15 / 设计 §2、§4）。
 *
 * 未读的唯一事实源是后端 SQLite 的 lan_share_conversations.unread：
 * - 首次挂载先拉一次 `lan_share_unread_total` 拿初始值；
 * - 之后订阅 `lan-share://unread`，聊天数据任何变化（新消息 / 已读 / 删除消息 /
 *   删会话 / 附件状态）后端都会推 `{ total }`，前端只维护这一个 ref。
 *
 * 主窗口 rail 和托盘 QuickPanel 各自实例化一份（两个 WebView 独立），
 * 都不再自己遍历会话列表求和；订阅失败只打 warn，不阻塞主界面。
 */
export function useLanShareUnread() {
  const unread = ref(0)
  let unlisten: UnlistenFn | null = null
  let disposed = false

  function applyTotal(total: number | null | undefined) {
    // 后端口径是 u32，这里再兜一层：负数 / 非数字都按 0
    const value = Number(total)
    unread.value = Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0
  }

  onMounted(async () => {
    // 先订阅再补初始值：避免「拉取完成 → 订阅建立」之间发生的未读变化丢失。
    // revision 用来丢弃过期的初始值：订阅后已经收到更新，就不再用旧快照覆盖。
    let revision = 0
    try {
      const stop = await onLanShareUnread((payload) => {
        revision += 1
        applyTotal(payload?.total)
      })
      if (disposed) {
        stop()
        return
      }
      unlisten = stop
    } catch (err) {
      console.warn('[lanshare] 订阅未读数失败', err)
    }

    if (disposed) return
    const beforeFetch = revision
    try {
      const total = await getLanShareUnreadTotal()
      if (revision === beforeFetch) applyTotal(total)
    } catch (err) {
      console.warn('[lanshare] 读取未读总数失败', err)
    }
  })

  onUnmounted(() => {
    disposed = true
    unlisten?.()
    unlisten = null
  })

  return { unread }
}
