import { ref } from 'vue'
import { message } from 'ant-design-vue'
import type { LogLevel, ProjectConfig, ProjectGroup } from '@/types'
import { pickDirectory } from '@/utils/dialog'
import { basename, joinPath } from '@/utils/path'

interface NewProjectData {
  name: string
  source_dir: string
  output_dir: string
  exclude: string[]
}

interface Options {
  addGroup: (name: string) => Promise<ProjectGroup>
  addProject: (
    groupId: string,
    data: NewProjectData,
  ) => Promise<ProjectConfig | undefined>
  addLog: (level: LogLevel, message: string) => void
  onProjectCreated?: (project: ProjectConfig) => void
}

/**
 * 新建分组 / 添加项目的表单状态与提交逻辑。
 * 表单校验、日志与错误提示都收在这里，App.vue 只绑定模板。
 */
export function useProjectForm(options: Options) {
  const { addGroup, addProject, addLog, onProjectCreated } = options

  // ---- 新建分组 ----
  // 附件上传统一走「设置 → 附件上传」里的地址，分组只需要一个名称
  const showAddGroupForm = ref(false)
  const newGroupName = ref('')

  function openAddGroupForm() {
    newGroupName.value = ''
    showAddGroupForm.value = true
  }

  // 表单错误反馈：写入日志并弹提示，避免只记日志看不到
  function rejectForm(text: string) {
    addLog('error', text)
    message.error(text)
  }

  async function handleAddGroup() {
    const name = newGroupName.value.trim()
    if (!name) {
      rejectForm('请填写分组名称')
      return
    }
    try {
      await addGroup(name)
      addLog('success', `已创建分组: ${name}`)
      newGroupName.value = ''
      showAddGroupForm.value = false
    } catch (err) {
      rejectForm(`创建分组失败: ${err}`)
    }
  }

  // ---- 添加项目 ----
  const showAddForm = ref(false)
  const targetGroupId = ref<string | null>(null)
  const newProjectName = ref('')
  const newProjectSource = ref('')
  const newProjectOutput = ref('')
  // 原生选目录对话框是模态的，期间再点「添加项目」会叠出第二个框
  const pickingProjectPath = ref(false)

  // 源目录变了就同步带出项目名称与输出目录（两者之后都能改）
  function applyProjectSource(dir: string) {
    newProjectSource.value = dir
    newProjectName.value = basename(dir)
    newProjectOutput.value = joinPath(dir, 'dist')
  }

  // 点「添加项目」先选项目目录，选好才弹窗补信息；取消则不弹窗
  async function openAddForm(groupId: string) {
    if (pickingProjectPath.value) return
    pickingProjectPath.value = true
    try {
      const selected = await pickDirectory({ title: '选择项目目录' })
      if (!selected) return
      targetGroupId.value = groupId
      applyProjectSource(selected)
      showAddForm.value = true
    } catch (err) {
      rejectForm(`打开目录选择失败: ${err}`)
    } finally {
      pickingProjectPath.value = false
    }
  }

  // 弹窗里重新选项目目录：项目名称与输出目录跟着重算
  async function selectNewSource() {
    if (pickingProjectPath.value) return
    pickingProjectPath.value = true
    try {
      const selected = await pickDirectory({
        defaultPath: newProjectSource.value || undefined,
        title: '选择项目目录',
      })
      if (!selected) return
      applyProjectSource(selected)
    } catch (err) {
      rejectForm(`打开目录选择失败: ${err}`)
    } finally {
      pickingProjectPath.value = false
    }
  }

  async function selectNewOutput() {
    if (pickingProjectPath.value) return
    pickingProjectPath.value = true
    try {
      const selected = await pickDirectory({ title: '选择输出目录' })
      if (selected) newProjectOutput.value = selected
    } catch (err) {
      rejectForm(`打开目录选择失败: ${err}`)
    } finally {
      pickingProjectPath.value = false
    }
  }

  async function handleAdd() {
    if (!targetGroupId.value) {
      addLog('warn', '未选择分组')
      return
    }
    if (!newProjectSource.value.trim()) {
      addLog('warn', '请选择项目目录')
      return
    }

    const sourceDir = newProjectSource.value.trim()
    // 项目名称为空时，自动用源目录最后一层名字
    const projectName = newProjectName.value.trim() || basename(sourceDir)
    // 输出目录为空时，自动追加 /dist
    const outputDir =
      newProjectOutput.value.trim() || joinPath(sourceDir, 'dist')

    if (!projectName) {
      addLog('warn', '请填写项目名称')
      return
    }

    const created = await addProject(targetGroupId.value, {
      name: projectName,
      source_dir: sourceDir,
      output_dir: outputDir,
      exclude: [],
    })
    if (created) onProjectCreated?.(created)
    addLog('success', `已添加项目: ${projectName}（${sourceDir}）`)
    showAddForm.value = false
  }

  return {
    // 分组表单
    showAddGroupForm,
    newGroupName,
    openAddGroupForm,
    handleAddGroup,
    // 项目表单
    showAddForm,
    newProjectName,
    newProjectSource,
    newProjectOutput,
    openAddForm,
    selectNewSource,
    selectNewOutput,
    handleAdd,
  }
}
