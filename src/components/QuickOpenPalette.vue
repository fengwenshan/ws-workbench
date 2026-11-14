<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import type {
  QuickAttachmentItem,
  QuickPaletteChoose,
  QuickProjectItem,
} from '@/types'

/**
 * VS Code 式快速搜索：居中悬浮在窗口上方，输入即筛选，↑↓ 选择、回车打开、Esc 关闭。
 *
 * 搜索范围：所有分组/项目的附件；输入关键字后还会搜项目本身（名称、分组路径、
 * 识别到的 Git / SVN 地址，全部按「包含」匹配，支持局部关键字）。
 * 打开动作由父组件决定：附件走预览，项目跳转到它所在的页面。
 */
const props = withDefaults(
  defineProps<{
    items: QuickAttachmentItem[]
    projects?: QuickProjectItem[]
  }>(),
  { projects: () => [] },
)

const emit = defineEmits<{
  close: []
  choose: [value: QuickPaletteChoose]
}>()

type Entry =
  | { kind: 'attachment'; attachment: QuickAttachmentItem }
  | { kind: 'project'; project: QuickProjectItem }

const query = ref('')
const selected = ref(0)
const inputEl = ref<HTMLInputElement | null>(null)

function vcsLabel(type: string): string {
  if (type === 'git') return 'Git'
  if (type === 'svn') return 'SVN'
  return ''
}

function matchAttachment(item: QuickAttachmentItem, keyword: string): boolean {
  return (
    item.name.toLowerCase().includes(keyword) ||
    item.id.toLowerCase().includes(keyword) ||
    item.entityPath.toLowerCase().includes(keyword)
  )
}

function matchProject(project: QuickProjectItem, keyword: string): boolean {
  return (
    project.name.toLowerCase().includes(keyword) ||
    project.path.toLowerCase().includes(keyword) ||
    project.vcsUrl.toLowerCase().includes(keyword) ||
    vcsLabel(project.vcsType).toLowerCase().includes(keyword)
  )
}

/**
 * 空关键字时保持老行为，只列附件；有关键字时才混入项目，
 * 避免每次打开搜索都先看到一串项目。
 */
/** 当前关键字（小写、已 trim），匹配和命中高亮共用 */
const keyword = computed(() => query.value.trim().toLowerCase())

/** 把文本按关键字切成「命中 / 未命中」片段，用于结果里高亮，一眼看到命中在哪 */
interface MatchSegment {
  text: string
  hit: boolean
}

function splitMatch(text: string, term: string): MatchSegment[] {
  if (!term) return [{ text, hit: false }]
  const lower = text.toLowerCase()
  const segments: MatchSegment[] = []
  let from = 0
  let at = lower.indexOf(term)
  while (at !== -1) {
    if (at > from) segments.push({ text: text.slice(from, at), hit: false })
    segments.push({ text: text.slice(at, at + term.length), hit: true })
    from = at + term.length
    at = lower.indexOf(term, from)
  }
  if (from < text.length || segments.length === 0) {
    segments.push({ text: text.slice(from), hit: false })
  }
  return segments
}

const visibleEntries = computed<Entry[]>(() => {
  const term = keyword.value
  const out: Entry[] = []
  for (const item of props.items) {
    if (!term || matchAttachment(item, term)) {
      out.push({ kind: 'attachment', attachment: item })
    }
  }
  if (term) {
    for (const project of props.projects ?? []) {
      if (matchProject(project, term)) out.push({ kind: 'project', project })
    }
  }
  return out
})

// 关键字变化后高亮回到第一条，避免选中项落在被过滤掉的位置；
// 项目地址是异步识别的，结果回来导致列表变化时不抢用户已经选中的行
watch(query, () => {
  selected.value = 0
})
watch(visibleEntries, (entries) => {
  if (selected.value >= entries.length) selected.value = 0
})

onMounted(async () => {
  await nextTick()
  inputEl.value?.focus()
})

function choose(entry: Entry | undefined) {
  if (!entry) return
  emit(
    'choose',
    entry.kind === 'attachment'
      ? { kind: 'attachment', attachment: entry.attachment }
      : { kind: 'project', project: entry.project },
  )
}

function entryName(entry: Entry): string {
  return entry.kind === 'attachment' ? entry.attachment.name : entry.project.name
}

function entryPath(entry: Entry): string {
  return entry.kind === 'attachment' ? entry.attachment.entityPath : entry.project.path
}

/** 项目条目右侧的仓库信息：Git / SVN + 地址（地址是搜索命中的主要目标） */
function entryVcs(entry: Entry): string {
  if (entry.kind !== 'project') return ''
  const label = vcsLabel(entry.project.vcsType)
  return [label, entry.project.vcsUrl].filter(Boolean).join(' · ')
}

