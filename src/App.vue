<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { useConfig } from '@/composables/useConfig'
import { usePack } from '@/composables/usePack'
import { useUpdater } from '@/composables/useUpdater'
import ProjectList from '@/components/ProjectList.vue'
import ProjectConfig from '@/components/ProjectConfig.vue'
import PackProgress from '@/components/PackProgress.vue'
import LogPanel from '@/components/LogPanel.vue'

const {
  config,
  selectedProjectId,
  loadConfig,
  saveConfig,
  addGroup,
  deleteGroup,
  renameGroup,
  reorderGroups,
  reorderProjects,
  moveProject,
  addProject,
  updateProject,
  deleteProject,
  findProject,
} = useConfig()

const {
  packing,
  progress,
  result,
  logs,
  addLog,
  clearLogs,
  pack,
} = usePack()

const {
  status: updateStatus,
  info: updateInfo,
  errorMessage: updateError,
  percentage: updatePercentage,
  hasUpdatePrompt,
  checkUpdate,
  installUpdate,
  restartApp,
  startDailyCheck,
  stopDailyCheck,
  dismissCurrent,
} = useUpdater()

// 每天 10:00 / 15:00 自动校验更新
onMounted(startDailyCheck)
onUnmounted(stopDailyCheck)

const updateBusy = computed(() =>
  updateStatus.value === 'checking' || updateStatus.value === 'downloading'
)

const updateButtonLabel = computed(() => {
  switch (updateStatus.value) {
    case 'checking': return '检查中…'
    case 'downloading': return `下载中 ${Math.floor(updatePercentage.value)}%`
    case 'available': return `更新到 v${updateInfo.value?.version}`
    case 'up-to-date': return '已是最新版本'
    case 'installed': return '重启生效'
    case 'error': return '重试'
    default: return '检查更新'
  }
})

async function handleUpdateAction() {
  if (updateStatus.value === 'available') {
    addLog('info', `开始下载 v${updateInfo.value?.version} …`)
    const ok = await installUpdate()
    if (ok) {
      addLog('success', `v${updateInfo.value?.version} 已安装，重启后生效`)
    } else {
      addLog('error', `更新失败: ${updateError.value}`)
    }
    return
  }

  if (updateStatus.value === 'installed') {
    await restartApp()
    return
  }

  const found = await checkUpdate()
  if (found) {
    addLog('info', `发现新版本 v${found.version}（当前 v${found.currentVersion}）`)
  } else if (updateStatus.value === 'up-to-date') {
    addLog('info', '当前已是最新版本')
  } else {
    addLog('error', `检查更新失败: ${updateError.value}`)
  }
}

function handleDismissUpdate() {
  const version = updateInfo.value?.version
  dismissCurrent()
  addLog('info', `已忽略 v${version}，本次不再提示`)
}

const selectedProject = computed(() => {
  if (!selectedProjectId.value) return null
  return findProject(selectedProjectId.value) || null
})

// 新建分组
const showAddGroupForm = ref(false)
const newGroupName = ref('')

async function handleAddGroup() {
  if (!newGroupName.value.trim()) {
    addLog('warn', '请填写分组名称')
    return
  }
  try {
    await addGroup(newGroupName.value.trim())
    addLog('success', `已创建分组: ${newGroupName.value}`)
    newGroupName.value = ''
    showAddGroupForm.value = false
  } catch (err: any) {
    addLog('error', `创建分组失败: ${err}`)
  }
}

// 确认弹窗
const showConfirmDialog = ref(false)
const confirmTitle = ref('')
const confirmMessage = ref('')
let confirmAction: (() => void) | null = null

function openConfirm(title: string, message: string, onConfirm: () => void) {
  confirmTitle.value = title
  confirmMessage.value = message
  confirmAction = onConfirm
  showConfirmDialog.value = true
}

function handleConfirmOk() {
  if (confirmAction) {
    confirmAction()
    confirmAction = null
  }
  showConfirmDialog.value = false
}

function handleConfirmCancel() {
  confirmAction = null
  showConfirmDialog.value = false
}

