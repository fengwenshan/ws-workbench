<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import type { EntityKind, ProjectGroup, ProjectConfig } from '@/types'

const props = defineProps<{
  groups: ProjectGroup[]
  /** 一级项目（不属于任何分组，和分组混排在树根） */
  projects: ProjectConfig[]
  /** 一级顺序：分组 id 与一级项目 id 的混排顺序 */
  treeOrder: string[]
  selectedId: string | null
}>()

const emit = defineEmits<{
  select: [id: string, kind: EntityKind]
  create: []
  deleteGroup: [id: string]
  renameGroup: [id: string, name: string]
  addProject: [groupId: string]
  deleteProject: [id: string]
  reorderRoot: [order: string[]]
  reorderProjects: [groupId: string, projects: ProjectConfig[]]
  moveProject: [
    projectId: string,
    fromGroupId: string | null,
    toGroupId: string | null,
    toIndex: number,
  ]
}>()

// ========== 一级条目（分组 + 一级项目混排） ==========

type RootItem =
  | { kind: 'group'; group: ProjectGroup }
  | { kind: 'project'; project: ProjectConfig }

function rootItemId(item: RootItem): string {
  return item.kind === 'group' ? item.group.id : item.project.id
}

/** 按 tree_order 混排；缺项时兜底成「分组在前、一级项目在后」 */
const rootItems = computed<RootItem[]>(() => {
  const groupMap = new Map(props.groups.map((g) => [g.id, g]))
  const projectMap = new Map(props.projects.map((p) => [p.id, p]))
  const items: RootItem[] = []
  const seen = new Set<string>()
  for (const id of props.treeOrder) {
    const group = groupMap.get(id)
    if (group) {
      items.push({ kind: 'group', group })
      seen.add(id)
      continue
    }
    const project = projectMap.get(id)
    if (project) {
      items.push({ kind: 'project', project })
      seen.add(id)
    }
  }
  for (const group of props.groups) {
    if (seen.has(group.id)) continue
    items.push({ kind: 'group', group })
    seen.add(group.id)
  }
  for (const project of props.projects) {
    if (seen.has(project.id)) continue
    items.push({ kind: 'project', project })
    seen.add(project.id)
  }
  return items
})

function rootOrderIds(): string[] {
  return rootItems.value.map(rootItemId)
}

// ========== 拖拽排序（指针拖拽 + 实时预览） ==========

type DragKind = 'group' | 'project'
type DropContainer = string | null // null = 一级（树根）

interface PreviewState {
  /** 一级顺序：分组 id + 一级项目 id 混排 */
  root: string[]
  /** 分组 id -> 分组内项目 id 顺序 */
  projects: Record<string, string[]>
}

const dragKind = ref<DragKind | null>(null)
const dragId = ref<string | null>(null)
// 项目拖拽的源容器：null = 一级
const dragFromGroupId = ref<DropContainer>(null)
// 拖动过程中的预览树（不写配置，松手才提交）
const preview = ref<PreviewState | null>(null)

const listBodyEl = ref<HTMLElement | null>(null)
const suppressClick = ref(false)

const groupMap = computed(() => new Map(props.groups.map((g) => [g.id, g])))
const projectMap = computed(() => {
  const map = new Map<string, ProjectConfig>()
  for (const group of props.groups) {
    for (const project of group.projects) map.set(project.id, project)
  }
  for (const project of props.projects) map.set(project.id, project)
  return map
})

type DisplayRootItem =
  | { kind: 'group'; id: string; group: ProjectGroup }
  | { kind: 'project'; id: string; project: ProjectConfig }

/** 拖拽时按预览顺序渲染，否则按配置顺序 */
const displayRootItems = computed<DisplayRootItem[]>(() => {
  const ids = preview.value?.root ?? rootItems.value.map(rootItemId)
  const items: DisplayRootItem[] = []
  for (const id of ids) {
    const group = groupMap.value.get(id)
    if (group) {
      items.push({ kind: 'group', id, group })
      continue
    }
    const project = projectMap.value.get(id)
    if (project) items.push({ kind: 'project', id, project })
  }
  return items
})

