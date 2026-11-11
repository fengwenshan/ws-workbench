import { ref } from 'vue'
import { message } from 'ant-design-vue'
import type { ProjectConfig, ProjectGroup } from '@/types'
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
    groupId: string | null,
    data: NewProjectData,
  ) => Promise<ProjectConfig | undefined>
  onProjectCreated?: (project: ProjectConfig) => void
}

/** 「新建」弹窗顶部的两种类型 */
export type CreateTab = 'group' | 'project'

/**
 * 新建分组 / 新建项目的表单状态与提交逻辑。
 * 两者共用一个弹窗：顶部切类型，下面切换对应表单；表单校验与错误提示都收在这里。
 */
export function useProjectForm(options: Options) {
  const { addGroup, addProject, onProjectCreated } = options

  const showCreateModal = ref(false)
  const createTab = ref<CreateTab>('group')
  // 顶部「+ 新建」显示类型切换；分组底部「+ 添加项目」只显示项目表单
  const createTabsVisible = ref(true)

  // ---- 新建分组 ----
  const newGroupName = ref('')

  async function handleAddGroup(): Promise<boolean> {
    const name = newGroupName.value.trim()
    if (!name) {
      message.error('请填写分组名称')
      return false
    }
    try {
      await addGroup(name)
      showCreateModal.value = false
      return true
    } catch (err) {
      message.error(`创建分组失败: ${err}`)
      return false
    }
  }

  // ---- 新建项目 ----
  const newProjectName = ref('')
  const newProjectSource = ref('')
  const newProjectOutput = ref('')
  // 所属分组：null = 一级（树根），也可以选某个分组
  const targetGroupId = ref<string | null>(null)
  // 原生选目录对话框是模态的，期间再点「选择」会叠出第二个框
  const pickingProjectPath = ref(false)

  function resetProjectDraft(groupId: string | null) {
    targetGroupId.value = groupId
    newProjectSource.value = ''
    newProjectName.value = ''
    newProjectOutput.value = ''
  }

  /** 打开「新建」弹窗：tab 决定顶部选中的类型，groupId 为分组底部「+ 添加项目」预置的所属分组 */
  function openCreateModal(tab: CreateTab, groupId: string | null = null) {
    newGroupName.value = ''
    resetProjectDraft(tab === 'project' ? groupId : null)
    createTab.value = tab
    createTabsVisible.value = true
    showCreateModal.value = true
  }

  function switchCreateTab(tab: CreateTab) {
    createTab.value = tab
  }

  // 分组底部「+ 添加项目」：先选项目目录，选好再弹出新建项目表单（目录已带出）
  async function openAddForm(groupId: string | null) {
    if (pickingProjectPath.value) return
    pickingProjectPath.value = true
    try {
      const selected = await pickDirectory({ title: '选择项目目录' })
      if (!selected) return
      openCreateModal('project', groupId)
      // 从分组进来只需要项目表单，不再显示「新建分组 / 新建项目」切换
      createTabsVisible.value = false
      applyProjectSource(selected)
    } catch (err) {
      message.error(`打开目录选择失败: ${err}`)
    } finally {
      pickingProjectPath.value = false
    }
  }

  // 源目录变了就同步带出项目名称与输出目录（两者之后都能改）
  function applyProjectSource(dir: string) {
    newProjectSource.value = dir
    newProjectName.value = basename(dir)
    newProjectOutput.value = joinPath(dir, 'dist')
  }

  async function selectNewSource() {
    if (pickingProjectPath.value) return
    pickingProjectPath.value = true
    try {
      const selected = await pickDirectory({ title: '选择项目目录' })
      if (!selected) return
      applyProjectSource(selected)
    } catch (err) {
      message.error(`打开目录选择失败: ${err}`)
    } finally {
      pickingProjectPath.value = false
    }
  }

  async function selectNewOutput() {
    if (pickingProjectPath.value) return
    pickingProjectPath.value = true
    try {
      const selected = await pickDirectory({
        defaultPath: newProjectOutput.value || undefined,
        title: '选择输出目录',
      })
      if (selected) newProjectOutput.value = selected
    } catch (err) {
      message.error(`打开目录选择失败: ${err}`)
    } finally {
      pickingProjectPath.value = false
    }
  }

  async function handleAdd(): Promise<boolean> {
    if (!newProjectSource.value.trim()) {
      message.warning('请选择项目目录')
      return false
    }

    const sourceDir = newProjectSource.value.trim()
    // 项目名称为空时，自动用源目录最后一层名字
    const projectName = newProjectName.value.trim() || basename(sourceDir)
    // 输出目录为空时，自动追加 /dist
    const outputDir =
      newProjectOutput.value.trim() || joinPath(sourceDir, 'dist')

    if (!projectName) {
      message.warning('请填写项目名称')
      return false
    }

    const created = await addProject(targetGroupId.value, {
      name: projectName,
      source_dir: sourceDir,
      output_dir: outputDir,
      exclude: [],
    })
    if (created) {
      onProjectCreated?.(created)
      showCreateModal.value = false
      return true
    }
    return false
  }

  return {
    // 新建弹窗（分组 / 项目共用）
    showCreateModal,
    createTab,
    createTabsVisible,
    openCreateModal,
    switchCreateTab,
    // 分组表单
    newGroupName,
    handleAddGroup,
    // 项目表单
    newProjectName,
    newProjectSource,
    newProjectOutput,
    targetGroupId,
    openAddForm,
    selectNewSource,
    selectNewOutput,
    handleAdd,
  }
}