function handleDeleteGroup(id: string) {
  const group = config.value.groups.find(g => g.id === id)
  if (!group) return
  // 默认分组不允许删除（内部项目仍可单独删除）
  if (group.is_default) {
    addLog('warn', `默认分组「${group.name}」不可删除`)
    return
  }
  const projectCount = group.projects.length
  const suffix = projectCount > 0 ? `（含 ${projectCount} 个项目）` : ''
  openConfirm(
    '删除分组',
    `确定要删除分组「${group.name}」${suffix}吗？此操作不可恢复。`,
    async () => {
      await deleteGroup(id)
      addLog('info', `已删除分组: ${group.name}`)
    }
  )
}

// 重命名分组（默认分组不允许重命名）
async function handleRenameGroup(id: string, name: string) {
  const group = config.value.groups.find(g => g.id === id)
  if (!group) return
  if (group.is_default) {
    addLog('warn', `默认分组「${group.name}」不可重命名`)
    return
  }
  const oldName = group.name
  try {
    await renameGroup(id, name)
    addLog('info', `分组已重命名: ${oldName} → ${name}`)
  } catch (err: any) {
    addLog('error', `重命名分组失败: ${err}`)
  }
}

// 添加项目（在指定分组中）
const showAddForm = ref(false)
const targetGroupId = ref<string | null>(null)
const newProjectName = ref('')
const newProjectSource = ref('')
const newProjectOutput = ref('')

// 从路径中提取最后一层目录名
function getLastDirName(path: string): string {
  const trimmed = path.replace(/\/+$/, '')
  const parts = trimmed.split('/')
  return parts[parts.length - 1] || ''
}

async function selectNewSource() {
  const selected = await open({ directory: true, multiple: false })
  if (selected) {
    const sourceDir = selected as string
    newProjectSource.value = sourceDir
    // 源目录改变时，强制更新项目名称和输出目录
    newProjectName.value = getLastDirName(sourceDir)
    newProjectOutput.value = sourceDir.replace(/\/+$/, '') + '/dist'
  }
}

async function selectNewOutput() {
  const selected = await open({ directory: true, multiple: false })
  if (selected) newProjectOutput.value = selected as string
}

function openAddForm(groupId: string) {
  targetGroupId.value = groupId
  newProjectName.value = ''
  newProjectSource.value = ''
  newProjectOutput.value = ''
  showAddForm.value = true
}

async function handleAdd() {
  if (!targetGroupId.value) {
    addLog('warn', '未选择分组')
    return
  }
  if (!newProjectSource.value.trim()) {
    addLog('warn', '请选择源目录')
    return
  }

  const sourceDir = newProjectSource.value.trim()
  // 项目名称为空时，自动用源目录最后一层名字
  const projectName = newProjectName.value.trim() || getLastDirName(sourceDir)
  // 输出目录为空时，自动追加 /dist
  const outputDir = newProjectOutput.value.trim() || (sourceDir.replace(/\/+$/, '') + '/dist')

  if (!projectName) {
    addLog('warn', '请填写项目名称')
    return
  }

  await addProject(targetGroupId.value, {
    name: projectName,
    source_dir: sourceDir,
    output_dir: outputDir,
    exclude: [],
  })
  addLog('success', `已添加项目: ${projectName}`)
  showAddForm.value = false
}

function handleSelect(id: string) {
  selectedProjectId.value = id
}

function handleDelete(id: string) {
  const project = findProject(id)
  openConfirm(
    '删除项目',
    `确定要删除项目「${project?.name ?? ''}」吗？此操作不可恢复。`,
    async () => {
      await deleteProject(id)
      addLog('info', `已删除项目: ${project?.name}`)
    }
  )
}

async function handleUpdate(id: string, data: any) {
  await updateProject(id, data)
}

async function handleUpdateDefaultExclude(rules: string[]) {
  config.value.default_exclude = rules
  await saveConfig()
}

async function handlePack(project: any, asZip: boolean = false) {
  await pack(project, config.value.default_exclude, asZip)
}

async function openOutputDir() {
  const project = selectedProject.value
  if (!project) return
  try {
    await invoke('open_output_dir', { outputDir: project.output_dir })
  } catch (err: any) {
    addLog('error', `打开目录失败: ${err}`)
  }
}
</script>