function groupProjectsOf(groupId: string): ProjectConfig[] {
  const ids = preview.value?.projects[groupId]
  if (!ids) return groupMap.value.get(groupId)?.projects ?? []
  return ids
    .map((id) => projectMap.value.get(id))
    .filter((project): project is ProjectConfig => Boolean(project))
}

function basePreview(): PreviewState {
  const projects: Record<string, string[]> = {}
  for (const group of props.groups) {
    projects[group.id] = group.projects.map((project) => project.id)
  }
  return { root: rootItems.value.map(rootItemId), projects }
}

/** 找项目当前在预览树里的容器：null = 一级，undefined = 找不到 */
function previewContainerOf(projectId: string): DropContainer | undefined {
  const state = preview.value
  if (!state) return undefined
  if (state.root.includes(projectId)) return null
  for (const [groupId, ids] of Object.entries(state.projects)) {
    if (ids.includes(projectId)) return groupId
  }
  return undefined
}

/** 把被拖条目挪到目标容器/index（index 是「插到第几个之前」） */
function movePreviewTo(container: DropContainer, index: number) {
  const state = preview.value
  const id = dragId.value
  if (!state || !id) return

  if (dragKind.value === 'group') {
    const fromIndex = state.root.indexOf(id)
    if (fromIndex === -1) return
    const sameArray = state.root
    const afterRemoval = sameArray.filter((item) => item !== id)
    const projected = fromIndex < index ? index - 1 : index
    const to = Math.max(0, Math.min(projected, afterRemoval.length))
    if (to === fromIndex) return
    state.root = afterRemoval
    state.root.splice(to, 0, id)
    return
  }

  const current = previewContainerOf(id)
  if (current === undefined) return
  const sameContainer = current === container
  const source = current === null ? state.root : state.projects[current]
  const target = container === null ? state.root : state.projects[container]
  if (!source || !target) return
  const fromIndex = source.indexOf(id)
  if (fromIndex === -1) return

  if (sameContainer) {
    const projected = fromIndex < index ? index - 1 : index
    const to = Math.max(0, Math.min(projected, target.length - 1))
    if (to === fromIndex) return
    source.splice(fromIndex, 1)
    target.splice(to, 0, id)
    return
  }

  const to = Math.max(0, Math.min(index, target.length))
  source.splice(fromIndex, 1)
  target.splice(to, 0, id)
}

function isDraggingGroup(groupId: string): boolean {
  return dragKind.value === 'group' && dragId.value === groupId
}

function isDraggingProject(projectId: string): boolean {
  return dragKind.value === 'project' && dragId.value === projectId
}

let pendingDrag: {
  kind: DragKind
  id: string
  from: DropContainer
  startX: number
  startY: number
} | null = null

function cleanupDragListeners() {
  document.removeEventListener('pointermove', onPointerMove)
  document.removeEventListener('pointerup', onPointerUp)
  document.removeEventListener('pointercancel', onPointerUp)
}

function resetDrag() {
  pendingDrag = null
  dragKind.value = null
  dragId.value = null
  dragFromGroupId.value = null
  preview.value = null
  cleanupDragListeners()
  document.body.style.userSelect = ''
}

function onDragStartPointerDown(
  event: PointerEvent,
  kind: DragKind,
  id: string,
  from: DropContainer,
) {
  if (event.button !== 0) return
  const target = event.target as HTMLElement | null
  if (target?.closest?.('button, input, textarea, select')) return
  pendingDrag = {
    kind,
    id,
    from,
    startX: event.clientX,
    startY: event.clientY,
  }
  document.addEventListener('pointermove', onPointerMove)
  document.addEventListener('pointerup', onPointerUp)
  document.addEventListener('pointercancel', onPointerUp)
}

function beginDrag(pending: NonNullable<typeof pendingDrag>) {
  dragKind.value = pending.kind
  dragId.value = pending.id
  dragFromGroupId.value = pending.from
  preview.value = basePreview()
  document.body.style.userSelect = 'none'
}

