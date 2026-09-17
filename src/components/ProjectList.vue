<script setup lang="ts">
import { ref, nextTick } from 'vue'
import type { ProjectGroup, ProjectConfig } from '@/types'

const props = defineProps<{
  groups: ProjectGroup[]
  selectedId: string | null
}>()

const emit = defineEmits<{
  select: [id: string]
  addGroup: []
  deleteGroup: [id: string]
  renameGroup: [id: string, name: string]
  addProject: [groupId: string]
  deleteProject: [id: string]
  reorderGroups: [groups: ProjectGroup[]]
  reorderProjects: [groupId: string, projects: ProjectConfig[]]
  moveProject: [projectId: string, fromGroupId: string, toGroupId: string, toIndex: number]
}>()

// ========== 拖拽状态 ==========
type DragType = 'group' | 'project' | null

const dragType = ref<DragType>(null)
const dragGroupId = ref<string | null>(null)
const dragProjectId = ref<string | null>(null)

// 拖拽悬停指示器位置
const hoverGroupIndex = ref<number | null>(null)
const hoverProjectInfo = ref<{ groupId: string; index: number } | null>(null)

// 每次开始拖拽先清掉上一次残留的悬停位置，避免指示线张冠李戴
function clearHover() {
  hoverGroupIndex.value = null
  hoverProjectInfo.value = null
}

// ========== 分组拖拽 ==========

function onGroupDragStart(e: DragEvent, groupId: string) {
  // 只有从分组标题上发起的拖拽才算分组拖拽。
  // 用「正向判定」而不是「排除项目行」：拖项目时若事件冒泡到这里（或引擎上报的目标
  // 不是标题），这里直接拒绝，绝不会把拖项目变成调分组顺序。
  const target = e.target as HTMLElement | null
  if (!target?.closest?.('.group-header')) return
  clearHover()
  dragType.value = 'group'
  dragGroupId.value = groupId
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData('text/plain', `group:${groupId}`)
  }
}

function onGroupDragOver(e: DragEvent, index: number) {
  if (dragType.value !== 'group') return
  e.preventDefault()
  // 阻止冒泡到 .list-body 的末尾投放区，否则位置会被覆盖成「列表末尾」
  e.stopPropagation()
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = 'move'
  }
  hoverGroupIndex.value = index
}

function onGroupDragLeave() {
  hoverGroupIndex.value = null
}

function onGroupDrop(e: DragEvent, targetIndex: number) {
  e.preventDefault()
  // 防止继续冒泡到 .list-body 的末尾投放区，造成二次处理
  e.stopPropagation()
  if (dragType.value !== 'group' || !dragGroupId.value) return

  const fromIndex = props.groups.findIndex(g => g.id === dragGroupId.value)
  if (fromIndex === -1 || fromIndex === targetIndex) {
    resetDrag()
    return
  }

  const newGroups = [...props.groups]
  const [removed] = newGroups.splice(fromIndex, 1)
  // 指示线画在被悬停分组的「上方」，而源元素移除后其后的索引都会前移一位，
  // 所以向下拖（fromIndex < targetIndex）时要减 1，否则会落到指示线下方一格。
  const actualToIndex = fromIndex < targetIndex ? targetIndex - 1 : targetIndex
  newGroups.splice(actualToIndex, 0, removed)

  emit('reorderGroups', newGroups)
  resetDrag()
}

// 拖到分组列表的空白区域（最后一个分组之后）→ 追加到末尾
function onGroupListDragOver(e: DragEvent) {
  if (dragType.value !== 'group') return
  e.preventDefault()
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = 'move'
  }
  hoverGroupIndex.value = props.groups.length
}

function onGroupListDrop(e: DragEvent) {
  if (dragType.value !== 'group' || !dragGroupId.value) return
  e.preventDefault()

  const fromIndex = props.groups.findIndex(g => g.id === dragGroupId.value)
  if (fromIndex === -1 || fromIndex === props.groups.length - 1) {
    resetDrag()
    return
  }

  const newGroups = [...props.groups]
  const [removed] = newGroups.splice(fromIndex, 1)
  newGroups.push(removed)

  emit('reorderGroups', newGroups)
  resetDrag()
}

// ========== 项目拖拽 ==========

// 声明「这里可以投放」。
// WebKit（Tauri 用的 WKWebView）要求 dragenter 也 preventDefault，否则 drop 不会触发；
// Chromium 只看 dragover，所以只在浏览器里测会漏掉这个问题。
function acceptDrop(e: DragEvent) {
  if (dragType.value !== 'project' && dragType.value !== 'group') return
  e.preventDefault()
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = 'move'
  }
}

