import { computed, type ComputedRef, type Ref } from 'vue'
import { Modal, message } from 'ant-design-vue'
import {
  deleteLanShareConversation,
  getLanShareSnapshot,
  respondLanShareOffer,
  sendLanShareChat,
  sendLanShareFiles,
  setLanSharePeerRemark,
  type ChatMessage,
  type LanShareSnapshot,
  type PendingOffer,
} from '@/api/lanshare'
import { pickFiles } from '@/utils/dialog'
import type { MergedPeer } from './types'

/**
 * 局域网分享的「写操作」：发文本 / 发文件 / 重试 / 接受拒绝 / 删除会话。
 *
 * 这些动作都要用壳里的状态（当前会话、消息列表、发送中标记），所以由
 * `useLanShareChat` 把 refs 和刷新函数传进来；单独成文件是为了让
 * 「事件 + 状态」和「用户动作」两块互不干扰，也避免单文件过长。
 */
export interface ChatActionContext {
  /** 当前选中的设备 id */
  activePeerId: Ref<string | null>
  /** 当前选中设备的展示信息（在线状态 / 别名） */
  activePeer: ComputedRef<MergedPeer | null>
  /** 左栏合并列表（删除前要确认存在历史会话） */
  mergedPeers: ComputedRef<MergedPeer[]>
  messages: Ref<ChatMessage[]>
  /** 输入框草稿（壳里持有） */
  draft: Ref<string>
  sendingText: Ref<boolean>
  sendingFiles: Ref<boolean>
  offerBusy: Ref<string | null>
  deletingConversation: Ref<boolean>
  applySnapshot: (next: LanShareSnapshot) => void
  refreshConversations: () => Promise<void>
  refreshMessages: (peerId: string) => Promise<void>
  scheduleConversationsRefresh: () => void
  upsertMessage: (next: ChatMessage) => void
}

/** 列表里的展示名：本机备注优先于设备名（与左栏 / 聊天头部一致） */
function displayName(peer: MergedPeer): string {
  return peer.remark || peer.alias
}

export function errorText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

