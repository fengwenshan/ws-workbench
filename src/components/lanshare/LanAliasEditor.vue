<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { message } from 'ant-design-vue'
import { setLanShareAlias, type LanShareSnapshot } from '@/api/lanshare'

/**
 * 本机设备名：铅笔就地编辑。
 *
 * 沿用改造前的逻辑：只有 ✓ / Enter 才写后端，blur 不保存（避免误改）；
 * 保存失败时保留编辑态和输入内容，方便直接重试。
 * 成功返回的最新快照通过 `updated` 上抛，让右栏 / 左栏状态一致。
 */
const props = defineProps<{
  /** 当前设备名（来自快照） */
  alias: string
}>()

const emit = defineEmits<{
  updated: [snapshot: LanShareSnapshot]
}>()

const editing = ref(false)
const saving = ref(false)
const draft = ref('')
const inputRef = ref<HTMLInputElement | null>(null)

// 正在编辑时不要被后台快照覆盖草稿；不在编辑态时跟随最新别名
watch(
  () => props.alias,
  (alias) => {
    if (!editing.value) draft.value = alias ?? ''
  },
)

function errorText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

/** 进入编辑态：聚焦并全选当前名字 */
function startEdit() {
  draft.value = props.alias ?? ''
  editing.value = true
  void nextTick(() => {
    inputRef.value?.focus()
    inputRef.value?.select()
  })
}

/** 取消编辑：恢复原值，不写后端 */
function cancelEdit() {
  editing.value = false
  draft.value = props.alias ?? ''
}

async function save() {
  if (saving.value) return
  const alias = draft.value.trim()
  if (!alias) {
    message.warning('设备名不能为空')
    return
  }
  if (alias === props.alias) {
    editing.value = false
    return
  }
  saving.value = true
  try {
    emit('updated', await setLanShareAlias(alias))
    editing.value = false
    message.success('设备名已保存')
  } catch (err) {
    // 失败时保留编辑态和输入内容，方便直接重试
    message.error(`保存设备名失败：${errorText(err)}`)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="alias-editor">
    <span class="alias-label">本机设备名</span>
    <div v-if="!editing" class="alias-view">
      <span class="alias-name" :title="alias">{{ alias || '—' }}</span>
      <button
        class="icon-btn"
        type="button"
        title="编辑设备名"
        aria-label="编辑设备名"
        @click="startEdit"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M12 20h9" />
          <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
        </svg>
      </button>
    </div>
    <div v-else class="alias-edit">
      <input
        ref="inputRef"
        v-model="draft"
        class="alias-input"
        type="text"
        maxlength="64"
        :disabled="saving"
        placeholder="默认使用主机名"
        @keydown.enter.prevent="save"
        @keydown.esc.prevent="cancelEdit"
      />
      <button
        class="icon-btn is-ok"
        type="button"
        title="保存（Enter）"
        aria-label="保存设备名"
        :disabled="saving"
        @click="save"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M20 6 9 17l-5-5" />
        </svg>
      </button>
      <button
        class="icon-btn"
        type="button"
        title="取消（Esc）"
        aria-label="取消编辑"
        :disabled="saving"
        @click="cancelEdit"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M18 6 6 18M6 6l12 12" />
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.alias-editor {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 12px;
}
.alias-label {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
}
.alias-view,
.alias-edit {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  min-height: 28px;
}
.alias-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.alias-edit {
  flex: 1;
  max-width: 420px;
}
.alias-input {
  flex: 1;
  min-width: 0;
  height: 28px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-primary);
  font-size: 13px;
  outline: none;
}
.alias-input:focus {
  border-color: var(--primary);
}
.alias-input:disabled {
  background: var(--bg-readonly);
  color: var(--text-muted);
}
.icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  flex-shrink: 0;
  transition: color 0.15s, background 0.15s;
}
.icon-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--primary);
}
.icon-btn.is-ok {
  color: var(--success);
}
.icon-btn.is-ok:hover:not(:disabled) {
  background: rgba(16, 185, 129, 0.12);
  color: var(--success);
}
.icon-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.icon-btn svg {
  width: 15px;
  height: 15px;
}
</style>