function onProjectDragStart(e: DragEvent, projectId: string, groupId: string) {
  // 只有从项目行上发起的拖拽才算项目拖拽（正向判定，见 onGroupDragStart 的说明）
  const target = e.target as HTMLElement | null
  if (!target?.closest?.('.list-item')) return
  // 阻止冒泡：分组也监听 dragstart，别让它把这里的状态覆盖掉
  e.stopPropagation()
  clearHover()
  dragType.value = 'project'
  dragProjectId.value = projectId
  dragGroupId.value = groupId
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData('text/plain', `project:${projectId}:${groupId}`)
  }
}

function onProjectDragOver(e: DragEvent, groupId: string, index: number) {
  if (dragType.value !== 'project') return
  e.preventDefault()
  // 必须阻止冒泡：父级 .group-projects 上还有一个 dragover 处理器，
  // 它会把位置覆盖成「分组末尾」，导致指示线和实际落点对不上。
  e.stopPropagation()
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = 'move'
  }
  hoverProjectInfo.value = { groupId, index }
}

function onProjectDragLeave() {
  // 不立即清除，避免闪烁，由 dragover 更新位置
}

function onProjectDrop(e: DragEvent, toGroupId: string, toIndex: number) {
  // 分组拖拽不在这里处理：不拦截，继续冒泡给分组的落点逻辑
  if (dragType.value !== 'project' || !dragProjectId.value || !dragGroupId.value) return
  e.preventDefault()
  e.stopPropagation()

  const fromGroupId = dragGroupId.value
  const projectId = dragProjectId.value

  // 同一分组内排序
  if (fromGroupId === toGroupId) {
    const group = props.groups.find(g => g.id === toGroupId)
    if (!group) {
      resetDrag()
      return
    }
    const fromIndex = group.projects.findIndex(p => p.id === projectId)
    if (fromIndex === -1 || fromIndex === toIndex) {
      resetDrag()
      return
    }

    const newProjects = [...group.projects]
    const [removed] = newProjects.splice(fromIndex, 1)
    // 计算实际目标位置（删除后索引可能变化）
    const actualToIndex = fromIndex < toIndex ? toIndex - 1 : toIndex
    newProjects.splice(actualToIndex, 0, removed)

    emit('reorderProjects', toGroupId, newProjects)
  } else {
    // 跨分组移动
    emit('moveProject', projectId, fromGroupId, toGroupId, toIndex)
  }

  resetDrag()
}

// 拖到分组区域的空白处或分组头部（追加到该分组末尾）
function onGroupProjectsDrop(e: DragEvent, groupId: string) {
  // 分组拖拽不在这里处理：不拦截，继续冒泡给分组的落点逻辑
  if (dragType.value !== 'project' || !dragProjectId.value || !dragGroupId.value) return
  e.preventDefault()
  e.stopPropagation()

  const group = props.groups.find(g => g.id === groupId)
  if (!group) {
    resetDrag()
    return
  }

  const toIndex = group.projects.length
  const fromGroupId = dragGroupId.value
  const projectId = dragProjectId.value

  if (fromGroupId === groupId) {
    // 同分组拖到末尾
    const fromIndex = group.projects.findIndex(p => p.id === projectId)
    if (fromIndex === -1 || fromIndex === toIndex) {
      resetDrag()
      return
    }
    const newProjects = [...group.projects]
    const [removed] = newProjects.splice(fromIndex, 1)
    newProjects.push(removed)
    emit('reorderProjects', groupId, newProjects)
  } else {
    emit('moveProject', projectId, fromGroupId, groupId, toIndex)
  }

  resetDrag()
}

function onGroupProjectsDragOver(e: DragEvent, groupId: string) {
  if (dragType.value !== 'project') return
  e.preventDefault()
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = 'move'
  }
  const group = props.groups.find(g => g.id === groupId)
  if (group) {
    hoverProjectInfo.value = { groupId, index: group.projects.length }
  }
}

function resetDrag() {
  dragType.value = null
  dragGroupId.value = null
  dragProjectId.value = null
  hoverGroupIndex.value = null
  hoverProjectInfo.value = null
}

function onDragEnd() {
  resetDrag()
}

// 判断是否是当前拖拽的分组
function isDraggingGroup(groupId: string): boolean {
  return dragType.value === 'group' && dragGroupId.value === groupId
}

function isDraggingProject(projectId: string): boolean {
  return dragType.value === 'project' && dragProjectId.value === projectId
}

