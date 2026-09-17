<script setup lang="ts">
import { ref, watch, computed, nextTick } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import type { LogLevel, ProjectConfig, ProjectType, VcsInfo } from '@/types'
import ExcludeRules from './ExcludeRules.vue'

const sourceTextarea = ref<HTMLTextAreaElement | null>(null)
const outputTextarea = ref<HTMLTextAreaElement | null>(null)

// 自动调整 textarea 高度
function autoResize(el: HTMLTextAreaElement | null) {
  if (!el) return
  el.style.height = 'auto'
  el.style.height = el.scrollHeight + 'px'
}

const props = defineProps<{
  project: ProjectConfig | null
  defaultExclude: string[]
}>()

const emit = defineEmits<{
  update: [id: string, data: Partial<ProjectConfig>]
  pack: [project: ProjectConfig, asZip: boolean]
  log: [level: LogLevel, message: string]
  updateDefaultExclude: [rules: string[]]
}>()

const localProject = ref<ProjectConfig | null>(null)
const localDefaultExclude = ref<string[]>([])
const projectType = ref<ProjectType>('unknown')
let saveTimer: any = null

watch(() => props.defaultExclude, (newVal) => {
  localDefaultExclude.value = [...newVal]
}, { immediate: true })

watch(() => props.project, (newVal) => {
  // 切换/删除项目时，先清除待保存的计时器，避免旧数据写入
  if (saveTimer) {
    clearTimeout(saveTimer)
    saveTimer = null
  }
  if (newVal) {
    localProject.value = { ...newVal, exclude: [...newVal.exclude] }
  } else {
    // 项目被删除时，清空所有本地状态
    localProject.value = null
  }
}, { immediate: true })

// 源目录和输出目录变化时自动调整 textarea 高度
watch(
  () => [localProject.value?.source_dir, localProject.value?.output_dir],
  async () => {
    await nextTick()
    autoResize(sourceTextarea.value)
    autoResize(outputTextarea.value)
  }
)

// 源目录变化时检测项目类型与版本控制信息
let detectTimer: any = null
let pendingDetect = 0
const detecting = ref(false)
const vcsInfo = ref<VcsInfo | null>(null)

watch(
  () => localProject.value?.source_dir,
  (dir) => {
    if (detectTimer) clearTimeout(detectTimer)
    // 先重置为未知，避免旧类型残留导致误操作
    projectType.value = 'unknown'
    vcsInfo.value = null
    if (!dir) return
    detectTimer = setTimeout(() => {
      detectProjectType(dir)
      detectVcs(dir)
    }, 250)
  },
  { immediate: true }
)

async function detectProjectType(dir: string) {
  if (!dir) {
    projectType.value = 'unknown'
    return
  }
  pendingDetect++
  detecting.value = true
  try {
    const type = await invoke<ProjectType>('detect_project_type', { sourceDir: dir })
    // 防止异步返回时目录已变化
    if (localProject.value?.source_dir === dir) {
      projectType.value = type
    }
  } catch (err) {
    if (localProject.value?.source_dir === dir) {
      projectType.value = 'unknown'
    }
  } finally {
    pendingDetect--
    if (pendingDetect <= 0) {
      pendingDetect = 0
      detecting.value = false
    }
  }
}

// 检测源目录的版本控制类型与远程仓库地址
async function detectVcs(dir: string) {
  if (!dir) {
    vcsInfo.value = null
    return
  }
  try {
    const info = await invoke<VcsInfo>('detect_vcs', { sourceDir: dir })
    // 防止异步返回时目录已变化
    if (localProject.value?.source_dir === dir) {
      vcsInfo.value = info && info.vcs_type !== 'none' ? info : null
    }
  } catch (err) {
    if (localProject.value?.source_dir === dir) {
      vcsInfo.value = null
    }
  }
}

// 手动触发识别（跳过防抖，立即执行）
function refreshProjectType() {
  if (detectTimer) {
    clearTimeout(detectTimer)
    detectTimer = null
  }
  const dir = localProject.value?.source_dir || ''
  if (!dir) {
    projectType.value = 'unknown'
    vcsInfo.value = null
    return
  }
  projectType.value = 'unknown'
  vcsInfo.value = null
  detectProjectType(dir)
  detectVcs(dir)
}

// 是否可手动识别（源目录有值时可用）
const canDetect = computed(() => !!localProject.value?.source_dir)

