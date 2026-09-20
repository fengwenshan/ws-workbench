<script setup lang="ts">
import { ref, nextTick, onUnmounted } from 'vue'
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
  toggleGroup: [id: string]
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

// 指示线只有一条，且用绝对定位绘制：它不参与布局，拖拽过程中列表不会被撑动，
// 也就不会出现「指示线撑开布局 → 指针落到别的元素 → 指示线跳走 → 布局回弹」的高频抖动。
const listBodyEl = ref<HTMLElement | null>(null)
const dropLineY = ref<number | null>(null)
// 分组落点：0..groups.length，含义是「插到第 index 个分组之前」
const dropGroupIndex = ref<number | null>(null)
// 项目落点：目标分组 + 组内插入位置
const dropProjectTarget = ref<{ groupId: string; index: number } | null>(null)

// ========== 落点计算 ==========
// 落点只由「指针坐标 + 各行静止时的位置」算出来，不再依赖 dragenter/dragleave。
// 旧实现是给每个分组/项目挂 dragover+dragleave：指针在子元素之间移动会不停触发
// dragleave 把位置清空，父容器又把它设成「列表末尾」，指示线于是来回跳；再加上
// 指示线本身是文档流里的元素、出现时会撑开列表，指针底下的元素跟着变，抖动就被放大。

/** 视口坐标 → .list-body 内容坐标（绝对定位指示线用；含滚动偏移） */
function toContentY(clientY: number): number {
  const body = listBodyEl.value
  if (!body) return 0
  return clientY - body.getBoundingClientRect().top + body.scrollTop
}

/** 当前渲染出的分组区块，顺序与 props.groups 一致 */
function groupSections(): HTMLElement[] {
  const body = listBodyEl.value
  if (!body) return []
  return Array.from(body.querySelectorAll<HTMLElement>(':scope > .group-section'))
}

function projectItemsOf(section: HTMLElement): HTMLElement[] {
  return Array.from(section.querySelectorAll<HTMLElement>('.group-projects > .list-item'))
}

/**
 * 按「中线」定插入位置：指针在某行上半部分就插到它前面，下半部分就插到它后面。
 * 纯函数式的判断，同一位置反复计算的结果恒定，所以不会抖。
 */
function insertionIndex(rects: DOMRect[], clientY: number): number {
  for (let i = 0; i < rects.length; i++) {
    if (clientY < rects[i].top + rects[i].height / 2) return i
  }
  return rects.length
}

function clearDropTarget() {
  dropLineY.value = null
  dropGroupIndex.value = null
  dropProjectTarget.value = null
}

// ========== 分组拖拽 ==========

function onGroupDragStart(e: DragEvent, groupId: string) {
  // 项目自身的 dragstart 会冒泡到这里，那种情况属于项目拖拽，不能改判
  if (e.target !== e.currentTarget) return
  dragType.value = 'group'
  dragGroupId.value = groupId
  // 明确标记「这次按下变成了拖拽」，供分组名的 click 判断（见 isDragNotClick）
  draggingSincePress = true
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData('text/plain', `group:${groupId}`)
  }
}

/** 分组拖拽：落点 = 第 dropGroupIndex 个分组的「上方」 */
function updateGroupDropTarget(clientY: number) {
  const sections = groupSections()
  if (sections.length === 0) {
    clearDropTarget()
    return
  }
  const rects = sections.map((el) => el.getBoundingClientRect())
  const index = insertionIndex(rects, clientY)
  dropGroupIndex.value = index
  dropProjectTarget.value = null
  const lineY = index < rects.length ? rects[index].top : rects[rects.length - 1].bottom
  dropLineY.value = toContentY(lineY)
}

// ========== 项目拖拽 ==========

function onProjectDragStart(e: DragEvent, projectId: string, groupId: string) {
  // 必须阻止冒泡：外层 .group-section 也是 draggable，
  // 否则它的 dragstart 处理器随后会把这次拖拽改判成「拖分组」。
  e.stopPropagation()
  dragType.value = 'project'
  dragProjectId.value = projectId
  dragGroupId.value = groupId
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData('text/plain', `project:${projectId}:${groupId}`)
  }
}

