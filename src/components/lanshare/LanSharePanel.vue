<script setup lang="ts">
import LanShareSidebar from './LanShareSidebar.vue'
import LanChatPane from './LanChatPane.vue'
import LanShareSettings from './LanShareSettings.vue'
import LanDeleteChatDialog from './LanDeleteChatDialog.vue'
import LanPeerContextMenu from './LanPeerContextMenu.vue'
import LanMessageContextMenu from './LanMessageContextMenu.vue'
import { useLanShareChat } from './useLanShareChat'
import { useLanShareNotify } from './useLanShareNotify'

/**
 * LANLink 页壳：左栏「设备列表」+ 右栏「聊天 / 设置」（Glass Deck 视觉，刷新 §2）。
 *
 * 状态与事件编排都在 `useLanShareChat` 里，这里只负责布局和接线，
 * 保持组件文件只做展示层的事情。
 */
const props = defineProps<{
  /** 当前工具页是否激活（App.vue 的 view === 'lanshare'） */
  active: boolean
}>()

const {
  snapshot,
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
  now,
  sendingText,
  sendingFiles,
  offerBusy,
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
  peerMenu,
  openPeerMenu,
  closePeerMenu,
  requestDeleteChat,
  requestDeleteAttachments,
  deleteChatDialog,
  deleteChatBusy,
  confirmDeleteChat,
  cancelDeleteChat,
  messageMenu,
  openMessageMenu,
  closeMessageMenu,
  requestDeleteMessage,
} = useLanShareChat(() => props.active)

// 免打扰开关与设置页共享同一份单例状态（右键菜单里的勾选态）
const { dnd, toggleDnd } = useLanShareNotify()

/** 齿轮：设置页没有「返回聊天」按钮了，已在该页时再点一次回到聊天 */
function toggleSettings() {
  if (pane.value === 'settings') pane.value = 'chat'
  else openSettings()
}
</script>

<template>
  <div class="lan-share-chat">
    <aside class="lan-side">
      <LanShareSidebar
        :items="mergedPeers"
        :active-id="activePeerId"
        :settings-active="pane === 'settings'"
        :loading="loading"
        :now="now"
        @select="selectPeer"
        @context-menu="openPeerMenu"
        @open-settings="toggleSettings"
      />
    </aside>

    <section class="lan-main">
      <p v-if="loadError" class="lan-alert">{{ loadError }}</p>

      <LanShareSettings
        v-if="pane === 'settings'"
        :snapshot="snapshot"
        @updated="applySnapshot"
      />
      <LanChatPane
        v-else
        :peer="activePeer"
        :messages="messages"
        :offers="activeOffers"
        :progress="progress"
        :now="now"
        :sending="sendingText"
        :sending-files="sendingFiles"
        :offer-busy="offerBusy"
        :deleting="deleteChatBusy"
        :draft="draft"
        @update:draft="draft = $event"
        @send-text="handleSendText"
        @send-files="handleSendFiles"
        @send-paths="handleSendPaths"
        @send-dropped-files="handleSendDroppedFiles"
        @retry="handleRetry"
        @accept-offer="respondOffer($event, true)"
        @reject-offer="respondOffer($event, false)"
        @delete-conversation="requestDeleteChat()"
        @message-menu="openMessageMenu"
        @save-remark="handleSavePeerRemark"
      />
    </section>

    <!-- 左栏右键菜单：删除聊天记录 / 删除附件 / 免打扰 -->
    <LanPeerContextMenu
      v-if="peerMenu"
      :item="peerMenu.item"
      :x="peerMenu.x"
      :y="peerMenu.y"
      :attachment-count="peerMenu.attachmentCount"
      :attachment-loading="peerMenu.attachmentLoading"
      :dnd="dnd"
      @delete-chat="requestDeleteChat()"
      @delete-attachments="requestDeleteAttachments"
      @toggle-dnd="toggleDnd"
      @close="closePeerMenu"
    />

    <!-- 消息右键菜单：只有「删除消息」（危险色） -->
    <LanMessageContextMenu
      v-if="messageMenu"
      :message="messageMenu.message"
      :x="messageMenu.x"
      :y="messageMenu.y"
      @delete="requestDeleteMessage"
      @close="closeMessageMenu"
    />

    <!-- 删除聊天记录二次确认：有本机收到的文件时才出现「同时删除文件」复选框 -->
    <LanDeleteChatDialog
      :open="!!deleteChatDialog"
      :peer-name="deleteChatDialog?.peerName ?? ''"
      :attachment-count="deleteChatDialog?.attachmentCount ?? 0"
      :busy="deleteChatBusy"
      @confirm="confirmDeleteChat"
      @cancel="cancelDeleteChat"
    />
  </div>
</template>

<style scoped>
/* 模块画布：浅色渐变 + 左上光晕；整体圆角 16 浮在应用底色上 */
.lan-share-chat {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 12px;
  margin: 10px 12px 12px;
  padding: 12px;
  border-radius: 16px;
  overflow: hidden;
  background: linear-gradient(160deg, #f3f6fc 0%, #eef2f9 55%, #edf1f8 100%);
  box-shadow: 0 1px 2px rgba(15, 23, 42, 0.04);
}
.lan-share-chat::before {
  content: '';
  position: absolute;
  inset: 0;
  background: radial-gradient(60% 55% at 8% 0%, rgba(99, 102, 241, 0.1), transparent 70%);
  pointer-events: none;
}

/* 左栏：半透明玻璃表面，不用右边框，与右栏靠间距和阴影分层 */
.lan-side {
  position: relative;
  width: 300px;
  flex-shrink: 0;
  min-height: 0;
  display: flex;
  border-radius: 16px;
  background: rgba(255, 255, 255, 0.55);
  backdrop-filter: blur(18px);
  -webkit-backdrop-filter: blur(18px);
  overflow: hidden;
}

/* 右栏：近白表面 + 中等阴影，像浮起来的对话卡 */
.lan-main {
  position: relative;
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  border-radius: 16px;
  background: #fbfcfe;
  box-shadow: 0 10px 30px -18px rgba(15, 23, 42, 0.28);
  overflow: hidden;
}

.lan-alert {
  flex-shrink: 0;
  margin: 10px 16px 0;
  padding: 8px 12px;
  border-radius: 10px;
  background: rgba(239, 68, 68, 0.08);
  color: var(--danger);
  font-size: 12px;
  line-height: 1.6;
}

@media (max-width: 900px) {
  .lan-side {
    width: 240px;
  }
}
</style>