// 是否为 Layui 项目（只有 Layui 项目支持打包）
const canPack = computed(() => projectType.value === 'layui')

const typeLabel = computed(() => {
  switch (projectType.value) {
    case 'layui': return 'Layui 项目'
    case 'vue2': return 'Vue2 项目'
    case 'vue3': return 'Vue3 项目'
    case 'react': return 'React 项目'
    case 'springboot': return 'Spring Boot 项目'
    case 'spring': return 'Spring 项目'
    case 'struts2': return 'Struts2 项目'
    case 'java': return 'Java 项目'
    default: return '未知类型项目'
  }
})

// 版本控制类型展示名（仅在检测到 git/svn 时展示）
const vcsTypeLabel = computed(() => {
  switch (vcsInfo.value?.vcs_type) {
    case 'git': return 'Git'
    case 'svn': return 'SVN'
    default: return ''
  }
})

const selectedProject = computed(() => localProject.value)

function debouncedSave() {
  if (!localProject.value) return
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(() => {
    if (localProject.value) {
      emit('update', localProject.value.id, {
        name: localProject.value.name,
        source_dir: localProject.value.source_dir,
        output_dir: localProject.value.output_dir,
        exclude: localProject.value.exclude,
      })
    }
  }, 500)
}

function onNameChange() { debouncedSave() }
function onSourceChange(e: Event) {
  autoResize(e.target as HTMLTextAreaElement)
  debouncedSave()
}
function onOutputChange(e: Event) {
  autoResize(e.target as HTMLTextAreaElement)
  debouncedSave()
}
function onExcludeChange() { debouncedSave() }

// 从路径中提取最后一层目录名（兼容 Windows 反斜杠与 POSIX 斜杠）
function getLastDirName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const idx = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  return idx >= 0 ? trimmed.slice(idx + 1) : trimmed
}

// 拼接子路径：沿用源目录的分隔符风格，避免 Windows 下出现 D:\a\b/dist
function joinPath(dir: string, name: string): string {
  const trimmed = dir.replace(/[\\/]+$/, '')
  const sep = trimmed.includes('\\') ? '\\' : '/'
  return `${trimmed}${sep}${name}`
}

async function selectSourceDir() {
  const current = localProject.value?.source_dir || ''
  const selected = await open({
    directory: true,
    multiple: false,
    defaultPath: current || undefined,
  })
  if (selected && localProject.value) {
    const sourceDir = selected as string
    localProject.value.source_dir = sourceDir
    // 源目录改变时，强制更新项目名称和输出目录
    localProject.value.name = getLastDirName(sourceDir)
    localProject.value.output_dir = joinPath(sourceDir, 'dist')
    debouncedSave()
  }
}

async function selectOutputDir() {
  const current = localProject.value?.output_dir || ''
  const selected = await open({
    directory: true,
    multiple: false,
    defaultPath: current || undefined,
  })
  if (selected && localProject.value) {
    localProject.value.output_dir = selected as string
    debouncedSave()
  }
}

function packFiles() {
  if (selectedProject.value) emit('pack', selectedProject.value!, false)
}

function packZip() {
  if (selectedProject.value) emit('pack', selectedProject.value!, true)
}

// ===== 打开相关 =====
async function runOpenCmd(cmd: string, path: string) {
  try {
    // open_dir 会返回实际打开的路径（目录不存在时已降级为上级目录）
    const actual = await invoke<string | null>(cmd, { path })
    if (actual && actual !== path) {
      emit('log', 'warn', `目录不存在，已打开上级目录: ${actual}`)
    } else {
      emit('log', 'info', `已打开: ${path}`)
    }
  } catch (err: any) {
    emit('log', 'error', `打开失败: ${err}`)
  }
}

function openSourceDir() {
  if (localProject.value) runOpenCmd('open_dir', localProject.value.source_dir)
}
function openSourceInTerminal() {
  if (localProject.value) runOpenCmd('open_in_terminal', localProject.value.source_dir)
}
function openSourceInVscode() {
  if (localProject.value) runOpenCmd('open_in_vscode', localProject.value.source_dir)
}
function openSourceInIdea() {
  if (localProject.value) runOpenCmd('open_in_idea', localProject.value.source_dir)
}
function openSourceInTrae() {
  if (localProject.value) runOpenCmd('open_in_trae', localProject.value.source_dir)
}
function openOutputDir() {
  if (localProject.value) runOpenCmd('open_dir', localProject.value.output_dir)
}
function openOutputInTerminal() {
  if (localProject.value) runOpenCmd('open_in_terminal', localProject.value.output_dir)
}
function openOutputInVscode() {
  if (localProject.value) runOpenCmd('open_in_vscode', localProject.value.output_dir)
}
function openOutputInIdea() {
  if (localProject.value) runOpenCmd('open_in_idea', localProject.value.output_dir)
}
function openOutputInTrae() {
  if (localProject.value) runOpenCmd('open_in_trae', localProject.value.output_dir)
}

