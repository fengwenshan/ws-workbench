<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'

/**
 * 聊天头部的「显示名 + 备注就地编辑」（设计 §2.3 / §4.3）。
 *
 * 显示名 = 备注 || 设备名；点铅笔进入编辑，Enter 保存、Esc 取消、
 * 清空后保存即删除备注。纯 UI：保存只抛 save 事件，命令调用与快照刷新由上层统一做。
 */
const props = defineProps<{
  /** 会话 id = 设备指纹：切设备时要退出编辑态 */
  peerId: string
  /** 设备名（最近的 announce 别名 / 会话别名） */
  alias: string
  /** 本机备注（空串 = 没起备注） */
  remark: string
}>()

const emit = defineEmits<{
  /** 保存备注（空串 = 删除备注） */
  save: [remark: string]
}>()

const editing = ref(false)
const draft = ref('')
const inputRef = ref<HTMLInputElement | null>(null)

/** 显示名：本机备注优先，没备注才用设备名 */
const displayName = computed(() => props.remark || props.alias)

function start() {
  draft.value = props.remark
  editing.value = true
  void nextTick(() => inputRef.value?.focus())
}

function cancel() {
  editing.value = false
  draft.value = ''
}

function save() {
  // 与后端限制一致：trim + 最多 32 个字符（emoji 按 1 个算）
  const text = [...draft.value.trim()].slice(0, 32).join('')
  editing.value = false
  draft.value = ''
  // 没改动就不打命令，避免无意义的快照往返
  if (text === props.remark) return
  emit('save', text)
}

function onKeydown(event: KeyboardEvent) {
  // 中文输入法确认候选词的 Enter 不能当成保存
  if (event.isComposing || event.keyCode === 229) return
  if (event.key === 'Enter') {
    event.preventDefault()
    save()
  } else if (event.key === 'Escape') {
    event.preventDefault()
    cancel()
  }
}

// 切设备时退出编辑态，别把备注写到另一台设备上
watch(
  () => props.peerId,
  () => cancel(),
)
</script>

<template>
  <span class="remark-editor">
    <template v-if="editing">
      <input
        ref="inputRef"
        v-model="draft"
        class="remark-input"
        type="text"
        maxlength="32"
        placeholder="备注（留空恢复设备名）"
        @keydown="onKeydown"
      />
      <button class="icon-btn" type="button" title="保存备注" @click="save">✓</button>
      <button class="icon-btn" type="button" title="取消" @click="cancel">✕</button>
    </template>
    <template v-else>
      <span class="name" :title="remark ? `设备名：${alias}` : alias">{{ displayName }}</span>
      <button
        class="edit-btn"
        type="button"
        title="设置备注"
        aria-label="设置备注"
        @click="start"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.7"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M12 20h9" />
          <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" />
        </svg>
      </button>
    </template>
  </span>
</template>

<style scoped>
.remark-editor {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.name {
  min-width: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.remark-input {
  width: 180px;
  max-width: 40vw;
  padding: 4px 8px;
  border: 1px solid var(--primary);
  border-radius: 4px;
  background: var(--bg-input);
  color: var(--text-primary);
  font-size: 13px;
  font-family: inherit;
  outline: none;
}
.icon-btn {
  padding: 0 4px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  line-height: 1.6;
  cursor: pointer;
}
.icon-btn:hover {
  background: var(--bg-hover);
}
.edit-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  padding: 0;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.edit-btn:hover {
  background: var(--bg-hover);
  color: var(--primary);
}
.edit-btn svg {
  width: 13px;
  height: 13px;
}
</style>
