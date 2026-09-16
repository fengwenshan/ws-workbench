import { ref, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { AppConfig, ProjectConfig, ProjectGroup } from '@/types'

export function useConfig() {
  const config = ref<AppConfig>({ groups: [], projects: [], default_exclude: [] })
  const selectedProjectId = ref<string | null>(null)
  const loading = ref(false)

  async function loadConfig() {
    loading.value = true
    try {
      config.value = await invoke<AppConfig>('get_config')
      // 自动选中第一个项目
      if (!selectedProjectId.value) {
        const first = config.value.groups
          .flatMap(g => g.projects)
          .find(Boolean)
        if (first) selectedProjectId.value = first.id
      }
    } finally {
      loading.value = false
    }
  }

  async function saveConfig() {
    await invoke('save_config', { config: config.value })
  }

  // 查找项目所在分组
  function findGroupByProject(projectId: string): ProjectGroup | undefined {
    return config.value.groups.find(g => g.projects.some(p => p.id === projectId))
  }

  // 查找项目
  function findProject(projectId: string): ProjectConfig | undefined {
    for (const g of config.value.groups) {
      const p = g.projects.find(p => p.id === projectId)
      if (p) return p
    }
    return undefined
  }

  // 新建分组
  async function addGroup(name: string): Promise<ProjectGroup> {
    const group: ProjectGroup = {
      id: crypto.randomUUID(),
      name,
      projects: [],
      is_default: false,
      created_at: new Date().toISOString(),
    }
    config.value.groups.push(group)
    await saveConfig()
    return group
  }

  // 删除分组（默认分组不允许删除，但内部项目可单独删除）
  async function deleteGroup(groupId: string) {
    const group = config.value.groups.find(g => g.id === groupId)
    if (!group) return
    if (group.is_default) return
    // 如果删除的分组中有选中项目，清除选中
    const hasSelected = group.projects.some(p => p.id === selectedProjectId.value)
    config.value.groups = config.value.groups.filter(g => g.id !== groupId)
    if (hasSelected) {
      const first = config.value.groups
        .flatMap(g => g.projects)
        .find(Boolean)
      selectedProjectId.value = first?.id ?? null
    }
    await saveConfig()
  }

  // 重命名分组（默认分组不允许重命名）
  async function renameGroup(groupId: string, name: string) {
    const group = config.value.groups.find(g => g.id === groupId)
    if (!group) return
    if (group.is_default) return
    const trimmed = name.trim()
    if (!trimmed) return
    group.name = trimmed
    await saveConfig()
  }

  // 分组重新排序
  async function reorderGroups(newGroups: ProjectGroup[]) {
    config.value.groups = newGroups
    await saveConfig()
  }

  // 分组内项目重新排序
  async function reorderProjects(groupId: string, newProjects: ProjectConfig[]) {
    const group = config.value.groups.find(g => g.id === groupId)
    if (!group) return
    group.projects = newProjects
    await saveConfig()
  }

  // 项目跨分组移动
  async function moveProject(projectId: string, fromGroupId: string, toGroupId: string, toIndex: number) {
    const fromGroup = config.value.groups.find(g => g.id === fromGroupId)
    const toGroup = config.value.groups.find(g => g.id === toGroupId)
    if (!fromGroup || !toGroup) return

    const projectIndex = fromGroup.projects.findIndex(p => p.id === projectId)
    if (projectIndex === -1) return

    const [project] = fromGroup.projects.splice(projectIndex, 1)
    toGroup.projects.splice(toIndex, 0, project)
    await saveConfig()
  }

  // 在指定分组中添加项目
  async function addProject(groupId: string, data: Omit<ProjectConfig, 'id' | 'created_at' | 'updated_at'>): Promise<ProjectConfig | undefined> {
    const group = config.value.groups.find(g => g.id === groupId)
    if (!group) return
    const now = new Date().toISOString()
    const project: ProjectConfig = {
      ...data,
      id: crypto.randomUUID(),
      created_at: now,
      updated_at: now,
    }
    group.projects.push(project)
    await saveConfig()
    selectedProjectId.value = project.id
    return project
  }

  // 更新项目
  async function updateProject(id: string, data: Partial<ProjectConfig>) {
    const project = findProject(id)
    if (project) {
      Object.assign(project, data, { updated_at: new Date().toISOString() })
      await saveConfig()
    }
  }

  // 删除项目
  async function deleteProject(id: string) {
    const group = findGroupByProject(id)
    if (!group) return
    group.projects = group.projects.filter(p => p.id !== id)
    if (selectedProjectId.value === id) {
      const first = config.value.groups
        .flatMap(g => g.projects)
        .find(Boolean)
      selectedProjectId.value = first?.id ?? null
    }
    await saveConfig()
  }

  onMounted(loadConfig)

  return {
    config,
    selectedProjectId,
    loading,
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
  }
}