// 分组悬停指示位置
function isGroupHoverAbove(index: number): boolean {
  return hoverGroupIndex.value === index
}

// 项目悬停指示位置
function isProjectHoverAbove(groupId: string, index: number): boolean {
  return hoverProjectInfo.value?.groupId === groupId && hoverProjectInfo.value?.index === index
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
      <span>项目分组</span>
    </div>
    <div class="list-body" @dragover="onGroupListDragOver" @drop="onGroupListDrop">
      <template v-for="(group, groupIndex) in groups" :key="group.id">
        <!-- 分组上方插入指示线 -->
        <div
          v-if="dragType === 'group' && isGroupHoverAbove(groupIndex)"
          class="group-drop-indicator"
        ></div>

        <div
          class="group-section"
          :class="{ 'dragging': isDraggingGroup(group.id) }"
          @dragenter="acceptDrop"
          @dragover="onGroupDragOver($event, groupIndex)"
          @dragleave="onGroupDragLeave"
          @drop="onGroupDrop($event, groupIndex)"
        >
          <div
            class="group-header"
            :draggable="editingGroupId !== group.id"
            @dragstart="onGroupDragStart($event, group.id)"
            @dragend="onDragEnd"
            @dragenter="acceptDrop"
            @dragover="onGroupProjectsDragOver($event, group.id)"
            @drop="onGroupProjectsDrop($event, group.id)"
          >
            <button
              class="group-toggle"
              :title="isCollapsed(group.id) ? '展开分组' : '收起分组'"
              @click.stop="toggleCollapse(group.id)"
            >{{ isCollapsed(group.id) ? '▸' : '▾' }}</button>
            <span class="group-drag-handle" title="拖拽排序">⋮⋮</span>
            <input
              v-if="editingGroupId === group.id"
              :ref="setRenameInputEl"
              v-model="editingName"
              class="group-name-input"
              @keydown.enter="commitRename(group)"
              @keydown.esc="cancelRename"
              @blur="commitRename(group)"
              @click.stop
              @dblclick.stop
            />
            <span
              v-else
              class="group-name"
              :class="{ 'name-readonly': group.is_default }"
              :title="group.is_default ? '默认分组不可重命名' : '双击重命名'"
              @dblclick="startRename(group)"
            >{{ group.name }}</span>
            <span
              v-if="group.is_default"
              class="group-default-badge"
              title="默认分组不可删除、不可重命名"
            >默认</span>
            <template v-else>
              <button
                class="group-rename"
                @click.stop="startRename(group)"
                title="重命名分组"
              >✎</button>
              <button
                class="group-delete"
                @click.stop="emit('deleteGroup', group.id)"
                title="删除分组"
              >×</button>
            </template>
          </div>
          <div
            v-if="!isCollapsed(group.id)"
            class="group-projects"
            @dragenter="acceptDrop"
            @dragover="onGroupProjectsDragOver($event, group.id)"
            @drop="onGroupProjectsDrop($event, group.id)"
          >
            <template v-for="(project, projIndex) in group.projects" :key="project.id">
              <!-- 项目上方插入指示线 -->
              <div
                v-if="dragType === 'project' && isProjectHoverAbove(group.id, projIndex)"
                class="project-drop-indicator"
              ></div>

              <div
                class="list-item"
                :class="{
                  active: project.id === selectedId,
                  dragging: isDraggingProject(project.id)
                }"
                draggable="true"
                @click="emit('select', project.id)"
                @dragstart="onProjectDragStart($event, project.id, group.id)"
                @dragenter="acceptDrop"
                @dragover="onProjectDragOver($event, group.id, projIndex)"
                @dragleave="onProjectDragLeave"
                @drop="onProjectDrop($event, group.id, projIndex)"
                @dragend="onDragEnd"
              >
                <span class="project-drag-handle" title="拖拽排序/移动">⋮⋮</span>
                <span class="item-name">{{ project.name }}</span>
                <button
                  v-if="project.id === selectedId"
                  class="item-delete"
                  @click.stop="emit('deleteProject', project.id)"
                  title="删除项目"
                >×</button>
              </div>
            </template>

            <button class="add-project-btn" @click="emit('addProject', group.id)">+ 添加项目</button>
          </div>

          <!-- 分组末尾插入指示线（拖到最后一个位置；分组收起时也显示，便于往组里拖） -->
          <div
            v-if="dragType === 'project' && hoverProjectInfo?.groupId === group.id && hoverProjectInfo?.index === group.projects.length && group.projects.length > 0"
            class="project-drop-indicator"
          ></div>
        </div>
      </template>

      <!-- 最后一个分组下方的指示线 -->
      <div
        v-if="dragType === 'group' && hoverGroupIndex === groups.length && groups.length > 0"
        class="group-drop-indicator"
      ></div>

      <div v-if="groups.length === 0" class="empty-tip">
        暂无分组，点击下方新建
      </div>
    </div>
    <button class="add-group-btn" @click="emit('addGroup')">+ 新建分组</button>
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
  padding: 16px;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  border-bottom: 1px solid var(--border-color);
}
.list-body {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
}
.group-section {
  margin-bottom: 8px;
  border-radius: 8px;
  transition: opacity 0.15s;
}
.group-section.dragging {
  opacity: 0.4;
}
.group-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  cursor: grab;
}
.group-header:active {
  cursor: grabbing;
}
.group-drag-handle {
  font-size: 12px;
  color: var(--text-secondary);
  user-select: none;
  flex-shrink: 0;
  letter-spacing: -1px;
}
.group-drag-handle:hover,
.project-drag-handle:hover {
  color: var(--primary);
}
/* 展开/收起分组 */
.group-toggle {
  flex-shrink: 0;
  width: 18px;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1;
  cursor: pointer;
}
.group-toggle:hover {
  color: var(--primary);
}
.group-name {
  flex: 1;
}
.name-readonly {
  cursor: default;
}
.group-name-input {
  flex: 1;
  min-width: 0;
  padding: 2px 4px;
  border: 1px solid var(--primary);
  border-radius: 3px;
  font-size: 12px;
  font-weight: 600;
  font-family: inherit;
  background: var(--bg-input);
  color: var(--text-primary);
  outline: none;
  text-transform: none;
  letter-spacing: normal;
}
.group-rename {
  border: none;
  background: none;
  cursor: pointer;
  color: var(--text-muted);
  font-size: 12px;
  padding: 0;
  line-height: 1;
  flex-shrink: 0;
  transform: scaleX(-1);
}
.group-rename:hover {
  color: var(--primary);
}
.group-default-badge {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 3px;
  color: var(--text-muted);
  border: 1px solid var(--border-color);
  flex-shrink: 0;
  user-select: none;
}
.group-delete {
  border: none;
  background: none;
  cursor: pointer;
  color: var(--text-muted);
  font-size: 16px;
  padding: 0;
  line-height: 1;
}
.group-delete:hover {
  color: var(--danger);
}
.group-projects {
  padding-left: 8px;
  min-height: 10px;
}
.list-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px 8px 4px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-secondary);
  transition: opacity 0.15s;
}
.list-item:hover {
  background: var(--bg-hover);
}
.list-item.active {
  background: var(--bg-active);
  color: var(--text-active);
  font-weight: 500;
}
.list-item.dragging {
  opacity: 0.4;
}
.project-drag-handle {
  font-size: 12px;
  color: var(--text-secondary);
  user-select: none;
  flex-shrink: 0;
  cursor: grab;
  letter-spacing: -1px;
  padding: 0 2px;
}
.project-drag-handle:active {
  cursor: grabbing;
}
.item-name {
  flex: 1;
}
.item-delete {
  border: none;
  background: none;
  cursor: pointer;
  color: var(--text-muted);
  font-size: 18px;
  padding: 0;
}
.item-delete:hover {
  color: var(--danger);
}
.add-project-btn {
  width: 100%;
  padding: 6px;
  border: none;
  border-radius: 6px;
  background: transparent;
  cursor: pointer;
  color: var(--text-muted);
  font-size: 12px;
  text-align: left;
  padding-left: 20px;
}
.add-project-btn:hover {
  color: var(--primary);
  background: var(--bg-hover);
}
.empty-tip {
  padding: 20px;
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
}
.add-group-btn {
  margin: 8px;
  padding: 10px;
  border: 1px dashed var(--border-color);
  border-radius: 6px;
  background: transparent;
  cursor: pointer;
  color: var(--text-secondary);
  font-size: 14px;
}
.add-group-btn:hover {
  border-color: var(--primary);
  color: var(--primary);
}

/* 拖拽插入指示线 */
.group-drop-indicator {
  height: 3px;
  background: var(--primary);
  border-radius: 2px;
  margin: 2px 4px;
  /* 纯视觉元素，不参与事件，避免挡住落点 */
  pointer-events: none;
}
.project-drop-indicator {
  height: 2px;
  background: var(--primary);
  border-radius: 2px;
  margin: 1px 12px;
  /* 纯视觉元素，不参与事件，避免挡住落点 */
  pointer-events: none;
}
</style>
