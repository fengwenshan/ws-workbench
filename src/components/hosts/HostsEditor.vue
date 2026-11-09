<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Modal, message } from 'ant-design-vue'
import { formatSize, useHosts } from '@/composables/useHosts'
import { platform } from '@/platform'
import { HOSTS_HIGHLIGHT_LIMIT, highlightHosts } from '@/utils/hostsHighlight'

/**
 * Hosts 在线编辑页签（系统页 -> Hosts）。
 *
 * 直接把系统 hosts 的完整内容读进一个等宽编辑器，不再解析成条目表：
 * 想加什么行就加什么行，`⌘/Ctrl+S` 保存整份文件（保存前自动备份 + 一次系统授权）。
 * 备份列表里可以还原；取消授权不会改动文件。
 */
const props = defineProps<{
  /** 当前页签是否激活（CleanerPanel 的 tab === 'hosts' && 整个工具页可见） */
  active: boolean
}>()

const hosts = useHosts()

const editorRef = ref<HTMLTextAreaElement | null>(null)
const gutterRef = ref<HTMLDivElement | null>(null)
const highlightRef = ref<HTMLPreElement | null>(null)
const showBackups = ref(false)
/** IME 组字中：先关掉叠层高亮，让 textarea 自己显示候选文本 */
const composing = ref(false)

/** 编辑器行号（1..n） */
const lineNumbers = computed(() =>
  Array.from({ length: hosts.lineCount }, (_, index) => index + 1),
)

/** 文件太大就退回纯文本：高亮要重排整份内容，没必要为几 MB 的异常文件卡住输入 */
const highlightEnabled = computed(() => hosts.size <= HOSTS_HIGHLIGHT_LIMIT)
/** 组字中或文件过大时关闭叠层，textarea 直接显示原文 */
const showHighlight = computed(() => highlightEnabled.value && !composing.value)
const highlighted = computed(() => (showHighlight.value ? highlightHosts(hosts.draft) : ''))

const saveShortcut = platform.is_macos ? '⌘S' : 'Ctrl+S'

/**
 * 首次进入页签才读文件；之后再进来只刷新系统状态 / 备份，不覆盖未保存的草稿。
 */
watch(
  () => props.active,
  (active) => {
    if (!active) return
    if (!hosts.loaded) {
      void hosts.load()
    } else if (!hosts.dirty) {
      void hosts.refreshStatus()
    }
  },
  { immediate: true },
)

/** 输入框里的 Tab 插入两个空格，避免焦点跳走（hosts 缩进习惯） */
function handleEditorKeydown(event: KeyboardEvent): void {
  if (event.key !== 'Tab' || event.metaKey || event.ctrlKey || event.altKey) return
  const element = editorRef.value
  if (!element) return
  event.preventDefault()
  const start = element.selectionStart
  const end = element.selectionEnd
  const indent = '  '
  hosts.draft = hosts.draft.slice(0, start) + indent + hosts.draft.slice(end)
  void nextTick(() => {
    element.selectionStart = start + indent.length
    element.selectionEnd = start + indent.length
  })
}

/** IME 组字结束：恢复高亮层，并把新 pre 的滚动位置对齐到光标处 */
function endCompose(): void {
  composing.value = false
  void nextTick(syncScroll)
}

// 叠层从无到有（IME 结束 / 大文件回退恢复）时，pre 的滚动位置要补一次
watch(showHighlight, () => {
  void nextTick(syncScroll)
})

function syncScroll(): void {
  const element = editorRef.value
  if (!element) return
  if (gutterRef.value) gutterRef.value.scrollTop = element.scrollTop
  // 高亮层在 textarea 下面，靠同一份 scrollTop/scrollLeft 对齐
  if (highlightRef.value) {
    highlightRef.value.scrollTop = element.scrollTop
    highlightRef.value.scrollLeft = element.scrollLeft
  }
}

async function handleSave(): Promise<void> {
  if (hosts.saving || !hosts.status?.readable) return
  if (!hosts.dirty) {
    message.info('没有需要保存的改动')
    return
  }
  const result = await hosts.save()
  if (!result) return
  if (result.saved) {
    message.success(result.message)
  } else {
    message.info(result.message)
  }
}

function handleRefresh(): void {
  if (hosts.loading) return
  if (hosts.dirty) {
    Modal.confirm({
      title: '刷新会丢弃未保存的改动',
      content: '当前编辑器里的内容还没有保存，刷新会从系统 hosts 重新读取并覆盖这些改动。继续？',
      okText: '丢弃并刷新',
      okType: 'danger',
      cancelText: '取消',
      onOk() {
        return hosts.load()
      },
    })
    return
  }
  void hosts.load()
}

function toggleBackups(): void {
  showBackups.value = !showBackups.value
  if (showBackups.value) void hosts.loadBackups()
}

function confirmRestore(id: string, createdAt: string): void {
  Modal.confirm({
    title: '从备份还原 hosts',
    content: `会用 ${createdAt} 的备份覆盖当前 hosts（还原前会再备份一次当前内容），过程中需要系统授权。继续？`,
    okText: '还原',
    okType: 'danger',
    cancelText: '取消',
    async onOk() {
      const result = await hosts.restore(id)
      if (result) message.success(result.message)
    },
  })
}

