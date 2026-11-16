<script setup lang="ts">
import LanShareSidebar from './LanShareSidebar.vue'
import LanChatPane from './LanChatPane.vue'
import LanShareSettings from './LanShareSettings.vue'
import LanDeleteChatDialog from './LanDeleteChatDialog.vue'
import LanPeerContextMenu from './LanPeerContextMenu.vue'
import { useLanShareChat } from './useLanShareChat'
import { useLanShareNotify } from './useLanShareNotify'

/**
 * 局域网分享页壳：左栏「设备列表」+ 右栏「聊天 / 设置」。
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
  enabled,
  statusText,
  statusClass,
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
} = useLanShareChat(() => props.active)

// 免打扰开关与设置页共享同一份单例状态（右键菜单里的勾选态）
const { dnd, toggleDnd } = useLanShareNotify()
</script>

<template>
  <div class="lan-share-chat">
    <aside class="lan-side">
      <LanShareSidebar
        :items="mergedPeers"
        :active-id="activePeerId"
        :settings-active="pane === 'settings'"
        :enabled="enabled"
        :loading="loading"
        :status-text="statusText"
        :status-class="statusClass"
        :now="now"
        @select="selectPeer"
        @context-menu="openPeerMenu"
        @open-settings="openSettings"
      />
    </aside>

    <section class="lan-main">
      <p v-if="loadError" class="lan-alert">{{ loadError }}</p>

      <LanShareSettings
        v-if="pane === 'settings'"
        :snapshot="snapshot"
        @close="pane = 'chat'"
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
/* 两栏壳：左栏固定 280px（窄窗口收窄），右栏吃掉剩余宽度；整体铺满 workspace-body */
.lan-share-chat {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
}
.lan-side {
  width: 280px;
  flex-shrink: 0;
  min-height: 0;
  display: flex;
  border-right: 1px solid var(--border-color);
  background: var(--bg-secondary);
}
.lan-main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
}
.lan-alert {
  margin: 8px 16px 0;
  padding: 8px 12px;
  border-radius: 6px;
  border: 1px solid rgba(239, 68, 68, 0.3);
  background: rgba(239, 68, 68, 0.06);
  color: var(--danger);
  font-size: 12px;
  line-height: 1.6;
  flex-shrink: 0;
}

@media (max-width: 760px) {
  .lan-side {
    width: 220px;
  }
}
</style>