// ===== 默认排除规则管理 =====
function removeDefaultRule(index: number) {
  localDefaultExclude.value.splice(index, 1)
  emit('updateDefaultExclude', [...localDefaultExclude.value])
}

// 清空默认排除规则
function clearDefaultRules() {
  localDefaultExclude.value = []
  emit('updateDefaultExclude', [])
}

// 恢复默认排除规则（以后端默认规则为准）
async function resetDefaultRules() {
  const defaults = await invoke<string[]>('get_default_exclude_rules')
  localDefaultExclude.value = [...defaults]
  emit('updateDefaultExclude', [...defaults])
}
</script>

<template>
  <div class="project-config" v-if="selectedProject">
    <div class="config-header">
      <h3>项目配置</h3>
      <div class="header-actions">
        <span class="type-badge" :class="'type-' + projectType">{{ detecting ? '识别中…' : typeLabel }}</span>
        <button
          class="btn-refresh-type"
          :disabled="detecting || !canDetect"
          title="重新识别项目类型及版本控制地址"
          @click="refreshProjectType"
        >{{ detecting ? '识别中…' : '识别' }}</button>
        <template v-if="canPack">
          <button class="btn btn-secondary" @click="packZip">打包 ZIP</button>
          <button class="btn btn-primary" @click="packFiles">开始打包</button>
        </template>
      </div>
    </div>

    <div class="config-body">
      <div class="form-row">
        <label>项目名称</label>
        <input
          v-model="localProject!.name"
          class="form-input"
          placeholder="项目名称"
          @input="onNameChange"
        />
        <div v-if="vcsInfo" class="vcs-info">
          <span class="vcs-badge" :class="'vcs-' + vcsInfo.vcs_type">{{ vcsTypeLabel }}</span>
          <span v-if="vcsInfo.url" class="vcs-url" :title="vcsInfo.url">{{ vcsInfo.url }}</span>
          <span v-else class="vcs-empty">未检测到仓库地址</span>
        </div>
      </div>

      <div class="form-row">
        <label>源目录</label>
        <div class="dir-row">
          <textarea
            ref="sourceTextarea"
            v-model="localProject!.source_dir"
            class="form-textarea"
            placeholder="选择源目录"
            rows="1"
            @input="onSourceChange"
          ></textarea>
          <button class="btn btn-pick" @click="selectSourceDir">选择</button>
        </div>
        <div class="action-btns">
          <button class="btn btn-action" @click="openSourceDir" title="Finder 中打开">打开</button>
          <button class="btn btn-action" @click="openSourceInTerminal" title="在终端中打开（目录不存在时打开上级目录）">终端</button>
          <button class="btn btn-action" @click="openSourceInVscode" title="用 VSCode 打开">VSCode</button>
          <button class="btn btn-action" @click="openSourceInIdea" title="用 IDEA 打开">IDEA</button>
          <button class="btn btn-action" @click="openSourceInTrae" title="用 Trae 打开">Trae</button>
        </div>
      </div>

      <div class="form-row">
        <label>输出目录</label>
        <div class="dir-row">
          <textarea
            ref="outputTextarea"
            v-model="localProject!.output_dir"
            class="form-textarea"
            placeholder="选择输出目录"
            rows="1"
            @input="onOutputChange"
          ></textarea>
          <button class="btn btn-pick" @click="selectOutputDir">选择</button>
        </div>
        <div class="action-btns">
          <button class="btn btn-action" @click="openOutputDir" title="Finder 中打开">打开</button>
          <button class="btn btn-action" @click="openOutputInTerminal" title="在终端中打开（目录不存在时打开上级目录）">终端</button>
          <button class="btn btn-action" @click="openOutputInVscode" title="用 VSCode 打开">VSCode</button>
          <button class="btn btn-action" @click="openOutputInIdea" title="用 IDEA 打开">IDEA</button>
          <button class="btn btn-action" @click="openOutputInTrae" title="用 Trae 打开">Trae</button>
        </div>
      </div>

      <div class="form-row">
        <label>项目排除规则</label>
        <ExcludeRules
          v-model="localProject!.exclude"
          placeholder="添加项目级排除规则（回车确认）"
          @update:modelValue="onExcludeChange"
        />
      </div>

      <div class="form-row">
        <div class="exclude-header">
          <label>默认排除规则（含空格文件名自动排除）</label>
          <button class="btn-reset-default" @click="resetDefaultRules">恢复默认</button>
        </div>
        <div class="default-exclude">
          <span
            v-for="(rule, index) in localDefaultExclude"
            :key="rule"
            class="tag-deletable"
          >
            {{ rule }}
            <button class="tag-remove" @click="removeDefaultRule(index)">×</button>
          </span>
          <span class="tag-readonly tag-special">*含空格的文件/目录</span>
        </div>
      </div>

      <div v-if="projectType === 'unknown'" class="type-tip type-tip-unknown">
        未识别到 Layui 项目特征，打包功能不可用。请确认源目录选择正确。
      </div>
    </div>
  </div>

  <div v-else class="no-project">
    <p>请选择或添加一个项目</p>
  </div>