async function handleOpenDir(): Promise<void> {
  const ok = await hosts.openDir()
  if (!ok) message.error(hosts.error || '打开 hosts 目录失败')
}

/** ⌘/Ctrl+S 全局保存（只在 Hosts 页签激活时生效） */
function handleWindowKeydown(event: KeyboardEvent): void {
  if (!props.active) return
  if (!(event.metaKey || event.ctrlKey) || event.altKey) return
  if (event.key.toLowerCase() !== 's') return
  event.preventDefault()
  void handleSave()
}

onMounted(() => {
  window.addEventListener('keydown', handleWindowKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleWindowKeydown)
})
</script>

<template>
  <div class="hosts-editor">
    <!-- 操作栏 -->
    <div class="hosts-toolbar">
      <div class="hosts-hint">
        <template v-if="hosts.status?.hint">{{ hosts.status.hint }}</template>
        <template v-else>保存会弹一次系统授权框；取消授权不会改动文件。</template>
      </div>
      <div class="hosts-actions">
        <button class="btn btn-secondary" :disabled="hosts.loading" @click="handleRefresh">
          {{ hosts.loading ? '刷新中…' : '刷新' }}
        </button>
        <button
          class="btn btn-secondary"
          :class="{ active: showBackups }"
          :disabled="hosts.backupsLoading"
          @click="toggleBackups"
        >
          {{ showBackups ? '收起备份' : `备份 ${hosts.backupCount}` }}
        </button>
        <button
          class="btn btn-secondary"
          :title="hosts.status?.path || '打开 hosts 所在目录'"
          @click="handleOpenDir"
        >
          打开目录
        </button>
        <button
          class="btn btn-primary hosts-save"
          :disabled="hosts.saving || !hosts.status?.readable"
          @click="handleSave"
        >
          {{ hosts.saving ? '保存中…' : '保存' }}
          <kbd>{{ saveShortcut }}</kbd>
        </button>
      </div>
    </div>

    <!-- 错误条 -->
    <div v-if="hosts.error" class="hosts-error">
      <span>{{ hosts.error }}</span>
      <button type="button" title="关闭" @click="hosts.clearError">×</button>
    </div>

    <!-- 备份面板 -->
    <div v-if="showBackups" class="hosts-backups">
      <div class="backups-head">
        <span>最近备份 · 保留 30 份</span>
        <span v-if="hosts.lastBackupAt" class="backups-latest">最近 {{ hosts.lastBackupAt }}</span>
      </div>
      <div v-if="hosts.backups.length === 0" class="backups-empty">
        还没有备份。每次保存前会自动备份当前 hosts。
      </div>
      <div v-else class="backups-list">
        <div v-for="backup in hosts.backups" :key="backup.id" class="backup-row">
          <span class="backup-time">{{ backup.created_at }}</span>
          <span class="backup-size">{{ formatSize(backup.size) }}</span>
          <button
            class="backup-restore"
            type="button"
            :disabled="hosts.restoringId !== ''"
            @click="confirmRestore(backup.id, backup.created_at)"
          >
            {{ hosts.restoringId === backup.id ? '还原中…' : '还原' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 编辑器 -->
    <div v-if="hosts.loading && !hosts.status" class="hosts-empty">正在读取 hosts…</div>
    <div v-else-if="!hosts.status?.readable" class="hosts-empty">
      <p>{{ hosts.error || 'hosts 文件不可读，暂时不能在线编辑。' }}</p>
      <button class="btn btn-secondary" @click="handleRefresh">重试</button>
    </div>
    <div v-else class="hosts-code">
      <div ref="gutterRef" class="hosts-gutter" aria-hidden="true">
        <div class="hosts-gutter-inner">
          <span v-for="line in lineNumbers" :key="line">{{ line }}</span>
        </div>
      </div>
      <div class="hosts-editarea">
        <!-- 叠层高亮：pre 只负责着色，textarea 透明文字负责光标 / 选区 / 输入 -->
        <pre
          v-if="showHighlight"
          ref="highlightRef"
          class="hosts-highlight"
          aria-hidden="true"
        ><code v-html="highlighted" /></pre>
        <textarea
          ref="editorRef"
          v-model="hosts.draft"
          class="hosts-textarea"
          :class="{ 'is-plain': !showHighlight }"
          spellcheck="false"
          autocomplete="off"
          autocapitalize="off"
          autocorrect="off"
          wrap="off"
          placeholder="127.0.0.1 localhost"
          @keydown="handleEditorKeydown"
          @scroll="syncScroll"
          @compositionstart="composing = true"
          @compositionend="endCompose"
        />
      </div>
    </div>

    <div v-if="hosts.dirty || !highlightEnabled" class="hosts-foot">
      <span v-if="!highlightEnabled" class="hosts-foot-note">
        文件较大（{{ formatSize(hosts.size) }}），已关闭语法高亮
      </span>
      <span v-if="hosts.dirty" class="hosts-foot-dirty">有未保存改动</span>
    </div>
  </div>
</template>

<style scoped>
.hosts-editor {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 10px 20px 16px;
  overflow: hidden;
}

/* ===== 操作栏 ===== */
.hosts-toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.hosts-hint {
  min-width: 0;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.6;
}

.hosts-actions {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.hosts-actions .btn.active {
  border-color: var(--primary);
  color: var(--primary);
}

.hosts-save kbd {
  margin-left: 6px;
  padding: 0 4px;
  border: 1px solid currentColor;
  border-radius: 4px;
  font-size: 10px;
  font-family: inherit;
  opacity: 0.75;
}

/* ===== 错误条 ===== */
.hosts-error {
  flex-shrink: 0;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 12px;
  border: 1px solid rgba(239, 68, 68, 0.35);
  border-radius: 8px;
  background: rgba(239, 68, 68, 0.06);
  color: var(--danger);
  font-size: 12px;
  line-height: 1.6;
}

.hosts-error button {
  flex-shrink: 0;
  border: 0;
  background: transparent;
  color: var(--danger);
  font-size: 15px;
  line-height: 1;
  cursor: pointer;
}

/* ===== 备份面板 ===== */
.hosts-backups {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  overflow: hidden;
}

.backups-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 12px;
}

.backups-latest {
  color: var(--text-muted);
}

.backups-empty {
  padding: 14px 12px;
  color: var(--text-muted);
  font-size: 12px;
}

.backups-list {
  max-height: 150px;
  overflow-y: auto;
}

.backup-row {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 5px 12px;
  border-top: 1px solid var(--border-color);
  font-size: 12px;
}

.backup-time {
  color: var(--text-secondary);
  font-family: 'SF Mono', Menlo, Consolas, monospace;
}

.backup-size {
  flex: 1;
  color: var(--text-muted);
}

.backup-restore {
  border: 0;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  cursor: pointer;
}

.backup-restore:disabled {
  color: var(--text-muted);
  cursor: default;
}

/* ===== 编辑器 ===== */
.hosts-code {
  flex: 1;
  min-height: 0;
  display: flex;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  overflow: hidden;
}

.hosts-gutter {
  flex-shrink: 0;
  width: 46px;
  padding: 10px 8px 10px 0;
  overflow: hidden;
  background: var(--bg-secondary);
  border-right: 1px solid var(--border-color);
  color: var(--text-muted);
  text-align: right;
  user-select: none;
}

.hosts-gutter-inner {
  display: flex;
  flex-direction: column;
  font-size: 12px;
  line-height: 1.7;
  font-family: 'SF Mono', Menlo, Consolas, monospace;
}

.hosts-editarea {
  position: relative;
  flex: 1;
  min-width: 0;
}

/* 高亮层与输入层必须用完全相同的排版，否则文字会错位 */
.hosts-highlight,
.hosts-textarea {
  margin: 0;
  padding: 10px 12px;
  font-family: 'SF Mono', Menlo, Consolas, monospace;
  font-size: 12px;
  line-height: 1.7;
  letter-spacing: normal;
  tab-size: 2;
  white-space: pre;
}

.hosts-highlight {
  position: absolute;
  inset: 0;
  overflow: hidden;
  background: transparent;
  color: var(--text-primary);
  pointer-events: none;
  user-select: none;
}

.hosts-highlight code {
  display: block;
  font: inherit;
}

.hosts-textarea {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  border: 0;
  outline: none;
  resize: none;
  background: transparent;
  /* 文字交给下面的高亮层显示，textarea 只保留光标 / 选区 / 输入能力 */
  color: transparent;
  caret-color: var(--text-primary);
  overflow: auto;
}

/* 大文件 / IME 组字时回退成普通输入框 */
.hosts-textarea.is-plain {
  color: var(--text-primary);
}

.hosts-textarea::placeholder {
  color: var(--text-muted);
}

.hosts-textarea::selection {
  background: rgba(59, 130, 246, 0.25);
}

.hosts-textarea:focus {
  box-shadow: inset 0 0 0 1px rgba(59, 130, 246, 0.35);
}

/* ===== hosts 语法着色 ===== */
.hosts-editor :deep(.tok-comment) {
  color: var(--text-muted);
  font-style: italic;
}
.hosts-editor :deep(.tok-ip) {
  color: #2563eb;
  font-weight: 600;
}
.hosts-editor :deep(.tok-ip6) {
  color: #7c3aed;
  font-weight: 600;
}
.hosts-editor :deep(.tok-host) {
  color: #047857;
}
.hosts-editor :deep(.tok-invalid) {
  color: #dc2626;
  text-decoration: underline wavy rgba(220, 38, 38, 0.55);
}

.hosts-foot-note {
  color: #b45309;
}

.hosts-foot-dirty {
  flex-shrink: 0;
  color: #b45309;
}

.hosts-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  border: 1px dashed var(--border-color);
  border-radius: 8px;
  color: var(--text-muted);
  font-size: 13px;
}

.hosts-empty p {
  margin: 0;
  padding: 0 24px;
  text-align: center;
  line-height: 1.7;
}

.hosts-foot {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 12px;
  color: var(--text-muted);
  font-size: 11px;
}
</style>