<template>
  <div class="app-layout">
    <!-- 左侧：项目分组列表 -->
    <aside class="sidebar">
      <ProjectList
        :groups="config.groups"
        :selected-id="selectedProjectId"
        @select="handleSelect"
        @add-group="showAddGroupForm = true"
        @delete-group="handleDeleteGroup"
        @rename-group="handleRenameGroup"
        @add-project="openAddForm"
        @delete-project="handleDelete"
        @reorder-groups="reorderGroups"
        @reorder-projects="reorderProjects"
        @move-project="moveProject"
      />

      <!-- 软件更新 -->
      <div class="update-bar">
        <button
          class="update-btn"
          :class="{ 'update-btn-primary': updateStatus === 'available' || updateStatus === 'installed' }"
          :disabled="updateBusy"
          @click="handleUpdateAction"
        >{{ updateButtonLabel }}</button>

        <div v-if="updateStatus === 'downloading'" class="update-progress">
          <div class="update-progress-inner" :style="{ width: updatePercentage + '%' }"></div>
        </div>

        <p v-if="updateStatus === 'error'" class="update-hint update-hint-error">{{ updateError }}</p>
        <p v-else-if="updateStatus === 'installed'" class="update-hint">安装完成，点击上方按钮重启</p>
        <p v-else-if="updateStatus === 'available'" class="update-hint">
          {{ updateInfo?.notes || '发现可用更新，点击上方按钮下载安装' }}
        </p>
      </div>
    </aside>

    <!-- 右侧：配置 + 进度 + 日志 -->
    <main class="main-content">
      <!-- 新版本提示（每日定时校验或手动检查后出现） -->
      <div v-if="hasUpdatePrompt" class="update-banner">
        <span class="update-banner-text">
          发现新版本 <strong>v{{ updateInfo?.version }}</strong>
          <span class="update-banner-sub">（当前 v{{ updateInfo?.currentVersion }}）</span>
        </span>
        <div class="update-banner-actions">
          <button class="btn btn-primary" :disabled="updateBusy" @click="handleUpdateAction">
            {{ updateBusy ? '下载中…' : '立即更新' }}
          </button>
          <button class="btn btn-secondary" @click="handleDismissUpdate">稍后</button>
        </div>
      </div>

      <!-- 新建分组弹窗 -->
      <div v-if="showAddGroupForm" class="add-form-overlay" @click.self="showAddGroupForm = false">
        <div class="add-form add-form-sm">
          <h3>新建分组</h3>
          <div class="form-row">
            <input v-model="newGroupName" class="form-input" placeholder="分组名称" @keydown.enter="handleAddGroup" autofocus />
          </div>
          <div class="form-actions">
            <button class="btn btn-secondary" @click="showAddGroupForm = false">取消</button>
            <button class="btn btn-primary" @click="handleAddGroup">确定</button>
          </div>
        </div>
      </div>

      <!-- 添加项目表单 -->
      <div v-if="showAddForm" class="add-form-overlay" @click.self="showAddForm = false">
        <div class="add-form">
          <h3>添加项目</h3>
          <div class="form-row">
            <label>项目名称</label>
            <input v-model="newProjectName" class="form-input" placeholder="如：余杭OA" />
          </div>
          <div class="form-row">
            <label>源目录</label>
            <div class="dir-row">
              <input v-model="newProjectSource" class="form-input" placeholder="选择源目录" readonly />
              <button class="btn btn-pick" @click="selectNewSource">选择</button>
            </div>
          </div>
          <div class="form-row">
            <label>输出目录 (可选，默认为源目录-dist)</label>
            <div class="dir-row">
              <input v-model="newProjectOutput" class="form-input" placeholder="选择输出目录" readonly />
              <button class="btn btn-pick" @click="selectNewOutput">选择</button>
            </div>
          </div>
          <div class="form-actions">
            <button class="btn btn-secondary" @click="showAddForm = false">取消</button>
            <button class="btn btn-primary" @click="handleAdd">确定</button>
          </div>
        </div>
      </div>

      <!-- 确认弹窗 -->
      <div v-if="showConfirmDialog" class="add-form-overlay" @click.self="handleConfirmCancel">
        <div class="confirm-dialog">
          <h3 class="confirm-title">{{ confirmTitle }}</h3>
          <p class="confirm-message">{{ confirmMessage }}</p>
          <div class="form-actions confirm-actions">
            <button class="btn btn-secondary" @click="handleConfirmCancel">取消</button>
            <button class="btn btn-danger" @click="handleConfirmOk">确定删除</button>
          </div>
        </div>
      </div>

      <!-- 配置面板 -->
      <div class="config-section">
        <ProjectConfig
          :project="selectedProject"
          :default-exclude="config.default_exclude"
          @update="handleUpdate"
          @pack="handlePack"
          @open-output="openOutputDir"
          @update-default-exclude="handleUpdateDefaultExclude"
        />
      </div>

      <!-- 进度条 -->
      <PackProgress :progress="progress" :packing="packing" />

      <!-- 日志面板 -->
      <div class="log-section">
        <LogPanel :logs="logs" @clear="clearLogs" />
      </div>
    </main>
  </div>
