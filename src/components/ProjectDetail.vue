<script setup lang="ts">
import { ref } from 'vue'
import { Tabs, TabPane } from 'ant-design-vue'
import ProjectConfigPanel from './ProjectConfig.vue'
import NoteWorkspace from './NoteWorkspace.vue'
import AttachmentPanel from './AttachmentPanel.vue'
import OpenWithButton from './OpenWithButton.vue'
import type { AttachmentMeta, LogLevel, ProjectConfig } from '@/types'

defineProps<{
  project: ProjectConfig
  defaultUploadDir: string | null
  /** 全局默认打包规则：项目未单独配置时继承 */
  defaultInclude?: string[]
}>()

const configRef = ref<InstanceType<typeof ProjectConfigPanel> | null>(null)

const emit = defineEmits<{
  update: [id: string, data: Partial<ProjectConfig>]
  pack: [project: ProjectConfig, asZip: boolean]
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
          <!-- 源目录的「打开方式」放在标题栏：打包前先开项目目录，配置页那两行不再各放一个 -->
          <OpenWithButton
            compact
            hint="源目录"
            :path="configRef.headerState.sourceDir"
            @log="(level, message) => emit('log', level, message)"
          />
        </div>
      </template>
      <TabPane key="note" tab="笔记">
        <div class="tab-pad">
          <NoteWorkspace
            :note="project.note"
            :default-upload-dir="defaultUploadDir"
            @update="(note) => emit('updateNote', project.id, note)"
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
          :default-include="defaultInclude"
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
  padding: 0 20px;
}
.detail-tabs {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
/* 标题栏压到固定高度，和左侧「项目分组」的下边线同一水平线 */
.detail-tabs :deep(.ant-tabs-nav) {
  height: var(--panel-header-height);
  margin: 0 0 12px;
  flex-shrink: 0;
}
.detail-tabs :deep(.ant-tabs-nav-wrap),
.detail-tabs :deep(.ant-tabs-nav-list) {
  height: 100%;
}
.detail-tabs :deep(.ant-tabs-tab) {
  height: 100%;
  padding: 0;
  margin: 0 28px 0 0;
}
.detail-tabs :deep(.ant-tabs-ink-bar) {
  bottom: 0;
}
.detail-tabs :deep(.ant-tabs-extra-content) {
  display: flex;
  align-items: center;
  height: 100%;
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
  height: 100%;
}
/* 类型标签和右侧两个操作按钮统一成同一高度，避免标题栏里高矮不齐 */
.header-actions .btn,
.header-actions .btn-type,
.header-actions .type-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 26px;
  padding: 0 10px;
  font-size: 12px;
  line-height: 1;
  white-space: nowrap;
  box-sizing: border-box;
}
.type-badge,
.btn-type {
  font-size: 12px;
  padding: 4px 8px;
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