function onPointerMove(event: PointerEvent) {
  if (!dragKind.value && pendingDrag) {
    const dx = event.clientX - pendingDrag.startX
    const dy = event.clientY - pendingDrag.startY
    if (Math.hypot(dx, dy) < 4) return
    beginDrag(pendingDrag)
  }
  if (!dragKind.value) return
  event.preventDefault()
  updateDropTarget(event.clientX, event.clientY)
}

function updateDropTarget(x: number, y: number) {
  const state = preview.value
  const body = listBodyEl.value
  if (!state || !body) return
  const bodyRect = body.getBoundingClientRect()
  // 靠近列表上下边缘时自动滚动
  if (y < bodyRect.top + 24) body.scrollTop -= 12
  else if (y > bodyRect.bottom - 24) body.scrollTop += 12

  const el = document.elementFromPoint(x, y) as HTMLElement | null
  if (!el || !body.contains(el)) return

  const itemEl = el.closest<HTMLElement>('[data-drop-item]')
  const groupEl = el.closest<HTMLElement>('[data-drop-group]')

  if (dragKind.value === 'group') {
    // 分组只能在一级排序：以「分组行 / 一级项目行」为参照决定插在哪
    let rootIndex = -1
    let rectEl: HTMLElement | null = null
    if (itemEl?.dataset.dropContainer === 'root') {
      rootIndex = displayRootItems.value.findIndex(
        (item) => item.id === itemEl.dataset.dropItem,
      )
      rectEl = itemEl
    } else if (groupEl) {
      rootIndex = displayRootItems.value.findIndex(
        (item) => item.id === groupEl.dataset.dropGroup,
      )
      rectEl = groupEl
    }
    if (rootIndex === -1 || !rectEl) return
    const rect = rectEl.getBoundingClientRect()
    const index = y > rect.top + rect.height / 2 ? rootIndex + 1 : rootIndex
    movePreviewTo(null, index)
    return
  }

  // 项目拖拽：落在项目行上按上下半区插到前后；落在分组空白处则进该分组末尾
  if (itemEl) {
    const container = itemEl.dataset.dropContainer === 'root'
      ? null
      : (itemEl.dataset.dropContainer ?? null)
    const id = itemEl.dataset.dropItem
    if (!id) return
    const arr = container === null ? state.root : (state.projects[container] ?? [])
    const index = arr.indexOf(id)
    if (index === -1) return
    const rect = itemEl.getBoundingClientRect()
    const insert = y > rect.top + rect.height / 2 ? index + 1 : index
    movePreviewTo(container, insert)
    return
  }
  if (groupEl) {
    const groupId = groupEl.dataset.dropGroup
    if (!groupId) return
    const rootIndex = displayRootItems.value.findIndex(
      (item) => item.id === groupId,
    )
    const rect = groupEl.getBoundingClientRect()
    // 分组上下边缘：按一级位置插到分组前 / 后；中间区域才是「放进分组」
    const edge = Math.min(10, rect.height / 4)
    if (rootIndex !== -1 && y < rect.top + edge) {
      movePreviewTo(null, rootIndex)
      return
    }
    if (rootIndex !== -1 && y > rect.bottom - edge) {
      movePreviewTo(null, rootIndex + 1)
      return
    }
    movePreviewTo(groupId, (state.projects[groupId] ?? []).length)
    return
  }
  movePreviewTo(null, state.root.length)
}

function commitDrag() {
  const state = preview.value
  const kind = dragKind.value
  const id = dragId.value
  if (!state || !kind || !id) {
    resetDrag()
    return
  }

  if (kind === 'group') {
    const original = rootItems.value.map(rootItemId)
    if (state.root.join('\u0000') !== original.join('\u0000')) {
      emit('reorderRoot', [...state.root])
    }
    resetDrag()
    return
  }

  const container = previewContainerOf(id)
  const from = dragFromGroupId.value
  if (container === undefined) {
    resetDrag()
    return
  }

  if (container === from) {
    if (container === null) {
      const original = rootItems.value.map(rootItemId)
      if (state.root.join('\u0000') !== original.join('\u0000')) {
        emit('reorderRoot', [...state.root])
      }
    } else {
      const originalIds = (
        groupMap.value.get(container)?.projects ?? []
      ).map((project) => project.id)
      const projects = groupProjectsOf(container)
      if (projects.map((project) => project.id).join('\u0000') !== originalIds.join('\u0000')) {
        emit('reorderProjects', container, projects)
      }
    }
  } else {
    const arr = container === null ? state.root : (state.projects[container] ?? [])
    const index = arr.indexOf(id)
    if (index !== -1) emit('moveProject', id, from, container, index)
  }
  resetDrag()
}

