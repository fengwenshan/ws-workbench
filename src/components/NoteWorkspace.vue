<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { PaperClipOutlined } from '@ant-design/icons-vue'
import NoteEditor from './NoteEditor.vue'
import NoteSidebar from './NoteSidebar.vue'
import type { LogLevel } from '@/types'
import { parseNoteAssets } from '@/utils/note-assets'

/**
 * 笔记工作区：编辑器 + 右侧「附件 / 大纲」栏。
 *
 * 数据流：
 * - 编辑器内部 500ms 防抖后 emit update，这里更新本地 markdown 并上抛给详情页
 * - 侧栏的增删改（重命名 / 删除资源）直接产出新正文，这里静默回写编辑器后上抛
 */
const props = defineProps<{
  note: string
  /** 设置里配置的全局上传目录（null 表示 App 私有目录） */
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  update: [note: string]
  log: [level: LogLevel, message: string]
}>()

const markdown = ref(props.note ?? '')
const sidebarCollapsed = ref(false)
const rootRef = ref<HTMLElement | null>(null)
const editorRef = ref<InstanceType<typeof NoteEditor> | null>(null)

// 外部（切换实体 / 配置回写）传入新笔记时同步本地与编辑器
watch(
  () => props.note,
  (value) => {
    const next = value ?? ''
    if (next === markdown.value) return
    markdown.value = next
    editorRef.value?.setMarkdown(next)
  },
)

const assetCount = computed(() => parseNoteAssets(markdown.value).length)

function onEditorUpdate(value: string) {
  markdown.value = value
  emit('update', value)
}

/** 输入过程中同步最新正文（不落盘），避免侧栏拿到 500ms 前的旧内容 */
function onEditorChange(value: string) {
  markdown.value = value
}

/** 侧栏改正文（重命名 alt / 删除资源）：同步编辑器并立即落盘，避免再次防抖 */
function onSidebarUpdate(next: string) {
  if (next === markdown.value) return
  markdown.value = next
  editorRef.value?.setMarkdown(next)
  emit('update', next)
}

function scrollToHeading(index: number) {
  editorRef.value?.scrollToHeading(index)
}

function onLog(level: LogLevel, message: string) {
  emit('log', level, message)
}

// 容器变窄时自动收起侧栏，保证编辑器可用宽度；变宽后由用户手动展开
let resizeObserver: ResizeObserver | null = null
onMounted(() => {
  if (!rootRef.value) return
  resizeObserver = new ResizeObserver((entries) => {
    const width = entries[0]?.contentRect.width ?? 0
    if (width > 0 && width < 760) sidebarCollapsed.value = true
  })
  resizeObserver.observe(rootRef.value)
})

onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  resizeObserver = null
})
</script>

<template>
  <div ref="rootRef" class="note-workspace">
    <div class="note-main">
      <NoteEditor
        ref="editorRef"
        :note="markdown"
        :default-upload-dir="defaultUploadDir"
        @update="onEditorUpdate"
        @change="onEditorChange"
        @log="onLog"
      />
      <!-- 侧栏收起后的入口：右下角悬浮按钮，显示当前资源数 -->
      <button
        v-if="sidebarCollapsed"
        class="sidebar-fab"
        title="展开笔记大纲 / 附件"
        @click="sidebarCollapsed = false"
      >
        <PaperClipOutlined />
        <span>大纲 / 附件</span>
        <span v-if="assetCount > 0" class="sidebar-fab-count">{{ assetCount }}</span>
      </button>
    </div>
    <NoteSidebar
      v-show="!sidebarCollapsed"
      class="note-sidebar"
      :markdown="markdown"
      @update-markdown="onSidebarUpdate"
      @scroll-to-heading="scrollToHeading"
      @collapse="sidebarCollapsed = true"
      @log="onLog"
    />
  </div>
</template>

<style scoped>
.note-workspace {
  display: flex;
  gap: 12px;
  height: 100%;
  min-height: 0;
}
.note-main {
  position: relative;
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  min-height: 0;
}
.note-sidebar {
  width: 236px;
  flex-shrink: 0;
  min-height: 0;
}
/* 收起侧栏后的悬浮入口 */
.sidebar-fab {
  position: absolute;
  right: 14px;
  bottom: 46px;
  z-index: 5;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 30px;
  padding: 0 12px;
  font-size: 12px;
  color: var(--text-secondary);
  background: var(--bg-primary);
  border: 1px solid var(--border-color);
  border-radius: 15px;
  box-shadow: 0 2px 8px rgba(15, 23, 42, 0.12);
  cursor: pointer;
  transition: all 0.15s;
}
.sidebar-fab:hover {
  color: var(--primary);
  border-color: var(--primary);
}
.sidebar-fab-count {
  min-width: 16px;
  padding: 0 5px;
  border-radius: 8px;
  font-size: 11px;
  line-height: 16px;
  color: #fff;
  background: var(--primary);
}
</style>