export function createLanShareActions(ctx: ChatActionContext) {
  const {
    activePeerId,
    activePeer,
    mergedPeers,
    messages,
    draft,
    sendingText,
    sendingFiles,
    offerBusy,
    deletingConversation,
    applySnapshot,
    refreshConversations,
    refreshMessages,
    scheduleConversationsRefresh,
    upsertMessage,
  } = ctx

  async function handleSendText(text: string) {
    const peerId = activePeerId.value
    const target = activePeer.value
    if (!peerId || !target || sendingText.value) return
    const content = text.trim()
    if (!content) return
    // 与后端 MAX_CHAT_CHARS 一致：按 Unicode 字符数（emoji 算 1 个）而不是 UTF-16 长度
    if ([...content].length > 4000) {
      message.warning('消息过长，最多 4000 个字符')
      return
    }
    if (!target.online) {
      message.warning('设备已离线，无法发送')
      return
    }
    sendingText.value = true
    try {
      const sent = await sendLanShareChat(peerId, content)
      // 发送最长 5 秒，期间用户可能切到别的会话：只把消息插回它所属的会话
      if (activePeerId.value === peerId) upsertMessage(sent)
      // 发送期间输入框仍可继续起草：只有草稿还是刚发出去的内容时才清空
      if (draft.value.trim() === content) draft.value = ''
      scheduleConversationsRefresh()
    } catch (err) {
      message.error(`发送失败：${errorText(err)}`)
    } finally {
      sendingText.value = false
    }
  }

  async function handleSendFiles() {
    const target = activePeer.value
    if (!target || sendingFiles.value) return
    if (!target.online) {
      message.warning('设备已离线，无法发送文件')
      return
    }
    sendingFiles.value = true
    try {
      const paths = await pickFiles({ title: `发送文件给 ${displayName(target)}` })
      if (!paths.length) return
      await sendLanShareFiles(target.id, paths)
      message.success(`已开始向「${displayName(target)}」发送 ${paths.length} 个文件`)
      // 文件消息由后端先落库；chat 事件也会来，这里立即拉一次让气泡马上出现
      await refreshMessages(target.id)
    } catch (err) {
      message.error(`发送失败：${errorText(err)}`)
    } finally {
      sendingFiles.value = false
    }
  }

  /** 失败气泡重试：文本重新发一条；文件用源路径重发（没有路径就重新选） */
  async function handleRetry(msg: ChatMessage) {
    const peerId = msg.peer_id || activePeerId.value
    if (!peerId) return
    if (!activePeer.value?.online) {
      message.warning('设备已离线，无法重试')
      return
    }
    if (msg.kind === 'text') {
      if (sendingText.value) return
      sendingText.value = true
      try {
        const sent = await sendLanShareChat(peerId, msg.text)
        if (activePeerId.value === peerId) upsertMessage(sent)
        scheduleConversationsRefresh()
      } catch (err) {
        message.error(`重试失败：${errorText(err)}`)
      } finally {
        sendingText.value = false
      }
      return
    }
    if (!msg.file_path) {
      // 后端不记录发送源路径（重启后也可能不存在）：退回重新选文件
      void handleSendFiles()
      return
    }
    if (sendingFiles.value) return
    sendingFiles.value = true
    try {
      await sendLanShareFiles(peerId, [msg.file_path])
      await refreshMessages(peerId)
    } catch (err) {
      message.error(`重试失败：${errorText(err)}`)
    } finally {
      sendingFiles.value = false
    }
  }

  async function respondOffer(offer: PendingOffer, accept: boolean) {
    if (offerBusy.value) return
    offerBusy.value = offer.id
    try {
      await respondLanShareOffer(offer.id, accept)
      message.success(
        accept ? `已接受「${offer.sender_alias || '对方设备'}」的分享` : '已拒绝这次分享',
      )
      // 接受成功后后端落盘并写入 received 文件消息，事件之外再拉一次兜底
      try {
        applySnapshot(await getLanShareSnapshot())
      } catch (err) {
        console.warn('[lanshare] 刷新快照失败', err)
      }
      if (activePeerId.value === offer.sender_fingerprint) {
        await refreshMessages(offer.sender_fingerprint)
      }
    } catch (err) {
      message.error(`${accept ? '接受' : '拒绝'}失败：${errorText(err)}`)
      try {
        applySnapshot(await getLanShareSnapshot())
      } catch {
        /* 刷新失败就等下一次事件 */
      }
    } finally {
      offerBusy.value = null
    }
  }

  /**
   * 保存本机给某台设备起的备注（空串 = 删除并恢复设备名）。
   * 命令返回最新快照，apply 后左栏 / 聊天头部立即按备注显示。
   */
  async function handleSavePeerRemark(peerId: string, remark: string) {
    // 与后端一致：trim + 最多 32 个字符（emoji 按 1 个算）
    const text = [...remark.trim()].slice(0, 32).join('')
    try {
      applySnapshot(await setLanSharePeerRemark(peerId, text))
      message.success(text ? '备注已保存' : '已删除备注')
    } catch (err) {
      message.error(`保存备注失败：${errorText(err)}`)
    }
  }

  function handleDeleteConversation(peerId?: string) {
    const id = peerId ?? activePeerId.value
    if (!id || deletingConversation.value) return
    const target = mergedPeers.value.find((item) => item.id === id)
    if (!target?.hasConversation) return
    Modal.confirm({
      title: '删除会话',
      content: `将删除与「${displayName(target)}」的本机聊天记录（对方不受影响）。`,
      okText: '删除',
      okType: 'danger',
      cancelText: '取消',
      onOk: async () => {
        deletingConversation.value = true
        try {
          await deleteLanShareConversation(id)
          if (activePeerId.value === id) messages.value = []
          await refreshConversations()
          message.success('会话已删除')
        } catch (err) {
          message.error(`删除会话失败：${errorText(err)}`)
        } finally {
          deletingConversation.value = false
        }
      },
    })
  }

  return {
    handleSendText,
    handleSendFiles,
    handleRetry,
    respondOffer,
    handleDeleteConversation,
    handleSavePeerRemark,
  }
}
