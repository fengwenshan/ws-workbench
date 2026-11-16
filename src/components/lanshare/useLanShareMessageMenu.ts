import { ref, type ComputedRef, type Ref } from 'vue'
import { Modal, message } from 'ant-design-vue'
import { deleteLanShareMessage, type ChatMessage } from '@/api/lanshare'
import type { MergedPeer } from './types'
import { errorText } from './useLanShareActions'

/**
 * 消息右键删除（补充需求 14 / 设计 §4）。
 *
 * 和左栏 useLanShareListMenu 同一个模式：菜单状态与删除流程单独成文件，
 * useLanShareChat 里实例化后 ...spread 返回给面板。删除只影响本机聊天记录：
 * 本机已收到的文件不删；删到会话最后一条时后端会连会话行一起删，离线对端
 * 因此从左栏消失，所以这里刷新完会话要检查 activePeerId 还在不在列表里。
 */
export interface MessageMenuContext {
  mergedPeers: ComputedRef<MergedPeer[]>
  activePeerId: Ref<string | null>
  messages: Ref<ChatMessage[]>
  refreshConversations: () => Promise<void>
  refreshMessages: (peerId: string) => Promise<void>
}

/** 菜单渲染状态：右键命中的消息 + 鼠标位置（clientX / clientY） */
export interface MessageMenuState {
  message: ChatMessage
  x: number
  y: number
}

/** 本机收到的文件消息（有落盘路径才需要提示「文件不会删除」） */
function isReceivedFile(target: ChatMessage): boolean {
  return target.kind === 'file' && target.direction === 'in' && !!target.file_path
}

export function useLanShareMessageMenu(ctx: MessageMenuContext) {
  const { mergedPeers, activePeerId, messages, refreshConversations, refreshMessages } = ctx

  const messageMenu = ref<MessageMenuState | null>(null)
  let deleting = false

  function openMessageMenu(target: ChatMessage, x: number, y: number) {
    messageMenu.value = { message: target, x, y }
  }

  function closeMessageMenu() {
    messageMenu.value = null
  }

  function requestDeleteMessage() {
    const menu = messageMenu.value
    closeMessageMenu()
    if (!menu || deleting) return
    const target = menu.message
    // 文案按设计 §3.1：文件消息强调「本机已收到的文件不会删除」，文本消息只影响本机记录
    const content = isReceivedFile(target)
      ? '删除这条消息？只移除聊天记录，本机已收到的文件不会删除；对方设备不受影响。'
      : '删除这条消息？只影响本机聊天记录，对方设备不受影响。'
    Modal.confirm({
      title: '删除消息',
      content,
      okText: '删除',
      okType: 'danger',
      cancelText: '取消',
      onOk: async () => {
        if (deleting) return
        deleting = true
        try {
          await deleteLanShareMessage(target.peer_id, target.id)
          // 删的可能正好是当前会话：先刷新消息，再刷新会话（最后一条删掉后会话行会消失）
          if (activePeerId.value === target.peer_id) await refreshMessages(target.peer_id)
          await refreshConversations()
          // 离线对端的最后一条被删 → 会话行消失 → 清空选中回到空态（在线 peer 仍在列表里）
          if (
            activePeerId.value === target.peer_id &&
            !mergedPeers.value.some((item) => item.id === target.peer_id)
          ) {
            activePeerId.value = null
            messages.value = []
          }
          message.success('已删除')
        } catch (err) {
          // 失败时提示原因，消息保留（后端未改动）
          message.error(`删除消息失败：${errorText(err)}`)
        } finally {
          deleting = false
        }
      },
    })
  }

  return { messageMenu, openMessageMenu, closeMessageMenu, requestDeleteMessage }
}