/** 项目拖拽：先判断指针落在哪个分组内（含分组标题），再在组内按中线定位 */
function updateProjectDropTarget(clientY: number) {
  const sections = groupSections()
  if (sections.length === 0) {
    clearDropTarget()
    return
  }

  let sectionIndex = sections.length - 1
  for (let i = 0; i < sections.length; i++) {
    const r = sections[i].getBoundingClientRect()
    if (clientY >= r.top && clientY <= r.bottom) {
      sectionIndex = i
      break
    }
  }

  const section = sections[sectionIndex]
  const group = props.groups[sectionIndex]
  if (!section || !group) {
    clearDropTarget()
    return
  }

  dropGroupIndex.value = null

  // 折叠分组里看不到项目，落点固定为「追加到该分组末尾」，指示线画在分组标题下方
  if (group.collapsed) {
    dropProjectTarget.value = { groupId: group.id, index: group.projects.length }
    dropLineY.value = toContentY(section.getBoundingClientRect().bottom - 2)
    return
  }

  // 空分组：插到第一个位置
  if (group.projects.length === 0) {
    const anchor = section.querySelector<HTMLElement>('.group-projects') ?? section
    dropProjectTarget.value = { groupId: group.id, index: 0 }
    dropLineY.value = toContentY(anchor.getBoundingClientRect().top + 4)
    return
  }

  const rects = projectItemsOf(section).map((el) => el.getBoundingClientRect())
  const index = insertionIndex(rects, clientY)
  dropProjectTarget.value = { groupId: group.id, index }
  const lineY = index < rects.length ? rects[index].top : rects[rects.length - 1].bottom
  dropLineY.value = toContentY(lineY)
}

// ========== 列表容器上的统一拖拽处理 ==========
// 监听挂在容器上而不是每个条目上：dragover 由容器连续收到，落点每次重算，
// 不再有「进入子元素触发 dragleave 清空 → 父容器又设成列表末尾」的来回跳。

/**
 * 声明「这里可以接收投放」。
 *
 * WebKit（macOS 上 Tauri 用的 WKWebView）要求 dragenter 也 preventDefault，
 * 否则 drop 根本不会触发；Chromium/WebView2 只看 dragover。
 * main 侧 5f55a70 就是为这个问题加的 acceptDrop，合并时不能丢——
 * 本实现把投放处理统一收到了 .list-body 上，所以这份「接受」也要挂在容器上。
 */
function acceptDrop(e: DragEvent) {
  if (!dragType.value) return
  e.preventDefault()
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = 'move'
  }
}

function onListDragOver(e: DragEvent) {
  if (!dragType.value) return
  // 不阻止默认行为就不会触发 drop
  e.preventDefault()
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = 'move'
  }
  if (dragType.value === 'group') updateGroupDropTarget(e.clientY)
  else updateProjectDropTarget(e.clientY)
}

function onListDrop(e: DragEvent) {
  if (!dragType.value) return
  e.preventDefault()
  if (dragType.value === 'group') {
    applyGroupReorder(dropGroupIndex.value)
  } else {
    applyProjectMove(dropProjectTarget.value)
  }
  resetDrag()
}

/** 把正在拖的分组插到 targetIndex 之前（取值 0..groups.length） */
function applyGroupReorder(targetIndex: number | null) {
  const id = dragGroupId.value
  if (targetIndex === null || !id) return

  const fromIndex = props.groups.findIndex((g) => g.id === id)
  if (fromIndex === -1) return
  // 插到自己前面、或紧跟在自己后面，都等于没动
  if (targetIndex === fromIndex || targetIndex === fromIndex + 1) return

  const newGroups = [...props.groups]
  const [removed] = newGroups.splice(fromIndex, 1)
  // 指示线画在目标分组「上方」，而源元素移除后其后的索引都会前移一位，
  // 所以向下拖（fromIndex < targetIndex）时要减 1，否则会落到指示线下方一格。
  const actualToIndex = fromIndex < targetIndex ? targetIndex - 1 : targetIndex
  newGroups.splice(actualToIndex, 0, removed)
  emit('reorderGroups', newGroups)
}

