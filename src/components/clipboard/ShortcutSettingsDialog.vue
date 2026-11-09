<script setup lang="ts">
import { ref, watch } from 'vue'
import { Button, Modal } from 'ant-design-vue'
import { setClipboardShortcut } from '@/api/clipboard'
import { formatShortcut, shortcutFromEvent } from '@/clipboard/shortcut'

/**
 * 呼出面板的全局快捷键设置弹窗。
 *
 * 点录制区域后直接按组合键即可；也可以清空，清空后不注册任何全局快捷键。
 * 保存时后端会尝试注册，被别的应用占用就把错误原样显示出来，原来的快捷键继续可用。
 */

const props = defineProps<{
  open: boolean
  /** 当前生效的加速键字符串，空字符串表示未设置 */
  shortcut: string
}>()

const emit = defineEmits<{
  'update:open': [boolean]
  saved: [string]
}>()

const draft = ref(props.shortcut)
const recording = ref(false)
const saving = ref(false)
const error = ref('')

watch(
  () => props.open,
  (open) => {
    if (open) {
      draft.value = props.shortcut
      error.value = ''
      saving.value = false
    }
    stopRecording()
  },
)

function startRecording() {
  if (saving.value) return
  error.value = ''
  recording.value = true
  window.addEventListener('keydown', onRecordingKey, true)
}

function stopRecording() {
  recording.value = false
  window.removeEventListener('keydown', onRecordingKey, true)
}

function onRecordingKey(event: KeyboardEvent) {
  if (!recording.value) return
  // 录制期间不能把按键交给弹窗 / 全局快捷键
  event.preventDefault()
  event.stopPropagation()
  if (event.key === 'Escape') {
    stopRecording()
    return
  }
  // 没有修饰键的退格 / 删除当作「清空」，这类组合本来也不适合当全局快捷键
  if (
    (event.code === 'Backspace' || event.code === 'Delete') &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.altKey
  ) {
    draft.value = ''
    stopRecording()
    return
  }
  const combo = shortcutFromEvent(event)
  if (!combo) return
  draft.value = combo
  stopRecording()
}

function clearDraft() {
  draft.value = ''
  error.value = ''
  stopRecording()
}

function close() {
  stopRecording()
  emit('update:open', false)
}

async function save() {
  if (saving.value || recording.value) return
  saving.value = true
  error.value = ''
  try {
    // 空字符串 = 清空快捷键
    const accelerator = await setClipboardShortcut(draft.value)
    emit('saved', accelerator)
    close()
  } catch (err) {
    error.value = String(err).replace(/^Error:\s*/, '')
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <Modal
    :open="open"
    title="快捷键"
    :width="320"
    :keyboard="!recording"
    :mask-closable="false"
    @cancel="close"
  >
    <div class="sd">
      <button
        type="button"
        class="sd-record"
        :class="{ recording, empty: !draft && !recording }"
        @click="startRecording"
      >
        {{ recording ? '按下快捷键…' : draft ? formatShortcut(draft) : '未设置' }}
      </button>
      <p v-if="error" class="sd-error">{{ error }}</p>
    </div>

    <template #footer>
      <div class="sd-footer">
        <Button type="text" :disabled="!draft || saving" @click="clearDraft">清空</Button>
        <div class="sd-footer-actions">
          <Button @click="close">取消</Button>
          <Button type="primary" :loading="saving" @click="save">保存</Button>
        </div>
      </div>
    </template>
  </Modal>
</template>

<style scoped>
.sd {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

/* 录制区域：和弹窗同宽，按键组合居中显示 */
.sd-record {
  height: 56px;
  padding: 0 12px;
  border: 1px dashed var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: 17px;
  letter-spacing: 1px;
  cursor: pointer;
}

.sd-record:hover {
  border-color: var(--primary);
}

.sd-record.empty {
  color: var(--text-muted);
  font-size: 14px;
  letter-spacing: 0;
}

.sd-record.recording {
  border-color: var(--primary);
  border-style: solid;
  color: var(--primary);
  font-size: 13px;
  letter-spacing: 0;
}

.sd-error {
  margin: 0;
  color: #dc2626;
  font-size: 12px;
  line-height: 1.5;
}

.sd-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.sd-footer-actions {
  display: flex;
  gap: 8px;
}
</style>
