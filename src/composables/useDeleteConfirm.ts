import { ref } from 'vue'
import { message } from 'ant-design-vue'
import type {
  AttachmentMeta,
  EntityKind,
  ProjectConfig,
  ProjectGroup,
} from '@/types'
import { removeAttachmentFile } from '@/api/attachments'

interface DeleteTarget {
  kind: EntityKind
  id: string
  name: string
  suffix: string
}

interface Options {
  findGroup: (id: string) => ProjectGroup | undefined
  findProject: (id: string) => ProjectConfig | undefined
  deleteGroup: (id: string) => Promise<void>
  deleteProject: (id: string) => Promise<void>
  collectAttachments: (kind: EntityKind, id: string) => AttachmentMeta[]
  ensureSelection: () => void
}

/** 分组/项目删除确认：单次确认 + 可选同时删除附件密文 */
export function useDeleteConfirm(options: Options) {
  const {
    findGroup,
    findProject,
    deleteGroup,
    deleteProject,
    collectAttachments,
    ensureSelection,
  } = options

  const deleteModalOpen = ref(false)
  const deleteWithFiles = ref(false)
  const deleteTarget = ref<DeleteTarget | null>(null)

  function requestDeleteGroup(id: string) {
    const group = findGroup(id)
    if (!group) return
    // 默认分组不允许删除（内部项目仍可单独删除）
    if (group.is_default) {
      message.warning(`默认分组「${group.name}」不可删除`)
      return
    }
    const projectCount = group.projects.length
    deleteTarget.value = {
      kind: 'group',
      id,
      name: group.name,
      suffix: projectCount > 0 ? `（含 ${projectCount} 个项目）` : '',
    }
    deleteWithFiles.value = false
    deleteModalOpen.value = true
  }

  function requestDeleteProject(id: string) {
    const project = findProject(id)
    if (!project) return
    deleteTarget.value = {
      kind: 'project',
      id,
      name: project.name,
      suffix: '',
    }
    deleteWithFiles.value = false
    deleteModalOpen.value = true
  }

  async function confirmDelete() {
    const target = deleteTarget.value
    if (!target) return
    try {
      if (deleteWithFiles.value) {
        // 勾选时逐个删除该实体（分组含其名下项目）的附件密文
        const metas = collectAttachments(target.kind, target.id)
        for (const meta of metas) {
          try {
            await removeAttachmentFile(meta.dir, meta.id)
          } catch (err) {
            message.warning(`删除附件文件失败（${meta.name}）: ${err}`)
          }
        }
      }
      if (target.kind === 'group') {
        await deleteGroup(target.id)
      } else {
        await deleteProject(target.id)
      }
      ensureSelection()
    } finally {
      deleteModalOpen.value = false
      deleteTarget.value = null
    }
  }

  return {
    deleteModalOpen,
    deleteWithFiles,
    deleteTarget,
    requestDeleteGroup,
    requestDeleteProject,
    confirmDelete,
  }
}
