import { computed, ref, watch, type Ref } from 'vue'
import type {
  AppConfig,
  EntityKind,
  ProjectConfig,
  ProjectGroup,
  Selection,
} from '@/types'

/** 项目树选中状态：分组/项目二选一，驱动工作区详情路由 */
export function useEntitySelection(
  config: Ref<AppConfig>,
  loading: Ref<boolean>,
) {
  const selection = ref<Selection | null>(null)

  const selectedId = computed(() => selection.value?.id ?? null)

  const selectedGroup = computed<ProjectGroup | null>(() => {
    const sel = selection.value
    if (!sel || sel.kind !== 'group') return null
    return config.value.groups.find((group) => group.id === sel.id) ?? null
  })

  const selectedProject = computed<ProjectConfig | null>(() => {
    const sel = selection.value
    if (!sel || sel.kind !== 'project') return null
    for (const group of config.value.groups) {
      const project = group.projects.find((item) => item.id === sel.id)
      if (project) return project
    }
    return null
  })

  /** 选中项失效（删除/加载完成）时回落到第一个项目 */
  function ensureSelection() {
    const current = selection.value
    if (current) {
      const alive =
        current.kind === 'group'
          ? config.value.groups.some((group) => group.id === current.id)
          : config.value.groups.some((group) =>
              group.projects.some((project) => project.id === current.id),
            )
      if (alive) return
    }
    const first = config.value.groups
      .flatMap((group) => group.projects)
      .find(Boolean)
    selection.value = first ? { kind: 'project', id: first.id } : null
  }

  watch(
    loading,
    (value) => {
      if (!value) ensureSelection()
    },
    { immediate: true },
  )

  function handleSelect(id: string, kind: EntityKind) {
    selection.value = { kind, id }
  }

  return {
    selection,
    selectedId,
    selectedGroup,
    selectedProject,
    ensureSelection,
    handleSelect,
  }
}
