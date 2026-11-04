import { ref } from 'vue'
import { message } from 'ant-design-vue'
import type { LogLevel, ProjectConfig, ProjectGroup } from '@/types'
import { ensureDir } from '@/api/system'
import { pickDirectory } from '@/utils/dialog'
import { basename, joinPath } from '@/utils/path'

interface NewProjectData {
  name: string
  source_dir: string
  output_dir: string
  exclude: string[]
}

interface Options {
  addGroup: (name: string, uploadDir: string | null) => Promise<ProjectGroup>
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
  const showAddGroupForm = ref(false)
  const newGroupName = ref('')
  const newGroupAddress = ref('')

  function openAddGroupForm() {
    newGroupName.value = ''
    newGroupAddress.value = ''
    showAddGroupForm.value = true
  }

  async function selectGroupAddress() {
    const selected = await pickDirectory()
    if (selected) newGroupAddress.value = selected
  }

  // 新建分组表单的错误反馈：写入日志并弹提示，避免只记日志看不到
  function rejectGroupForm(text: string) {
    addLog('error', text)
    message.error(text)
  }

  async function handleAddGroup() {
    const name = newGroupName.value.trim()
    if (!name) {
      rejectGroupForm('请填写分组名称')
      return
    }
    const address = newGroupAddress.value.trim()
    // 未填写地址时回退到系统默认上传位置；填写时先校验并由后端按需创建
    let uploadDir: string | null = null
    if (address) {
      try {
        uploadDir = await ensureDir(address)
      } catch (err) {
        rejectGroupForm(`地址不可用: ${err}`)
        return
      }
    }
    try {
      await addGroup(name, uploadDir)
      addLog('success', `已创建分组: ${name}`)
      newGroupName.value = ''
      newGroupAddress.value = ''
      showAddGroupForm.value = false
    } catch (err) {
      rejectGroupForm(`创建分组失败: ${err}`)
    }
  }

  // ---- 添加项目 ----
  const showAddForm = ref(false)
  const targetGroupId = ref<string | null>(null)
  const newProjectName = ref('')
  const newProjectSource = ref('')
  const newProjectOutput = ref('')

  async function selectNewSource() {
    const selected = await pickDirectory()
    if (!selected) return
    newProjectSource.value = selected
    // 源目录改变时，强制更新项目名称和输出目录
    newProjectName.value = basename(selected)
    newProjectOutput.value = joinPath(selected, 'dist')
  }

  async function selectNewOutput() {
    const selected = await pickDirectory()
    if (selected) newProjectOutput.value = selected
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
    addLog('success', `已添加项目: ${projectName}`)
    showAddForm.value = false
  }

  return {
    // 分组表单
    showAddGroupForm,
    newGroupName,
    newGroupAddress,
    openAddGroupForm,
    selectGroupAddress,
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
