<script setup lang="ts">
import LanShareSidebar from './LanShareSidebar.vue'
import LanChatPane from './LanChatPane.vue'
import LanShareSettings from './LanShareSettings.vue'
import { useLanShareChat } from './useLanShareChat'

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
  deletingConversation,
  applySnapshot,
  selectPeer,
  openSettings,
  handleSendText,
  handleSendFiles,
  handleRetry,
  respondOffer,
  handleDeleteConversation,
} = useLanShareChat(() => props.active)
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
        @delete="handleDeleteConversation"
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
        :deleting="deletingConversation"
        :draft="draft"
        @update:draft="draft = $event"
        @send-text="handleSendText"
        @send-files="handleSendFiles"
        @retry="handleRetry"
        @accept-offer="respondOffer($event, true)"
        @reject-offer="respondOffer($event, false)"
        @delete-conversation="handleDeleteConversation()"
      />
    </section>
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
