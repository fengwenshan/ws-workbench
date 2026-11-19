<script setup lang="ts">
import PaneResizer from '@/components/PaneResizer.vue'
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
  /** 左侧设备列表是否收起（再点一次「LANLink」图标切换，和项目树 / VPN 列表一致） */
  menuCollapsed?: boolean
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

// 右键菜单里的「免打扰模式」勾选态（全应用共享同一份单例）
const { dnd, toggleDnd } = useLanShareNotify()

/** 齿轮：设置页没有「返回聊天」按钮了，已在该页时再点一次回到聊天 */
function toggleSettings() {
  if (pane.value === 'settings') pane.value = 'chat'
  else openSettings()
}
</script>

<template>
  <div class="lan-share-chat" :class="{ 'is-collapsed': menuCollapsed }">
    <aside v-show="!menuCollapsed" class="lan-side">
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

    <PaneResizer v-show="!menuCollapsed" />

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
/* 工作区本身就是外框：模块铺满工作区，只留两栏之间的间隙 */
.lan-share-chat {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 0;
  overflow: hidden;
}
/* 设备列表收起后不再占位，右栏自己补上和 rail 的 4px 间隙 */
.lan-share-chat.is-collapsed {
  gap: 0;
}
.lan-share-chat.is-collapsed .lan-main {
  margin-left: 4px;
}

/* 左栏：紧贴 rail 的独立卡片，顶部贴标题栏、下留 12px，整圈 1px 边框 + 12px 四角圆角 */
.lan-side {
  position: relative;
  width: var(--pane-left-width);
  flex-shrink: 0;
  min-height: 0;
  display: flex;
  margin-bottom: 8px;
  border: 1px solid var(--border-color);
  border-left: 0;
  border-radius: 0 var(--radius-lg) var(--radius-lg) 0;
  background: var(--bg-primary);
  overflow: hidden;
}

/* 右栏内容区：独立卡片，顶部贴住标题栏，右/下留 8px，整圈 1px 边框 + 12px 圆角 */
.lan-main {
  position: relative;
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  margin: 0 4px 8px 0;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  overflow: hidden;
}

.lan-alert {
  flex-shrink: 0;
  margin: 12px 16px 0;
  padding: 8px 12px;
  border-radius: var(--radius-md);
  background: var(--danger-soft);
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
