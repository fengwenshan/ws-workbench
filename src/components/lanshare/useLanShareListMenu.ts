import { ref, type ComputedRef, type Ref } from 'vue'
import { Modal, message } from 'ant-design-vue'
import {
  deleteLanShareAttachments,
  deleteLanShareConversation,
  getLanShareAttachmentCount,
  type ChatMessage,
} from '@/api/lanshare'
import type { MergedPeer } from './types'
import { errorText } from './useLanShareActions'

/**
 * 左栏会话管理：右键菜单状态 + 两个删除流程（设计 §3.2 - §3.4）。
 *
 * 弹窗和菜单都是这个面板的临时 UI 状态，和 useLanShareChat 的「数据订阅」分开，
 * 单独成文件避免主 composable 继续膨胀。删除动作只影响本机：
 * - 删除聊天记录：可选同时把本机收到的文件移入废纸篓；
 * - 删除附件：保留聊天记录，只删本机收到的文件，文件消息变「已删除」。
 */
export interface ListMenuContext {
  mergedPeers: ComputedRef<MergedPeer[]>
  activePeerId: Ref<string | null>
  messages: Ref<ChatMessage[]>
  refreshConversations: () => Promise<void>
  refreshMessages: (peerId: string) => Promise<void>
}

/** 右键菜单渲染状态；attachmentCount 为 null 表示还在查询 */
export interface PeerMenuState {
  item: MergedPeer
  x: number
  y: number
  attachmentCount: number | null
  attachmentLoading: boolean
}

/** 删除聊天记录弹窗状态 */
export interface DeleteChatDialogState {
  peerId: string
  peerName: string
  attachmentCount: number
}

function displayName(peer: MergedPeer): string {
  return peer.remark || peer.alias
}

export function useLanShareListMenu(ctx: ListMenuContext) {
  const { mergedPeers, activePeerId, messages, refreshConversations, refreshMessages } = ctx

  const peerMenu = ref<PeerMenuState | null>(null)
  const deleteChatDialog = ref<DeleteChatDialogState | null>(null)
  const deleteChatBusy = ref(false)

  /** 菜单打开序号：异步数量回来后菜单已经换人 / 关闭就丢弃 */
  let menuSeq = 0

  async function openPeerMenu(item: MergedPeer, x: number, y: number) {
    const seq = ++menuSeq
    peerMenu.value = { item, x, y, attachmentCount: null, attachmentLoading: true }
    try {
      const count = await getLanShareAttachmentCount(item.id)
      if (seq !== menuSeq || peerMenu.value?.item.id !== item.id) return
      peerMenu.value = { ...peerMenu.value, attachmentCount: count, attachmentLoading: false }
    } catch {
      // 查询失败按「没有可删附件」处理：菜单里的删除附件保持置灰，不阻塞其它项
      if (seq !== menuSeq || peerMenu.value?.item.id !== item.id) return
      peerMenu.value = { ...peerMenu.value, attachmentCount: 0, attachmentLoading: false }
    }
  }

  function closePeerMenu() {
    menuSeq += 1
    peerMenu.value = null
  }

  /**
   * 打开删除聊天记录确认弹窗。
   * 入口有两个：右键菜单（带着已查到的数量）和聊天头部「删除会话」按钮（现查一次）。
   */
  async function requestDeleteChat(peerId?: string) {
    if (deleteChatBusy.value) return
    const menu = peerMenu.value
    const id = peerId ?? menu?.item.id ?? activePeerId.value
    if (!id) return
    const item = menu?.item.id === id ? menu.item : mergedPeers.value.find((peer) => peer.id === id)
    closePeerMenu()
    if (!item?.hasConversation) return
    let attachmentCount = menu?.item.id === id ? menu.attachmentCount : null
    if (attachmentCount == null) {
      try {
        attachmentCount = await getLanShareAttachmentCount(id)
      } catch {
        attachmentCount = 0
      }
    }
    deleteChatDialog.value = {
      peerId: id,
      peerName: displayName(item),
      attachmentCount,
    }
  }

  function cancelDeleteChat() {
    if (deleteChatBusy.value) return
    deleteChatDialog.value = null
  }

  /** 弹窗确认：deleteFiles = 是否同时删除本机收到的文件 */
  async function confirmDeleteChat(deleteFiles: boolean) {
    const dialog = deleteChatDialog.value
    if (!dialog || deleteChatBusy.value) return
    deleteChatBusy.value = true
    try {
      await deleteLanShareConversation(dialog.peerId, deleteFiles)
      if (activePeerId.value === dialog.peerId) messages.value = []
      await refreshConversations()
      message.success(deleteFiles ? '聊天记录和本机文件已删除' : '聊天记录已删除')
    } catch (err) {
      message.error(`删除聊天记录失败：${errorText(err)}`)
    } finally {
      deleteChatBusy.value = false
      deleteChatDialog.value = null
    }
  }

  /** 右键「删除附件」：只删本机收到的文件，聊天记录保留 */
  async function requestDeleteAttachments() {
    const menu = peerMenu.value
    closePeerMenu()
    if (!menu || !menu.item.hasConversation || menu.attachmentLoading) return
    const peerId = menu.item.id
    const count = menu.attachmentCount ?? 0
    if (count <= 0) {
      message.info('没有可删除的附件')
      return
    }
    Modal.confirm({
      title: '删除附件',
      content: `将删除本机收到的 ${count} 个文件（移入废纸篓），聊天记录保留；对方设备不受影响。`,
      okText: '删除',
      okType: 'danger',
      cancelText: '取消',
      onOk: async () => {
        try {
          const deleted = await deleteLanShareAttachments(peerId)
          // 后端已把文件消息改成 status=deleted、file_path=null，拉一次让文件卡变「已删除」
          if (activePeerId.value === peerId) await refreshMessages(peerId)
          await refreshConversations()
          message.success(deleted > 0 ? `已删除 ${deleted} 个文件` : '没有可删除的附件')
        } catch (err) {
          message.error(`删除附件失败：${errorText(err)}`)
        }
      },
    })
  }

  return {
    // 状态
    peerMenu,
    deleteChatDialog,
    deleteChatBusy,
    // 右键菜单
    openPeerMenu,
    closePeerMenu,
    // 删除聊天记录
    requestDeleteChat,
    confirmDeleteChat,
    cancelDeleteChat,
    // 删除附件
    requestDeleteAttachments,
  }
}
