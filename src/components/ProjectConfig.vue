<script setup lang="ts">
import { ref, reactive, watch, computed, nextTick, onUnmounted } from 'vue'
import type { LogLevel, ProjectConfig, ProjectType, VcsInfo } from '@/types'
import {
  detectProjectType as detectProjectTypeCommand,
  detectVcs as detectVcsCommand,
} from '@/api/project'
import { checkDir } from '@/api/system'
import { pickDirectory } from '@/utils/dialog'
import { basename, joinPath } from '@/utils/path'

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
}>()

const emit = defineEmits<{
  update: [id: string, data: Partial<ProjectConfig>]
  pack: [project: ProjectConfig, asZip: boolean]
  log: [level: LogLevel, message: string]
}>()

const localProject = ref<ProjectConfig | null>(null)
const projectType = ref<ProjectType>('unknown')
// 仓库地址复制反馈（需要在项目切换的 watch 之前声明）
const copied = ref(false)
let copiedTimer: ReturnType<typeof setTimeout> | null = null
let saveTimer: any = null

watch(() => props.project, (newVal) => {
  // 切换/删除项目时，先清除待保存的计时器，避免旧数据写入
  if (saveTimer) {
    clearTimeout(saveTimer)
    saveTimer = null
  }
  copied.value = false
  if (newVal) {
    localProject.value = { ...newVal }
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
// 源目录是否已不存在（只影响提示与「识别」按钮，不改任何数据）
const sourceMissing = ref(false)
let sourceCheckSeq = 0

watch(
  () => localProject.value?.source_dir,
  (dir) => {
    if (detectTimer) clearTimeout(detectTimer)
    // 先重置为未知，避免旧类型残留导致误操作
    projectType.value = 'unknown'
    vcsInfo.value = null
    sourceMissing.value = false
    if (!dir) return
    detectTimer = setTimeout(() => {
      detectProjectType(dir)
      detectVcs(dir)
      checkSourceDir(dir)
    }, 250)
  },
  { immediate: true }
)

// 目录失效时给出提示：记录还在，重新选目录即可恢复，笔记/附件/排除规则都不受影响
async function checkSourceDir(dir: string) {
  const seq = ++sourceCheckSeq
  try {
    const status = await checkDir(dir)
    if (seq === sourceCheckSeq && localProject.value?.source_dir === dir) {
      sourceMissing.value = !status.is_dir
    }
  } catch {
    if (seq === sourceCheckSeq) sourceMissing.value = false
  }
}

async function detectProjectType(dir: string) {
  if (!dir) {
    projectType.value = 'unknown'
    return
  }
  pendingDetect++
  detecting.value = true
  try {
    const type = await detectProjectTypeCommand(dir)
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
    const info = await detectVcsCommand(dir)
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
  checkSourceDir(dir)
}

// 是否可手动识别（源目录存在时可用；目录失效只影响识别，不影响记录）
const canDetect = computed(
  () => !!localProject.value?.source_dir && !sourceMissing.value,
)

// 是否为 Layui 项目（只有 Layui 项目支持打包）
const canPack = computed(() => projectType.value === 'layui' && !sourceMissing.value)

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

// 点击 Git / SVN 标签或地址复制远程仓库地址，短暂显示「已复制」反馈
async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    // 某些 WebView 下 Clipboard API 不可用，退回文本框复制
  }
  try {
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.style.position = 'fixed'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(textarea)
    return ok
  } catch {
    return false
  }
}

async function copyVcsUrl() {
  const url = vcsInfo.value?.url
  if (!url) return
  const ok = await copyText(url)
  if (!ok) {
    emit('log', 'error', '复制仓库地址失败，请手动选中地址复制')
    return
  }
  copied.value = true
  emit('log', 'success', `已复制仓库地址: ${url}`)
  if (copiedTimer) clearTimeout(copiedTimer)
  copiedTimer = setTimeout(() => {
    copied.value = false
  }, 1500)
}

onUnmounted(() => {
  if (copiedTimer) clearTimeout(copiedTimer)
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

async function selectSourceDir() {
  const current = localProject.value?.source_dir || ''
  const selected = await pickDirectory({ defaultPath: current || undefined })
  if (selected && localProject.value) {
    const sourceDir = selected
    localProject.value.source_dir = sourceDir
    // 源目录改变时，强制更新项目名称和输出目录
    localProject.value.name = basename(sourceDir)
    localProject.value.output_dir = joinPath(sourceDir, 'dist')
    debouncedSave()
  }
}

async function selectOutputDir() {
  const current = localProject.value?.output_dir || ''
  const selected = await pickDirectory({ defaultPath: current || undefined })
  if (selected && localProject.value) {
    localProject.value.output_dir = selected
    debouncedSave()
  }
}

function packFiles() {
  if (selectedProject.value) emit('pack', selectedProject.value!, false)
}

function packZip() {
  if (selectedProject.value) emit('pack', selectedProject.value!, true)
}

// 暴露头部操作区所需的状态与方法，供父组件在 Tabs 右侧渲染
const headerState = reactive({
  projectType,
  detecting,
  canDetect,
  canPack,
  typeLabel,
  sourceMissing,
  /** 标题栏「用其它应用打开」要用：源目录（取本地编辑值，不等防抖保存） */
  sourceDir: computed(() => localProject.value?.source_dir || ''),
})

defineExpose({
  headerState,
  refreshProjectType,
  packZip,
  packFiles,
})
</script>

<template>
  <div class="project-config" v-if="selectedProject">
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
          <button
            type="button"
            class="vcs-badge"
            :class="['vcs-' + vcsInfo.vcs_type, { 'vcs-badge-copy': !!vcsInfo.url }]"
            :title="vcsInfo.url ? `点击复制仓库地址：${vcsInfo.url}` : '未检测到仓库地址'"
            @click="copyVcsUrl"
          >
            {{ vcsTypeLabel }}
          </button>
          <span
            v-if="vcsInfo.url"
            class="vcs-url"
            :title="`点击复制：${vcsInfo.url}`"
            @click="copyVcsUrl"
          >{{ vcsInfo.url }}</span>
          <span v-else class="vcs-empty">未检测到仓库地址</span>
          <span v-if="copied" class="vcs-copied">已复制</span>
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
        <p v-if="sourceMissing" class="dir-missing-hint">
          ⚠ 源目录已不存在，打包与类型识别暂不可用。重新选择一个目录即可恢复，项目笔记、附件和排除规则都不会丢。
        </p>
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
.config-body {
  flex: 1;
  overflow-y: auto;
  /* 和其它页签（笔记 / 上传）对齐：内容区不加内边距，只留底部 12px，避免比别的页多缩进 */
  padding: 0 0 12px;
}
.type-tip {
  margin-top: 12px;
  padding: 10px 12px;
  border-radius: 6px;
  font-size: 14px;
  line-height: 1.5;
}
.type-tip-unknown {
  color: var(--text-secondary);
  background: var(--bg-input);
  border: 1px solid var(--border-color);
}
.form-row {
  margin-bottom: 16px;
}
.form-row label {
  display: block;
  margin-bottom: 6px;
  font-size: 14px;
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
/* 源目录失效提示：记录还在，重新选目录即可恢复 */
.dir-missing-hint {
  margin: 8px 0 0;
  font-size: 12px;
  line-height: 1.5;
  color: var(--danger);
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
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  font: inherit;
  line-height: 1.2;
}
.vcs-badge-copy {
  cursor: pointer;
  transition: border-color 0.12s, box-shadow 0.12s, filter 0.12s;
}
.vcs-badge-copy:hover {
  filter: brightness(0.96);
  box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.12);
}
.vcs-badge-copy:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: 1px;
}
.vcs-copied {
  flex: none;
  color: var(--success);
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
  cursor: pointer;
  transition: color 0.12s;
}
.vcs-url:hover {
  color: var(--primary);
  text-decoration: underline;
  text-underline-offset: 2px;
}
.vcs-empty {
  color: var(--text-muted);
}
.no-project {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-muted);
}
</style>
