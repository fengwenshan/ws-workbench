<script setup lang="ts">
import { Tabs, TabPane } from 'ant-design-vue'
import NoteWorkspace from './NoteWorkspace.vue'
import AttachmentPanel from './AttachmentPanel.vue'
import type { AttachmentMeta, LogLevel, ProjectGroup } from '@/types'

defineProps<{
  group: ProjectGroup
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  /** 带上实体 id：卸载补交的笔记不能落到切换后的新选中项上 */
  updateNote: [id: string, note: string]
  addAttachment: [meta: AttachmentMeta]
  removeAttachment: [id: string]
  renameAttachment: [id: string, name: string]
  openSettings: []
  log: [level: LogLevel, message: string]
}>()
</script>

<template>
  <div class="detail-panel">
    <div class="detail-title">
      <span class="detail-kind">分组</span>
      <span class="detail-name" :title="group.name">{{ group.name }}</span>
    </div>
    <Tabs class="detail-tabs">
      <TabPane key="note" tab="笔记">
        <NoteWorkspace
          :note="group.note"
          :default-upload-dir="defaultUploadDir"
          @update="(note) => emit('updateNote', group.id, note)"
          @log="(level, message) => emit('log', level, message)"
        />
      </TabPane>
      <TabPane key="upload" tab="上传">
        <AttachmentPanel
          kind="group"
          :entity="group"
          :default-upload-dir="defaultUploadDir"
          @add-attachment="(meta) => emit('addAttachment', meta)"
          @remove-attachment="(id) => emit('removeAttachment', id)"
          @rename-attachment="(id, name) => emit('renameAttachment', id, name)"
          @open-settings="emit('openSettings')"
          @log="(level, message) => emit('log', level, message)"
        />
      </TabPane>
    </Tabs>
  </div>
</template>

<style scoped>
.detail-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  padding: 12px 20px 0;
}
.detail-title {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  padding-bottom: 8px;
}
.detail-kind {
  font-size: 12px;
  color: var(--text-secondary);
  background: var(--bg-tag);
  border-radius: 4px;
  padding: 2px 8px;
}
.detail-name {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.detail-tabs {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.detail-tabs :deep(.ant-tabs-nav) {
  margin-bottom: 12px;
}
.detail-tabs :deep(.ant-tabs-content-holder) {
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.detail-tabs :deep(.ant-tabs-content) {
  height: 100%;
}
.detail-tabs :deep(.ant-tabs-tabpane) {
  height: 100%;
}
</style>