/** 把正在拖的项目放到目标分组的指定位置 */
function applyProjectMove(target: { groupId: string; index: number } | null) {
  const projectId = dragProjectId.value
  const fromGroupId = dragGroupId.value
  if (!target || !projectId || !fromGroupId) return

  const group = props.groups.find((g) => g.id === target.groupId)
  if (!group) return

  if (fromGroupId !== target.groupId) {
    // 跨分组：目标组里本来没有这个项目，索引可直接使用
    emit('moveProject', projectId, fromGroupId, target.groupId, target.index)
    return
  }

  const fromIndex = group.projects.findIndex((p) => p.id === projectId)
  if (fromIndex === -1) return
  // 插到自己前面、或紧跟在自己后面，都等于没动
  if (target.index === fromIndex || target.index === fromIndex + 1) return

  const newProjects = [...group.projects]
  const [removed] = newProjects.splice(fromIndex, 1)
  const actualToIndex = fromIndex < target.index ? target.index - 1 : target.index
  newProjects.splice(actualToIndex, 0, removed)
  emit('reorderProjects', target.groupId, newProjects)
}

function resetDrag() {
  dragType.value = null
  dragGroupId.value = null
  dragProjectId.value = null
  clearDropTarget()
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

// ========== 分组展开/折叠 ==========
// 分组名同时承担两个手势：单击展开/折叠、双击重命名。
// 单击先延迟一小会儿再生效，若期间来了双击就取消，这样重命名时不会先折叠再展开。
const TOGGLE_DELAY = 200
let pendingToggle: number | null = null
// 本次「按下」是否已经变成了一次拖拽。
// 注意：drop 早于 dragend 触发，onListDrop 里会 resetDrag()，
// 所以不能在 dragend 里判断「刚才是不是在拖分组」（那时 dragType 已经被清空），
// 必须在 dragstart 这个确定的时刻置位，并在每次按下时重置。
let draggingSincePress = false
let pressPoint: { x: number; y: number } | null = null

function cancelPendingToggle() {
  if (pendingToggle !== null) {
    window.clearTimeout(pendingToggle)
    pendingToggle = null
  }
}

function onGroupToggleMouseDown(e: MouseEvent) {
  draggingSincePress = false
  pressPoint = { x: e.clientX, y: e.clientY }
}

// 拖拽排序后浏览器仍可能补发一次 click；按下后有明显位移的拖拽尝试同理。
// 两者都不应该被当成「点击折叠」。
function isDragNotClick(e: MouseEvent): boolean {
  if (draggingSincePress) return true
  if (!pressPoint) return false
  const moved = Math.abs(e.clientX - pressPoint.x) + Math.abs(e.clientY - pressPoint.y)
  return moved > 6
}

function onGroupNameClick(e: MouseEvent, group: ProjectGroup) {
  if (isDragNotClick(e)) return
  cancelPendingToggle()
  pendingToggle = window.setTimeout(() => {
    pendingToggle = null
    emit('toggleGroup', group.id)
  }, TOGGLE_DELAY)
}

// 箭头不做延迟，点一下就切
function onGroupCaretClick(e: MouseEvent, group: ProjectGroup) {
  if (isDragNotClick(e)) return
  cancelPendingToggle()
  emit('toggleGroup', group.id)
}

function onGroupNameDblClick(group: ProjectGroup) {
  cancelPendingToggle()
  startRename(group)
}

// 悬停提示：默认分组不可重命名，所以只说折叠/展开
function groupNameTitle(group: ProjectGroup): string {
  const action = group.collapsed ? '点击展开' : '点击折叠'
  return group.is_default ? action : `${action}，双击重命名`
}

onUnmounted(cancelPendingToggle)

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
</script>

<template>
  <div class="project-list">
    <div class="list-header">
      <span>项目分组</span>
    </div>
    <div
      ref="listBodyEl"
      class="list-body"
      @dragenter="acceptDrop"
      @dragover="onListDragOver"
      @drop="onListDrop"
    >
      <template v-for="group in groups" :key="group.id">
        <div
          class="group-section"
          :class="{ 'dragging': isDraggingGroup(group.id) }"
          :draggable="editingGroupId !== group.id"
          @dragenter="acceptDrop"
          @dragstart="onGroupDragStart($event, group.id)"
          @dragend="onDragEnd"
        >
          <div class="group-header">
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
            <template v-else>
              <!-- 折叠箭头：单击立即切换，并指示当前展开状态 -->
              <span
                class="group-caret"
                :title="group.collapsed ? '点击展开' : '点击折叠'"
                @mousedown="onGroupToggleMouseDown"
                @click.stop="onGroupCaretClick($event, group)"
              >{{ group.collapsed ? '▸' : '▾' }}</span>
              <span
                class="group-name"
                :title="groupNameTitle(group)"
                @mousedown="onGroupToggleMouseDown"
                @click.stop="onGroupNameClick($event, group)"
                @dblclick.stop="onGroupNameDblClick(group)"
              >{{ group.name }}</span>
              <!-- 折叠时给出项目数量，避免看不出里面还有内容 -->
              <span
                v-if="group.collapsed && group.projects.length"
                class="group-count"
                :title="`${group.projects.length} 个项目`"
              >{{ group.projects.length }}</span>
            </template>
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
          <div v-if="!group.collapsed" class="group-projects">
            <div
              v-for="project in group.projects"
              :key="project.id"
              class="list-item"
              :class="{
                active: project.id === selectedId,
                dragging: isDraggingProject(project.id)
              }"
              draggable="true"
              @click="emit('select', project.id)"
              @dragstart="onProjectDragStart($event, project.id, group.id)"
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

            <button class="add-project-btn" @click="emit('addProject', group.id)">+ 添加项目</button>
          </div>
        </div>
      </template>

      <div v-if="groups.length === 0" class="empty-tip">
        暂无分组，点击下方新建
      </div>

      <!-- 全列表唯一的插入指示线：绝对定位、不参与布局，拖拽时列表不会被撑动 -->
      <div
        v-if="dropLineY !== null"
        class="drop-line"
        :style="{ top: dropLineY + 'px' }"
      ></div>
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
  position: relative; /* 插入指示线的定位参照 */
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
  font-size: 10px;
  color: var(--text-muted);
  opacity: 0.6;
  user-select: none;
  flex-shrink: 0;
  letter-spacing: -1px;
}
.group-caret {
  flex-shrink: 0;
  width: 12px;
  font-size: 10px;
  line-height: 1;
  text-align: center;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
}
.group-caret:hover {
  color: var(--primary);
}
.group-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}
.group-name:hover {
  color: var(--text-secondary);
}
/* 折叠后藏起来的项目数量 */
.group-count {
  flex-shrink: 0;
  padding: 0 5px;
  border-radius: 8px;
  background: var(--bg-tag);
  color: var(--text-muted);
  font-size: 10px;
  line-height: 14px;
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
  font-size: 10px;
  color: var(--text-muted);
  opacity: 0.6;
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

/* 拖拽插入指示线：绝对定位，不占布局、不接收指针事件。
   这样它既不会把列表撑动（撑动会让指针底下的元素变来变去，产生抖动），
   也不会自己成为 dragover 的目标。 */
.drop-line {
  position: absolute;
  left: 12px;
  right: 12px;
  height: 2px;
  background: var(--primary);
  border-radius: 2px;
  pointer-events: none;
}
/* 左端小圆点，让落点更醒目 */
.drop-line::before {
  content: '';
  position: absolute;
  left: -3px;
  top: -2px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--primary);
}
</style>
