<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Modal, message } from 'ant-design-vue'
import { formatSize, useHosts } from '@/composables/useHosts'
import { platform } from '@/platform'
import { createHostsEditor, type HostsEditorHandle } from '@/editor/monacoHosts'

/**
 * Hosts 在线编辑页签（系统页 -> Hosts）。
 *
 * 直接把系统 hosts 的完整内容读进一个等宽编辑器，不再解析成条目表：
 * 想加什么行就加什么行，`⌘/Ctrl+S` 保存整份文件（保存前自动备份 + 一次系统授权）。
 * 备份列表里可以还原；取消授权不会改动文件。
 *
 * 编辑区用 Monaco（VS Code 内核）：行号、语法着色、光标定位、撤销重做、查找替换都由它负责，
 * 不再手写「透明 textarea + 叠层高亮 + 自绘行号」那套（那个方案在文件以换行结尾、滚到最底部时
 * 两层排版会差一行，点哪都错一行；见 docs/superpowers/specs/2026-11-10-hosts-monaco-editor-design.md）。
 */
const props = defineProps<{
  /** 当前页签是否激活（CleanerPanel 的 tab === 'hosts' && 整个工具页可见） */
  active: boolean
}>()

const hosts = useHosts()

const editorMount = ref<HTMLDivElement | null>(null)
const editorLoading = ref(false)
const editorError = ref('')
const showBackups = ref(false)
let editor: HostsEditorHandle | null = null
/** 防止并发创建（watch 与 onMounted 都可能触发） */
let mounting = false
/** 组件是否已卸载：Monaco 是异步加载的，加载完可能已经卸载，别把编辑器挂到脱离文档的容器上 */
let unmounted = false

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
    // antd Tabs 未激活的页签是 display:none，切回来要重新量一次尺寸
    void nextTick(() => editor?.layout())
  },
  { immediate: true },
)

/** 容器渲染好（文件可读 + 页签激活）之后再创建编辑器 */
watch(
  [() => hosts.status?.readable, () => props.active],
  () => {
    // watch 回调（pre flush）早于 DOM 更新，等一帧再拿容器
    void nextTick(mountEditor)
  },
  { immediate: true },
)

async function mountEditor(): Promise<void> {
  if (editor || mounting) return
  if (!props.active || !hosts.status?.readable) return
  const container = editorMount.value
  if (!container) return
  mounting = true
  editorLoading.value = true
  editorError.value = ''
  try {
    const handle = await createHostsEditor(container, {
      value: hosts.draft,
      onChange: (value) => {
        hosts.draft = value
      },
      onSave: () => {
        void handleSave()
      },
    })
    if (unmounted) {
      handle.dispose()
      return
    }
    editor = handle
  } catch (error) {
    editorError.value = `编辑器加载失败：${error instanceof Error ? error.message : String(error)}`
  } finally {
    mounting = false
    editorLoading.value = false
  }
}

/** 外部改动（刷新 / 还原 / 首次加载）推进编辑器；用户自己打字时两边已经相等，不会回环 */
watch(
  () => hosts.draft,
  (draft) => {
    editor?.setValue(draft)
  },
)

watch(
  () => hosts.saving,
  (saving) => {
    editor?.setReadOnly(saving)
  },
)

async function handleSave(): Promise<void> {
  if (hosts.saving || !hosts.status?.readable) return
  // 以编辑器内容为准：刚敲完就按 ⌘S 时，draft 可能还差一次同步
  if (editor) {
    const current = editor.getValue()
    if (current !== hosts.draft) hosts.draft = current
  }
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

/** ⌘/Ctrl+S 全局保存（只在 Hosts 页签激活时生效；编辑器内有同一条命令） */
function handleWindowKeydown(event: KeyboardEvent): void {
  if (!props.active) return
  if (!(event.metaKey || event.ctrlKey) || event.altKey) return
  if (event.key.toLowerCase() !== 's') return
  event.preventDefault()
  void handleSave()
}

onMounted(() => {
  window.addEventListener('keydown', handleWindowKeydown)
  // immediate 的 watch 早于 DOM 挂载，这里再补一次
  void nextTick(mountEditor)
})

onUnmounted(() => {
  unmounted = true
  window.removeEventListener('keydown', handleWindowKeydown)
  editor?.dispose()
  editor = null
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
      <div ref="editorMount" class="hosts-monaco" />
      <div v-if="editorLoading" class="hosts-editor-status">正在加载编辑器…</div>
      <div v-else-if="editorError" class="hosts-editor-status is-error">
        <span>{{ editorError }}</span>
        <button class="btn btn-secondary" type="button" @click="mountEditor">重试</button>
      </div>
    </div>

    <div v-if="hosts.dirty" class="hosts-foot">
      <span class="hosts-foot-dirty">有未保存改动</span>
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

/* ===== 编辑器（Monaco） ===== */
.hosts-code {
  position: relative;
  flex: 1;
  min-height: 0;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  overflow: hidden;
}

.hosts-monaco {
  position: absolute;
  inset: 0;
}

.hosts-editor-status {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  background: var(--bg-primary);
  color: var(--text-muted);
  font-size: 13px;
}

.hosts-editor-status.is-error {
  color: var(--danger);
}

/* ===== 底部 ===== */
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
