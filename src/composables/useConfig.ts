import { ref, onMounted } from 'vue'
import { loadAppConfig, saveAppConfig } from '@/api/config'
import { nowIso } from '@/utils/time'
import type { AppConfig, AttachmentMeta, EntityKind, ProjectConfig, ProjectGroup } from '@/types'

export function useConfig() {
  const config = ref<AppConfig>({
    groups: [],
    projects: [],
    default_include: [],
    vpn_profiles: [],
    vpn_rule_sets: [],
    vpn_active_profile_id: '',
    default_upload_dir: null,
  })
  const selectedProjectId = ref<string | null>(null)
  const loading = ref(false)

  // 老版本配置缺字段时补齐，避免模板访问 undefined
  function normalizeEntity(entity: ProjectConfig | ProjectGroup) {
    if (typeof entity.note !== 'string') entity.note = ''
    if (!Array.isArray(entity.attachments)) entity.attachments = []
    // 旧版本给分组/项目单独存过上传目录，现在统一用设置里的地址，读到就丢弃
    delete (entity as ProjectConfig & { upload_dir?: string | null }).upload_dir
  }

  async function loadConfig() {
    loading.value = true
    try {
      const loaded = await loadAppConfig()
      // 老版本配置里没有 VPN 字段，这里补齐，避免模板里访问 undefined
      config.value = {
        ...loaded,
        vpn_profiles: loaded.vpn_profiles ?? [],
        vpn_rule_sets: loaded.vpn_rule_sets ?? [],
        vpn_active_profile_id: loaded.vpn_active_profile_id ?? '',
        default_upload_dir: loaded.default_upload_dir ?? null,
      }
      // 补齐 note / attachments，并清掉旧版本单独存的上传目录
      config.value.groups.forEach(normalizeEntity)
      config.value.groups.forEach(g => g.projects.forEach(normalizeEntity))
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
    await saveAppConfig(config.value)
  }

  // 查找分组
  function findGroup(groupId: string): ProjectGroup | undefined {
    return config.value.groups.find(g => g.id === groupId)
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

  // 按 kind 解析分组/项目实体
  function resolveEntity(kind: EntityKind, id: string): ProjectGroup | ProjectConfig | undefined {
    return kind === 'group' ? findGroup(id) : findProject(id)
  }

  // 新建分组（附件上传统一走设置里的地址，分组不再有自己的目录）
  async function addGroup(name: string): Promise<ProjectGroup> {
    const group: ProjectGroup = {
      id: crypto.randomUUID(),
      name,
      projects: [],
      is_default: false,
      created_at: nowIso(),
      note: '',
      attachments: [],
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

  // 按 id 更新分组名称，笔记、附件与组内项目原样保留
  async function updateGroup(groupId: string, data: { name?: string }) {
    const group = config.value.groups.find(g => g.id === groupId)
    if (!group) return
    // 默认分组不允许重命名
    if (typeof data.name === 'string' && !group.is_default) {
      const trimmed = data.name.trim()
      if (trimmed) group.name = trimmed
    }
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
  async function addProject(
    groupId: string,
    data: Omit<ProjectConfig, 'id' | 'created_at' | 'updated_at' | 'note' | 'attachments'> &
      Partial<Pick<ProjectConfig, 'note' | 'attachments'>>,
  ): Promise<ProjectConfig | undefined> {
    const group = config.value.groups.find(g => g.id === groupId)
    if (!group) return
    const now = nowIso()
    const project: ProjectConfig = {
      note: '',
      attachments: [],
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
      Object.assign(project, data, { updated_at: nowIso() })
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

  // 更新分组/项目笔记
  async function updateNote(kind: EntityKind, id: string, note: string) {
    const entity = resolveEntity(kind, id)
    if (!entity) return
    entity.note = note
    if (kind === 'project') (entity as ProjectConfig).updated_at = nowIso()
    await saveConfig()
  }

  // 设置全局默认附件上传目录（分组/项目的附件统一存到这里）
  async function setDefaultUploadDir(dir: string | null) {
    config.value.default_upload_dir = dir
    await saveConfig()
  }

  // 追加附件元数据
  async function addAttachment(kind: EntityKind, id: string, meta: AttachmentMeta) {
    const entity = resolveEntity(kind, id)
    if (!entity) return
    entity.attachments.push(meta)
    if (kind === 'project') (entity as ProjectConfig).updated_at = nowIso()
    await saveConfig()
  }

  // 移除附件元数据
  async function removeAttachment(kind: EntityKind, id: string, attachmentId: string) {
    const entity = resolveEntity(kind, id)
    if (!entity) return
    entity.attachments = entity.attachments.filter(a => a.id !== attachmentId)
    if (kind === 'project') (entity as ProjectConfig).updated_at = nowIso()
    await saveConfig()
  }

  // 重命名附件（仅改元数据，blob 名保持 uuid）
  async function renameAttachment(kind: EntityKind, id: string, attachmentId: string, name: string) {
    const entity = resolveEntity(kind, id)
    if (!entity) return
    const meta = entity.attachments.find(a => a.id === attachmentId)
    if (!meta) return
    meta.name = name
    if (kind === 'project') (entity as ProjectConfig).updated_at = nowIso()
    await saveConfig()
  }

  // 收集实体名下的附件（分组含其名下所有项目），用于删除实体时联动删 blobs
  function collectAttachments(kind: EntityKind, id: string): AttachmentMeta[] {
    if (kind === 'project') {
      const project = findProject(id)
      return project ? [...project.attachments] : []
    }
    const group = findGroup(id)
    if (!group) return []
    return [...group.attachments, ...group.projects.flatMap(p => p.attachments)]
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
    updateGroup,
    reorderGroups,
    reorderProjects,
    moveProject,
    addProject,
    updateProject,
    deleteProject,
    findGroup,
    findProject,
    updateNote,
    setDefaultUploadDir,
    addAttachment,
    removeAttachment,
    renameAttachment,
    collectAttachments,
  }
}
