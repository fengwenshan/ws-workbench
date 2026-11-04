<script setup lang="ts">
import { ref } from 'vue'
import { Tabs, TabPane } from 'ant-design-vue'
import ProjectConfigPanel from './ProjectConfig.vue'
import NoteEditor from './NoteEditor.vue'
import AttachmentPanel from './AttachmentPanel.vue'
import type { AttachmentMeta, LogLevel, ProjectConfig } from '@/types'

defineProps<{
  project: ProjectConfig
  defaultUploadDir: string | null
}>()

const configRef = ref<InstanceType<typeof ProjectConfigPanel> | null>(null)

const emit = defineEmits<{
  update: [id: string, data: Partial<ProjectConfig>]
  pack: [project: ProjectConfig, asZip: boolean]
  updateNote: [note: string]
  addAttachment: [meta: AttachmentMeta]
  removeAttachment: [id: string]
  renameAttachment: [id: string, name: string]
  openSettings: []
  log: [level: LogLevel, message: string]
}>()
</script>

<template>
  <div class="detail-panel">
    <Tabs class="detail-tabs">
      <template #rightExtra>
        <div v-if="configRef" class="header-actions">
          <span
            v-if="configRef.headerState.sourceMissing"
            class="type-badge type-missing"
            title="源目录已不存在：重新选择目录即可恢复，笔记与附件记录不会丢"
          >源目录失效</span>
          <!-- 按钮名就是识别出来的项目类型，点一下重新识别 -->
          <button
            class="btn-type"
            :class="'type-' + configRef.headerState.projectType"
            :disabled="configRef.headerState.detecting || !configRef.headerState.canDetect"
            :title="configRef.headerState.sourceMissing
              ? '源目录不存在，请先重新选择目录'
              : `当前识别为「${configRef.headerState.typeLabel}」，点击重新识别`"
            @click="configRef.refreshProjectType()"
          >{{ configRef.headerState.detecting ? '识别中…' : configRef.headerState.typeLabel }}</button>
          <template v-if="configRef.headerState.canPack">
            <button class="btn btn-secondary" @click="configRef.packZip()">打包 ZIP</button>
            <button class="btn btn-primary" @click="configRef.packFiles()">开始打包</button>
          </template>
        </div>
      </template>
      <TabPane key="note" tab="笔记">
        <div class="tab-pad">
          <NoteEditor
            :note="project.note"
            :default-upload-dir="defaultUploadDir"
            @update="(note) => emit('updateNote', note)"
            @log="(level, message) => emit('log', level, message)"
          />
        </div>
      </TabPane>
      <!-- 配置页 force-render：头部「识别 / 打包」依赖 configRef，
           不强制渲染的话默认停在笔记 tab 时头部按钮会不出来 -->
      <TabPane key="config" tab="配置" force-render>
        <ProjectConfigPanel
          ref="configRef"
          :project="project"
          @update="(id, data) => emit('update', id, data)"
          @pack="(p, asZip) => emit('pack', p, asZip)"
          @log="(level, message) => emit('log', level, message)"
        />
      </TabPane>
      <TabPane key="upload" tab="上传">
        <div class="tab-pad">
          <AttachmentPanel
            kind="project"
            :entity="project"
            :default-upload-dir="defaultUploadDir"
            @add-attachment="(meta) => emit('addAttachment', meta)"
            @remove-attachment="(id) => emit('removeAttachment', id)"
            @rename-attachment="(id, name) => emit('renameAttachment', id, name)"
            @open-settings="emit('openSettings')"
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
.type-badge,
.btn-type {
  font-size: 12px;
  padding: 3px 8px;
  border-radius: 4px;
  border: 1px solid var(--border-color);
  color: var(--text-secondary);
  background: transparent;
  white-space: nowrap;
}
/* 识别按钮：按钮名就是识别出来的项目类型，点击重新识别 */
.btn-type {
  font-family: inherit;
  cursor: pointer;
  transition: all 0.15s;
}
.btn-type:hover:not(:disabled) {
  filter: brightness(0.96);
}
.btn-type:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
/* 源目录失效：只做提示，记录与笔记/附件都还在 */
.type-missing {
  color: var(--danger);
  border-color: var(--danger);
  background: rgba(220, 38, 38, 0.06);
}
.type-layui {
  color: #16a34a;
  border-color: #16a34a;
  background: rgba(22, 163, 74, 0.08);
}
.type-vue2,
.type-vue3 {
  color: #42b883;
  border-color: #42b883;
  background: rgba(66, 184, 131, 0.08);
}
.type-react {
  color: #087ea4;
  border-color: #087ea4;
  background: rgba(8, 126, 164, 0.08);
}
.type-spring,
.type-springboot {
  color: #6db33f;
  border-color: #6db33f;
  background: rgba(109, 179, 63, 0.08);
}
.type-struts2 {
  color: #d22128;
  border-color: #d22128;
  background: rgba(210, 33, 40, 0.08);
}
.type-java {
  color: #e76f00;
  border-color: #e76f00;
  background: rgba(231, 111, 0, 0.08);
}
.type-unknown {
  color: var(--text-secondary);
  border-color: var(--border-color);
}
</style>