function onPointerUp() {
  if (dragKind.value) {
    commitDrag()
    suppressClick.value = true
    window.setTimeout(() => {
      suppressClick.value = false
    }, 0)
  } else {
    resetDrag()
  }
}

function handleItemClick(id: string, kind: EntityKind) {
  if (suppressClick.value) return
  emit('select', id, kind)
}

// ========== 分组重命名 ==========
const editingGroupId = ref<string | null>(null)
const editingName = ref('')
const renameInputEl = ref<HTMLInputElement | null>(null)

function setRenameInputEl(el: any) {
  renameInputEl.value = (el as HTMLInputElement) || null
}

async function startRename(group: ProjectGroup) {
  // 默认分组不允许重命名
  if (group.is_default) return
  editingGroupId.value = group.id
  editingName.value = group.name
  await nextTick()
  renameInputEl.value?.focus()
  renameInputEl.value?.select()
}

function commitRename(group: ProjectGroup) {
  if (editingGroupId.value !== group.id) return
  const nextName = editingName.value.trim()
  const original = group.name
  cancelRename()
  if (nextName && nextName !== original) {
    emit('renameGroup', group.id, nextName)
  }
}

function cancelRename() {
  editingGroupId.value = null
  editingName.value = ''
}

// 分组的展开/收起（仅界面状态，不写入配置）
const collapsedGroupIds = ref<string[]>([])

function isCollapsed(groupId: string) {
  return collapsedGroupIds.value.includes(groupId)
}

function toggleCollapse(groupId: string) {
  collapsedGroupIds.value = isCollapsed(groupId)
    ? collapsedGroupIds.value.filter((id) => id !== groupId)
    : [...collapsedGroupIds.value, groupId]
}
</script>

