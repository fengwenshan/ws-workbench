import { ref, onMounted } from 'vue'
import { migrateVpnRuleSets } from '@/utils/vpnConfigMigration'
import { loadAppConfig, saveAppConfig } from '@/api/config'
import { nowIso } from '@/utils/time'
import type { AppConfig, AttachmentMeta, EntityKind, ProjectConfig, ProjectGroup } from '@/types'

export function useConfig() {
  const config = ref<AppConfig>({
    groups: [],
    projects: [],
    tree_order: [],
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

  /** 一级项目：不属于任何分组、排在树根上的项目 */
  function rootProjects(): ProjectConfig[] {
    return config.value.projects ?? []
  }

  /** 一级兜底顺序：老数据没有 tree_order 时，分组在前、一级项目在后 */
  function defaultTreeOrder(): string[] {
    return [...config.value.groups.map(g => g.id), ...rootProjects().map(p => p.id)]
  }

  /** 清理 tree_order：去掉已删除的 id、补上遗漏的条目 */
  function normalizeTreeOrder(): string[] {
    const alive = new Set<string>([
      ...config.value.groups.map(g => g.id),
      ...rootProjects().map(p => p.id),
    ])
    const seen = new Set<string>()
    const order: string[] = []
    for (const id of config.value.tree_order ?? []) {
      if (!alive.has(id) || seen.has(id)) continue
      order.push(id)
      seen.add(id)
    }
    for (const id of defaultTreeOrder()) {
      if (seen.has(id)) continue
      order.push(id)
      seen.add(id)
    }
    return order
  }

  /** 按 tree_order 重排一级项目数组（数据库里的排序也用它） */
  function syncRootProjectOrder() {
    const order = config.value.tree_order ?? []
    const byId = new Map(rootProjects().map(p => [p.id, p]))
    const ordered: ProjectConfig[] = []
    for (const id of order) {
      const project = byId.get(id)
      if (project) ordered.push(project)
    }
    for (const project of rootProjects()) {
      if (!order.includes(project.id)) ordered.push(project)
    }
    config.value.projects = ordered
  }

  /** 树里第一个项目：按 tree_order 走，分组取组内第一个 */
  function firstProjectInTree(): ProjectConfig | undefined {
    for (const id of config.value.tree_order ?? []) {
      const project = rootProjects().find(p => p.id === id)
      if (project) return project
      const group = config.value.groups.find(g => g.id === id)
      if (group && group.projects.length > 0) return group.projects[0]
    }
    return rootProjects()[0] ?? config.value.groups.flatMap(g => g.projects)[0]
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
      // 老配置把规则放在全局列表里用 rule_set_id 引用，这里一次性搬进各配置
      const migration = migrateVpnRuleSets(config.value)
      if (migration.changed) {
        await saveConfig()
        if (migration.orphanLists > 0) {
          console.warn(
            `[vpn] 已迁移为“规则内嵌配置”模型；${migration.orphanLists} 份未被任何配置引用的旧规则列表仍保留在 config.json 中`,
          )
        }
      }
      // 补齐 note / attachments，并清掉旧版本单独存的上传目录
      config.value.groups.forEach(normalizeEntity)
      config.value.groups.forEach(g => g.projects.forEach(normalizeEntity))
      rootProjects().forEach(normalizeEntity)
      // 老配置没有 tree_order：按「分组在前、一级项目在后」补一份
      config.value.tree_order = normalizeTreeOrder()
      // 自动选中第一个项目
      if (!selectedProjectId.value) {
        const first = firstProjectInTree()
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
    return rootProjects().find(p => p.id === projectId)
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
    config.value.tree_order = [...(config.value.tree_order ?? defaultTreeOrder()), group.id]
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
    config.value.tree_order = (config.value.tree_order ?? []).filter(id => id !== groupId)
    if (hasSelected) {
      selectedProjectId.value = firstProjectInTree()?.id ?? null
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

  // 一级条目（分组 + 一级项目混排）重新排序
  async function reorderRoot(order: string[]) {
    const alive = new Set<string>([
      ...config.value.groups.map(g => g.id),
      ...rootProjects().map(p => p.id),
    ])
    const seen = new Set<string>()
    const next: string[] = []
    for (const id of order) {
      if (!alive.has(id) || seen.has(id)) continue
      next.push(id)
      seen.add(id)
    }
    for (const id of defaultTreeOrder()) {
      if (seen.has(id)) continue
      next.push(id)
      seen.add(id)
    }
    config.value.tree_order = next
    // 分组数组顺序跟着一级顺序走，数据库的 sort_order 才和界面一致
    const groupById = new Map(config.value.groups.map(g => [g.id, g]))
    const orderedGroups = next
      .map(id => groupById.get(id))
      .filter((g): g is ProjectGroup => Boolean(g))
    for (const group of config.value.groups) {
      if (!orderedGroups.includes(group)) orderedGroups.push(group)
    }
    config.value.groups = orderedGroups
    syncRootProjectOrder()
    await saveConfig()
  }

  // 分组内项目重新排序
  async function reorderProjects(groupId: string, newProjects: ProjectConfig[]) {
    const group = config.value.groups.find(g => g.id === groupId)
    if (!group) return
    group.projects = newProjects
    await saveConfig()
  }

  // 项目移动：分组 ↔ 分组、分组 ↔ 一级、一级内部排序
  //
  // fromGroupId / toGroupId 为 null 表示一级（树根）；toIndex 是「移除源之后」
  // 目标数组里的插入下标（一级项目对应 tree_order 的下标）。
  async function moveProject(
    projectId: string,
    fromGroupId: string | null,
    toGroupId: string | null,
    toIndex: number,
  ) {
    const project = findProject(projectId)
    if (!project) return

    // 先从源位置移除
    if (fromGroupId === null) {
      config.value.projects = rootProjects().filter(p => p.id !== projectId)
      config.value.tree_order = (config.value.tree_order ?? []).filter(id => id !== projectId)
    } else {
      const fromGroup = findGroup(fromGroupId)
      if (!fromGroup) return
      fromGroup.projects = fromGroup.projects.filter(p => p.id !== projectId)
    }

    // 再插入目标位置
    if (toGroupId === null) {
      const order = config.value.tree_order ?? (config.value.tree_order = defaultTreeOrder())
      const pos = Math.max(0, Math.min(toIndex, order.length))
      order.splice(pos, 0, projectId)
      config.value.projects.push(project)
      syncRootProjectOrder()
    } else {
      const toGroup = findGroup(toGroupId)
      if (!toGroup) return
      const pos = Math.max(0, Math.min(toIndex, toGroup.projects.length))
      toGroup.projects.splice(pos, 0, project)
    }
    await saveConfig()
  }

  // 添加项目：groupId 为 null 时加到一级（树根）
  async function addProject(
    groupId: string | null,
    data: Omit<ProjectConfig, 'id' | 'created_at' | 'updated_at' | 'note' | 'attachments'> &
      Partial<Pick<ProjectConfig, 'note' | 'attachments'>>,
  ): Promise<ProjectConfig | undefined> {
    const group = groupId === null ? null : findGroup(groupId)
    if (groupId !== null && !group) return
    const now = nowIso()
    const project: ProjectConfig = {
      note: '',
      attachments: [],
      ...data,
      id: crypto.randomUUID(),
      created_at: now,
      updated_at: now,
    }
    if (group) {
      group.projects.push(project)
    } else {
      config.value.projects.push(project)
      const order = config.value.tree_order ?? normalizeTreeOrder()
      config.value.tree_order = [...order.filter(id => id !== project.id), project.id]
      syncRootProjectOrder()
    }
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

  // 删除项目（分组内或一级都支持）
  async function deleteProject(id: string) {
    const group = findGroupByProject(id)
    if (group) {
      group.projects = group.projects.filter(p => p.id !== id)
    } else {
      config.value.projects = rootProjects().filter(p => p.id !== id)
      config.value.tree_order = (config.value.tree_order ?? []).filter(item => item !== id)
    }
    if (selectedProjectId.value === id) {
      selectedProjectId.value = firstProjectInTree()?.id ?? null
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
    reorderRoot,
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
