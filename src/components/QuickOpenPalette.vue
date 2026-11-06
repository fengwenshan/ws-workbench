<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import type { QuickAttachmentItem } from '@/types'

/**
 * VS Code 式快速搜索：居中悬浮在窗口上方，输入即筛选，↑↓ 选择、回车打开、Esc 关闭。
 *
 * 只负责展示与键盘交互；搜索范围（所有分组/项目的附件）和「打开」动作由父组件决定。
 */
const props = defineProps<{
  items: QuickAttachmentItem[]
}>()

const emit = defineEmits<{
  close: []
  choose: [item: QuickAttachmentItem]
}>()

const query = ref('')
const selected = ref(0)
const inputEl = ref<HTMLInputElement | null>(null)

/** 文件名、附件 id、所属位置都能搜到，和 VS Code 的命令面板一样宽松 */
const visibleItems = computed(() => {
  const keyword = query.value.trim().toLowerCase()
  if (!keyword) return props.items
  return props.items.filter(
    (item) =>
      item.name.toLowerCase().includes(keyword) ||
      item.id.toLowerCase().includes(keyword) ||
      item.entityPath.toLowerCase().includes(keyword),
  )
})

// 输入变化后高亮回到第一条，避免选中项落在被过滤掉的位置
watch(visibleItems, () => {
  selected.value = 0
})

onMounted(async () => {
  await nextTick()
  inputEl.value?.focus()
})

function choose(item: QuickAttachmentItem | undefined) {
  if (!item) return
  emit('choose', item)
}

function onKeydown(event: KeyboardEvent) {
  const list = visibleItems.value
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
      <div class="quickopen" role="dialog" aria-modal="true" aria-label="搜索附件">
        <input
          ref="inputEl"
          v-model="query"
          class="quickopen-input"
          type="text"
          placeholder="按名称搜索附件"
          spellcheck="false"
          @keydown="onKeydown"
        />
        <div v-if="visibleItems.length > 0" class="quickopen-list">
          <button
            v-for="(item, index) in visibleItems"
            :key="`${item.entityKind}:${item.entityId}:${item.id}`"
            type="button"
            class="quickopen-item"
            :class="{ active: index === selected }"
            @pointermove="selected = index"
            @click="choose(item)"
          >
            <span class="quickopen-name" :title="item.name">{{ item.name }}</span>
            <span class="quickopen-path" :title="item.entityPath">{{ item.entityPath }}</span>
          </button>
        </div>
        <div v-else class="quickopen-empty">
          {{ props.items.length > 0 ? '没有匹配的附件' : '还没有任何附件' }}
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
  min-width: 0;
  font-size: 14px;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.quickopen-path {
  flex-shrink: 0;
  margin-left: auto;
  max-width: 46%;
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