<template>
  <div class="project-list">
    <div class="list-header">
      <span>项目</span>
      <button class="add-group-btn" @click="emit('create')">+ 新建</button>
    </div>
    <div ref="listBodyEl" class="list-body" :class="{ dragging: Boolean(dragKind) }">
      <template v-for="item in displayRootItems" :key="item.id">

        <!-- 分组（不能嵌套分组，只能在一级里排序） -->
        <div
          v-if="item.kind === 'group'"
          class="group-section"
          :class="{ dragging: isDraggingGroup(item.group.id) }"
          :data-drop-group="item.group.id"
          @pointerdown="onDragStartPointerDown($event, 'group', item.group.id, null)"
        >
          <div
            class="group-header"
            :class="{ active: item.group.id === selectedId }"
            @click="handleItemClick(item.group.id, 'group')"
          >
            <button
              class="group-toggle"
              :title="isCollapsed(item.group.id) ? '展开分组' : '收起分组'"
              :aria-expanded="!isCollapsed(item.group.id)"
              @click.stop="toggleCollapse(item.group.id)"
            >
              <svg
                class="group-chevron"
                :class="{ open: !isCollapsed(item.group.id) }"
                viewBox="0 0 12 12"
                aria-hidden="true"
              >
                <path d="M4.5 2.5 8 6l-3.5 3.5" />
              </svg>
            </button>
            <input
              v-if="editingGroupId === item.group.id"
              :ref="setRenameInputEl"
              v-model="editingName"
              class="group-name-input"
              @keydown.enter="commitRename(item.group)"
              @keydown.esc="cancelRename"
              @blur="commitRename(item.group)"
              @click.stop
              @dblclick.stop
            />
            <span
              v-else
              class="group-name"
              :class="{ 'name-readonly': item.group.is_default }"
              :title="item.group.is_default ? '默认分组不可重命名' : '双击重命名'"
              @dblclick="startRename(item.group)"
            >{{ item.group.name }}</span>
            <span
              v-if="item.group.is_default"
              class="group-default-badge"
              title="默认分组不可删除、不可重命名"
            >默认</span>
            <template v-else>
              <button
                class="group-rename"
                @click.stop="startRename(item.group)"
                title="重命名分组"
                aria-label="重命名分组"
              >
                <svg viewBox="64 64 896 896" aria-hidden="true">
                  <path
                    d="M257.7 752c2 0 4-.2 6-.5L431.9 722c2-.4 3.9-1.3 5.3-2.8l423.9-423.9a9.96 9.96 0 000-14.1L694.9 114.9c-1.9-1.9-4.4-2.9-7.1-2.9s-5.2 1-7.1 2.9L256.8 538.8c-1.5 1.5-2.4 3.3-2.8 5.3l-29.5 168.2a33.5 33.5 0 009.4 29.8c6.6 6.4 14.9 9.9 23.8 9.9zm67.4-174.4L687.8 215l73.3 73.3-362.7 362.6-88.9 15.7 15.6-89zM880 836H144c-17.7 0-32 14.3-32 32v36c0 4.4 3.6 8 8 8h784c4.4 0 8-3.6 8-8v-36c0-17.7-14.3-32-32-32z"
                  />
                </svg>
              </button>
              <button
                class="group-delete"
                @click.stop="emit('deleteGroup', item.group.id)"
                title="删除分组"
                aria-label="删除分组"
              >
                <svg viewBox="64 64 896 896" aria-hidden="true">
                  <path
                    d="M799.86 166.31c.02 0 .04.02.08.06l57.69 57.7c.04.03.05.05.06.08a.12.12 0 010 .06c0 .03-.02.05-.06.09L569.93 512l287.7 287.7c.04.04.05.06.06.09a.12.12 0 010 .07c0 .02-.02.04-.06.08l-57.7 57.69c-.03.04-.05.05-.07.06a.12.12 0 01-.07 0c-.03 0-.05-.02-.09-.06L512 569.93l-287.7 287.7c-.04.04-.06.05-.09.06a.12.12 0 01-.07 0c-.02 0-.04-.02-.08-.06l-57.69-57.7c-.04-.03-.05-.05-.06-.07a.12.12 0 010-.07c0-.03.02-.05.06-.09L454.07 512l-287.7-287.7c-.04-.04-.05-.06-.06-.09a.12.12 0 010-.07c0-.02.02-.04.06-.08l57.7-57.69c.03-.04.05-.05.07-.06a.12.12 0 01.07 0c.03 0 .05.02.09.06L512 454.07l287.7-287.7c.04-.04.06-.05.09-.06a.12.12 0 01.07 0z"
                  />
                </svg>
              </button>
            </template>
          </div>
          <div v-if="!isCollapsed(item.group.id)" class="group-projects">
            <template v-for="project in groupProjectsOf(item.group.id)" :key="project.id">
              <div
                class="list-item"
                :class="{
                  active: project.id === selectedId,
                  dragging: isDraggingProject(project.id)
                }"
                :data-drop-item="project.id"
                :data-drop-container="item.group.id"
                @click="handleItemClick(project.id, 'project')"
                @pointerdown.stop="
                  onDragStartPointerDown($event, 'project', project.id, item.group.id)
                "
              >
                <span class="item-name">{{ project.name }}</span>
                <button
                  class="item-delete"
                  @click.stop="emit('deleteProject', project.id)"
                  title="删除项目"
                  aria-label="删除项目"
                >
                  <svg viewBox="64 64 896 896" aria-hidden="true">
                    <path
                      d="M799.86 166.31c.02 0 .04.02.08.06l57.69 57.7c.04.03.05.05.06.08a.12.12 0 010 .06c0 .03-.02.05-.06.09L569.93 512l287.7 287.7c.04.04.05.06.06.09a.12.12 0 010 .07c0 .02-.02.04-.06.08l-57.7 57.69c-.03.04-.05.05-.07.06a.12.12 0 01-.07 0c-.03 0-.05-.02-.09-.06L512 569.93l-287.7 287.7c-.04.04-.06.05-.09.06a.12.12 0 01-.07 0c-.02 0-.04-.02-.08-.06l-57.69-57.7c-.04-.03-.05-.05-.06-.07a.12.12 0 010-.07c0-.03.02-.05.06-.09L454.07 512l-287.7-287.7c-.04-.04-.05-.06-.06-.09a.12.12 0 010-.07c0-.02.02-.04.06-.08l57.7-57.69c.03-.04.05-.05.07-.06a.12.12 0 01.07 0c.03 0 .05.02.09.06L512 454.07l287.7-287.7c.04-.04.06-.05.09-.06a.12.12 0 01.07 0z"
                    />
                  </svg>
                </button>
              </div>
            </template>

            <button class="add-project-btn" @click="emit('addProject', item.group.id)">+ 添加项目</button>
          </div>

        </div>

        <!-- 一级项目（像文件管理器里根目录下的文件） -->
        <div
          v-else
          class="list-item root-item"
          :class="{
            active: item.project.id === selectedId,
            dragging: isDraggingProject(item.project.id)
          }"
          :data-drop-item="item.project.id"
          data-drop-container="root"
          @click="handleItemClick(item.project.id, 'project')"
          @pointerdown.stop="onDragStartPointerDown($event, 'project', item.project.id, null)"
        >
          <span class="item-name">{{ item.project.name }}</span>
          <button
            class="item-delete"
            @click.stop="emit('deleteProject', item.project.id)"
            title="删除项目"
            aria-label="删除项目"
          >
            <svg viewBox="64 64 896 896" aria-hidden="true">
              <path
                d="M799.86 166.31c.02 0 .04.02.08.06l57.69 57.7c.04.03.05.05.06.08a.12.12 0 010 .06c0 .03-.02.05-.06.09L569.93 512l287.7 287.7c.04.04.05.06.06.09a.12.12 0 010 .07c0 .02-.02.04-.06.08l-57.7 57.69c-.03.04-.05.05-.07.06a.12.12 0 01-.07 0c-.03 0-.05-.02-.09-.06L512 569.93l-287.7 287.7c-.04.04-.06.05-.09.06a.12.12 0 01-.07 0c-.02 0-.04-.02-.08-.06l-57.69-57.7c-.04-.03-.05-.05-.06-.07a.12.12 0 010-.07c0-.03.02-.05.06-.09L454.07 512l-287.7-287.7c-.04-.04-.05-.06-.06-.09a.12.12 0 010-.07c0-.02.02-.04.06-.08l57.7-57.69c.03-.04.05-.05.07-.06a.12.12 0 01.07 0c.03 0 .05.02.09.06L512 454.07l287.7-287.7c.04-.04.06-.05.09-.06a.12.12 0 01.07 0z"
              />
            </svg>
          </button>
        </div>
      </template>

      <div v-if="displayRootItems.length === 0" class="empty-tip">
        暂无项目，点击右上角「+ 新建」
      </div>
    </div>
  </div>