</template>

<style scoped>
.app-layout {
  display: flex;
  height: 100vh;
}
.sidebar {
  width: 240px;
  border-right: 1px solid var(--border-color);
  background: var(--bg-primary);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}
.main-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* 软件更新栏 */
.update-bar {
  flex-shrink: 0;
  padding: 10px;
  border-top: 1px solid var(--border-color);
}
.update-btn {
  width: 100%;
  padding: 8px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 13px;
  cursor: pointer;
  transition: all 0.15s;
}
.update-btn:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--primary);
}
.update-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.update-btn-primary {
  border-color: var(--primary);
  color: var(--primary);
  font-weight: 500;
}
.update-progress {
  margin-top: 8px;
  height: 4px;
  border-radius: 2px;
  background: var(--bg-tag);
  overflow: hidden;
}
.update-progress-inner {
  height: 100%;
  background: var(--primary);
  transition: width 0.2s;
}
.update-hint {
  margin: 8px 0 0;
  font-size: 12px;
  line-height: 1.45;
  color: var(--text-muted);
  word-break: break-word;
}
.update-hint-error {
  color: var(--danger);
}

/* 新版本提示横幅 */
.update-banner {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 20px;
  background: rgba(37, 99, 235, 0.08);
  border-bottom: 1px solid rgba(37, 99, 235, 0.25);
}
.update-banner-text {
  font-size: 13px;
  color: var(--text-primary);
}
.update-banner-text strong {
  color: var(--primary);
}
.update-banner-sub {
  color: var(--text-muted);
}
.update-banner-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
.config-section {
  flex: 1;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}
.log-section {
  height: 200px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-primary);
  flex-shrink: 0;
}

/* 弹窗 */
.add-form-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.3);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}
.add-form {
  background: var(--bg-primary);
  border-radius: 12px;
  padding: 24px;
  width: 500px;
  max-width: 90vw;
}
.add-form-sm {
  width: 360px;
}
.add-form h3 {
  margin-bottom: 20px;
  font-size: 18px;
  color: var(--text-primary);
}
.form-row {
  margin-bottom: 16px;
}
.form-row label {
  display: block;
  margin-bottom: 6px;
  font-size: 13px;
  color: var(--text-secondary);
}
.form-input {
  width: 100%;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  font-size: 14px;
  background: var(--bg-input);
  color: var(--text-primary);
  outline: none;
}
.form-input:focus {
  border-color: var(--primary);
}
.dir-row {
  display: flex;
  gap: 8px;
}
.dir-row .form-input {
  flex: 1;
}
.form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
}

/* 确认弹窗 */
.confirm-dialog {
  background: var(--bg-primary);
  border-radius: 12px;
  padding: 24px;
  width: 380px;
  max-width: 90vw;
}
.confirm-title {
  font-size: 17px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 12px;
}
.confirm-message {
  font-size: 14px;
  color: var(--text-secondary);
  line-height: 1.6;
  margin-bottom: 8px;
}
.confirm-actions {
  margin-top: 24px;
}
</style>
