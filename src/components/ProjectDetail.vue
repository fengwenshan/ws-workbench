<script setup lang="ts">
import { ref } from 'vue'
import { Tabs, TabPane } from 'ant-design-vue'
import ProjectConfigPanel from './ProjectConfig.vue'
import NoteEditor from './NoteEditor.vue'
import AttachmentPanel from './AttachmentPanel.vue'
import type { AttachmentMeta, LogLevel, ProjectConfig } from '@/types'

defineProps<{
  project: ProjectConfig
  defaultExclude: string[]
  defaultUploadDir: string | null
}>()

const configRef = ref<InstanceType<typeof ProjectConfigPanel> | null>(null)

const emit = defineEmits<{
  update: [id: string, data: Partial<ProjectConfig>]
  pack: [project: ProjectConfig, asZip: boolean]
  updateDefaultExclude: [rules: string[]]
  updateNote: [note: string]
  updateUploadDir: [dir: string | null]
  addAttachment: [meta: AttachmentMeta]
  removeAttachment: [id: string]
  renameAttachment: [id: string, name: string]
  log: [level: LogLevel, message: string]
}>()
</script>

<template>
  <div class="detail-panel">
    <Tabs class="detail-tabs">
      <template #rightExtra>
        <div v-if="configRef" class="header-actions">
          <span class="type-badge" :class="'type-' + configRef.headerState.projectType">
            {{ configRef.headerState.detecting ? '识别中…' : configRef.headerState.typeLabel }}
          </span>
          <button
            class="btn-refresh-type"
            :disabled="configRef.headerState.detecting || !configRef.headerState.canDetect"
            title="重新识别项目类型及版本控制地址"
            @click="configRef.refreshProjectType()"
          >{{ configRef.headerState.detecting ? '识别中…' : '识别' }}</button>
          <template v-if="configRef.headerState.canPack">
            <button class="btn btn-secondary" @click="configRef.packZip()">打包 ZIP</button>
            <button class="btn btn-primary" @click="configRef.packFiles()">开始打包</button>
          </template>
        </div>
      </template>
      <TabPane key="config" tab="项目配置">
        <ProjectConfigPanel
          ref="configRef"
          :project="project"
          :default-exclude="defaultExclude"
          @update="(id, data) => emit('update', id, data)"
          @pack="(p, asZip) => emit('pack', p, asZip)"
          @log="(level, message) => emit('log', level, message)"
          @update-default-exclude="(rules) => emit('updateDefaultExclude', rules)"
        />
      </TabPane>
      <TabPane key="note" tab="笔记">
        <div class="tab-pad">
          <NoteEditor
            :note="project.note"
            :upload-dir="project.upload_dir"
            :default-upload-dir="defaultUploadDir"
            @update="(note) => emit('updateNote', note)"
            @add-attachment="(meta) => emit('addAttachment', meta)"
            @log="(level, message) => emit('log', level, message)"
          />
        </div>
      </TabPane>
      <TabPane key="upload" tab="上传">
        <div class="tab-pad">
          <AttachmentPanel
            kind="project"
            :entity="project"
            :default-upload-dir="defaultUploadDir"
            @update-upload-dir="(dir) => emit('updateUploadDir', dir)"
            @add-attachment="(meta) => emit('addAttachment', meta)"
            @remove-attachment="(id) => emit('removeAttachment', id)"
            @rename-attachment="(id, name) => emit('renameAttachment', id, name)"
            @log="(level, message) => emit('log', level, message)"
          />
        </div>
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
.detail-tabs {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.detail-tabs :deep(.ant-tabs-nav) {
  margin-bottom: 12px;
  flex-shrink: 0;
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
.tab-pad {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding-bottom: 12px;
}
.header-actions {
  display: flex;
  gap: 8px;
  align-items: center;
}
.type-badge {
  font-size: 12px;
  padding: 3px 8px;
  border-radius: 4px;
  border: 1px solid var(--border-color);
  color: var(--text-secondary);
  white-space: nowrap;
}
.type-badge.type-layui {
  color: #16a34a;
  border-color: #16a34a;
  background: rgba(22, 163, 74, 0.08);
}
.type-badge.type-vue2,
.type-badge.type-vue3 {
  color: #42b883;
  border-color: #42b883;
  background: rgba(66, 184, 131, 0.08);
}
.type-badge.type-react {
  color: #087ea4;
  border-color: #087ea4;
  background: rgba(8, 126, 164, 0.08);
}
.type-badge.type-spring,
.type-badge.type-springboot {
  color: #6db33f;
  border-color: #6db33f;
  background: rgba(109, 179, 63, 0.08);
}
.type-badge.type-struts2 {
  color: #d22128;
  border-color: #d22128;
  background: rgba(210, 33, 40, 0.08);
}
.type-badge.type-java {
  color: #e76f00;
  border-color: #e76f00;
  background: rgba(231, 111, 0, 0.08);
}
.type-badge.type-unknown {
  color: var(--text-secondary);
  border-color: var(--border-color);
}
.btn-refresh-type {
  font-size: 12px;
  padding: 3px 10px;
  border-radius: 4px;
  border: 1px solid var(--border-color);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.15s;
}
.btn-refresh-type:hover:not(:disabled) {
  color: var(--primary);
  border-color: var(--primary);
}
.btn-refresh-type:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