</template>

<style scoped>
.project-list {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}
.list-header {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  height: var(--panel-header-height);
  padding: 0 16px;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.2px;
  color: #000;
  border-bottom: 1px solid var(--border-color);
}
.list-body {
  flex: 1;
  overflow-y: auto;
  padding: 6px 8px 8px;
}
.group-section {
  margin-bottom: 4px;
  border-radius: 6px;
  transition: opacity 0.15s;
}
.group-section.dragging .group-header,
.list-item.dragging {
  opacity: 0.45;
}
.list-body.dragging {
  user-select: none;
}
.group-header {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  padding: 0 6px;
  border-radius: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
  user-select: none;
  cursor: default;
  transition: background 0.12s, color 0.12s;
}
.group-header:hover {
  background: var(--bg-hover);
}
.group-header.active {
  background: var(--bg-active);
  color: var(--text-active);
}
/* 展开/收起分组：用细线箭头，展开时旋转 90°，比 ▸/▾ 更贴合中文排版 */
.group-toggle {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 16px;
  height: 16px;
  padding: 0;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: color 0.12s;
}
.group-toggle:hover {
  color: var(--primary);
}
.group-toggle:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: 0;
}
.group-chevron {
  width: 12px;
  height: 12px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.6;
  stroke-linecap: round;
  stroke-linejoin: round;
  transition: transform 0.15s ease;
}
.group-chevron.open {
  transform: rotate(90deg);
}
.group-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.name-readonly {
  cursor: default;
}
.group-name-input {
  flex: 1;
  min-width: 0;
  height: 22px;
  padding: 2px 4px;
  border: 1px solid var(--primary);
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
  font-family: inherit;
  background: var(--bg-input);
  color: var(--text-primary);
  outline: none;
  letter-spacing: normal;
}
.group-default-badge {
  flex-shrink: 0;
  height: 16px;
  padding: 0 6px;
  border-radius: 4px;
  background: var(--bg-tag);
  color: var(--text-muted);
  font-size: 10px;
  font-weight: 500;
  line-height: 16px;
  user-select: none;
}
.group-rename,
.group-delete {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.12s, background 0.12s, color 0.12s;
}
.group-header:hover .group-rename,
.group-header:hover .group-delete,
.group-header:focus-within .group-rename,
.group-header:focus-within .group-delete {
  opacity: 1;
  pointer-events: auto;
}
.group-rename svg,
.group-delete svg,
.item-delete svg {
  width: 14px;
  height: 14px;
  fill: currentColor;
}
.group-rename:hover {
  background: var(--bg-tag);
  color: var(--text-primary);
}
.group-delete:hover {
  background: rgba(239, 68, 68, 0.12);
  color: var(--danger);
}
.group-rename:focus-visible,
.group-delete:focus-visible {
  outline: 1px solid var(--primary);
}
/* 项目列表：左侧一条细竖线表达层级，行高统一，长名称省略 */
.group-projects {
  margin-left: 12px;
  padding: 0 0 2px 8px;
  border-left: 1px solid var(--border-color);
}
.list-item {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 30px;
  padding: 0 6px 0 16px;
  border-radius: 6px;
  font-size: 14px;
  color: var(--text-secondary);
  user-select: none;
  cursor: pointer;
  transition: background 0.12s, color 0.12s, box-shadow 0.12s;
}
.list-item:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.list-item.active {
  background: var(--bg-active);
  color: var(--text-active);
  font-weight: 500;
  box-shadow: inset 0 0 0 1px rgba(59, 130, 246, 0.18);
}
.list-item.dragging {
  opacity: 0.4;
}
.item-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.item-delete {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-muted);
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.12s, background 0.12s, color 0.12s;
}
.list-item:hover .item-delete,
.list-item.active .item-delete,
.list-item:focus-within .item-delete {
  opacity: 1;
  pointer-events: auto;
}
.item-delete:hover {
  background: rgba(239, 68, 68, 0.12);
  color: var(--danger);
}
.item-delete:focus-visible {
  outline: 1px solid var(--primary);
}
.add-project-btn {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  height: 28px;
  margin: 2px 0;
  padding: 0 6px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
  opacity: 0.75;
  transition: opacity 0.12s, background 0.12s, color 0.12s;
}
.add-project-btn:hover {
  opacity: 1;
  background: var(--bg-hover);
  color: var(--primary);
}
.empty-tip {
  padding: 18px 12px;
  text-align: center;
  color: var(--text-muted);
  font-size: 12px;
}
.add-group-btn {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  margin-left: auto;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  font-weight: 500;
  line-height: 1;
  cursor: pointer;
}
.add-group-btn:hover {
  color: var(--primary-hover);
  text-decoration: underline;
  text-underline-offset: 2px;
}
.add-group-btn:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: 1px;
}

/* 根级项目行：和分组标题的名称列对齐，像文件管理器里的文件 */
.root-item {
  height: 28px;
  padding: 0 6px 0 26px;
  margin-bottom: 4px;
}

</style>