</template>

<style scoped>
.project-config {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.config-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-color);
}
.config-header h3 {
  margin: 0;
  font-size: 16px;
  color: var(--text-primary);
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
.type-tip {
  margin-top: 12px;
  padding: 10px 12px;
  border-radius: 6px;
  font-size: 13px;
  line-height: 1.5;
}
.type-tip-unknown {
  color: var(--text-secondary);
  background: var(--bg-input);
  border: 1px solid var(--border-color);
}
.config-body {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
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
  align-items: flex-start;
}
.dir-row .form-textarea {
  flex: 1;
  resize: none;
  overflow: hidden;
  line-height: 1.4;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  font-size: 14px;
  background: var(--bg-input);
  color: var(--text-primary);
  outline: none;
  font-family: inherit;
  min-height: 36px;
  box-sizing: border-box;
}
.dir-row .form-textarea:focus {
  border-color: var(--primary);
}
.action-btns {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}
.btn-action {
  padding: 4px 10px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--bg-input);
  cursor: pointer;
  font-size: 12px;
  color: var(--text-secondary);
  transition: all 0.15s;
}
.btn-action:hover {
  border-color: var(--primary);
  color: var(--primary);
  background: #eff6ff;
}
.vcs-info {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  margin-top: 6px;
  font-size: 12px;
}
.vcs-badge {
  flex: none;
  padding: 2px 6px;
  border: 1px solid var(--border-color);
  border-radius: 3px;
  color: var(--text-secondary);
}
.vcs-badge.vcs-git {
  color: #f05032;
  border-color: #f05032;
  background: rgba(240, 80, 50, 0.08);
}
.vcs-badge.vcs-svn {
  color: #809cc9;
  border-color: #809cc9;
  background: rgba(128, 156, 201, 0.08);
}
.vcs-url {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-secondary);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  user-select: text;
}
.vcs-empty {
  color: var(--text-muted);
}
.exclude-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 6px;
}
.exclude-header label {
  margin-bottom: 0;
}
.btn-reset-default {
  padding: 4px 10px;
  font-size: 12px;
  border: 1px solid var(--primary);
  border-radius: 4px;
  background: transparent;
  color: var(--primary);
  cursor: pointer;
  transition: all 0.15s;
}
.btn-reset-default:hover {
  background: var(--primary);
  color: white;
}
.default-exclude {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  padding: 8px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-readonly);
}
.tag-readonly {
  padding: 2px 6px;
  background: var(--bg-tag);
  border-radius: 3px;
  font-size: 12px;
  color: var(--text-muted);
}
.tag-deletable {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 4px 2px 6px;
  background: var(--bg-tag);
  border-radius: 3px;
  font-size: 12px;
  color: var(--text-muted);
}
.tag-remove {
  border: none;
  background: none;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-muted);
  padding: 0;
  line-height: 1;
  border-radius: 2px;
  width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
}
.tag-remove:hover {
  background: var(--danger);
  color: white;
}
.tag-special {
  background: #fef3c7;
  color: #92400e;
}
.no-project {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-muted);
}
</style>