function onKeydown(event: KeyboardEvent) {
  const list = visibleEntries.value
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    if (list.length) selected.value = (selected.value + 1) % list.length
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    if (list.length) {
      selected.value = (selected.value - 1 + list.length) % list.length
    }
  } else if (event.key === 'Enter') {
    event.preventDefault()
    choose(list[selected.value])
  } else if (event.key === 'Escape') {
    event.preventDefault()
    emit('close')
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="quickopen-overlay" @pointerdown.self="emit('close')">
      <div class="quickopen" role="dialog" aria-modal="true" aria-label="搜索附件或项目">
        <input
          ref="inputEl"
          v-model="query"
          class="quickopen-input"
          type="text"
          placeholder="搜索附件、项目或 Git/SVN 地址"
          spellcheck="false"
          @keydown="onKeydown"
        />
        <div v-if="visibleEntries.length > 0" class="quickopen-list">
          <button
            v-for="(entry, index) in visibleEntries"
            :key="
              entry.kind === 'attachment'
                ? `attachment:${entry.attachment.entityKind}:${entry.attachment.entityId}:${entry.attachment.id}`
                : `project:${entry.project.id}`
            "
            type="button"
            class="quickopen-item"
            :class="{ active: index === selected }"
            @pointermove="selected = index"
            @click="choose(entry)"
          >
            <span class="quickopen-name" :title="entryName(entry)">
              <template v-for="(seg, i) in splitMatch(entryName(entry), keyword)" :key="i">
                <mark v-if="seg.hit" class="quickopen-hit">{{ seg.text }}</mark>
                <template v-else>{{ seg.text }}</template>
              </template>
            </span>
            <span v-if="entry.kind === 'project'" class="quickopen-kind">项目</span>
            <span
              v-if="entry.kind === 'project' && entryVcs(entry)"
              class="quickopen-vcs"
              :title="entryVcs(entry)"
            >
              <template v-for="(seg, i) in splitMatch(entryVcs(entry), keyword)" :key="i">
                <mark v-if="seg.hit" class="quickopen-hit">{{ seg.text }}</mark>
                <template v-else>{{ seg.text }}</template>
              </template>
            </span>
            <span class="quickopen-path" :title="entryPath(entry)">
              <template v-for="(seg, i) in splitMatch(entryPath(entry), keyword)" :key="i">
                <mark v-if="seg.hit" class="quickopen-hit">{{ seg.text }}</mark>
                <template v-else>{{ seg.text }}</template>
              </template>
            </span>
          </button>
        </div>
        <div v-else class="quickopen-empty">
          {{ props.items.length > 0 || (props.projects ?? []).length > 0 ? '没有匹配的附件或项目' : '还没有任何附件' }}
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.quickopen-overlay {
  position: fixed;
  inset: 0;
  z-index: 3000;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 10vh;
}
.quickopen {
  width: min(680px, calc(100vw - 40px));
  max-height: 60vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
  border: 1px solid var(--border-color);
  border-radius: 10px;
  box-shadow: 0 18px 50px rgba(0, 0, 0, 0.22);
  overflow: hidden;
}
.quickopen-input {
  flex-shrink: 0;
  width: 100%;
  padding: 12px 16px;
  border: none;
  border-bottom: 1px solid var(--border-color);
  outline: none;
  background: transparent;
  font-size: 14px;
  color: var(--text-primary);
}
.quickopen-input::placeholder {
  color: var(--text-muted);
}
.quickopen-list {
  overflow-y: auto;
  padding: 4px;
}
.quickopen-item {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  border-radius: 6px;
  background: transparent;
  text-align: left;
  cursor: pointer;
}
.quickopen-item.active {
  background: rgba(59, 130, 246, 0.12);
}
.quickopen-name {
  flex: 1 1 auto;
  min-width: 0;
  font-size: 14px;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.quickopen-kind {
  flex: none;
  padding: 0 5px;
  border-radius: 4px;
  background: var(--bg-tag);
  color: var(--text-tag);
  font-size: 11px;
  line-height: 16px;
}
.quickopen-hit {
  background: transparent;
  color: var(--primary);
  font-weight: 600;
}
.quickopen-vcs {
  flex: 0 1 auto;
  min-width: 0;
  max-width: 38%;
  font-size: 11px;
  color: var(--text-secondary);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.quickopen-path {
  flex-shrink: 0;
  margin-left: auto;
  max-width: 32%;
  font-size: 12px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.quickopen-empty {
  padding: 18px 16px;
  font-size: 14px;
  color: var(--text-muted);
}
</style>
