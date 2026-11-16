<script setup lang="ts">
import { ref, watch } from 'vue'
import { Modal } from 'ant-design-vue'

/**
 * 删除聊天记录的二次确认弹窗（设计 §3.3）。
 *
 * 文案明确「只删本机、对方不受影响」；会话里有本机收到的文件时才出现复选框，
 * 默认不勾选（不误删文件）。勾选后由后端把收到的文件移入废纸篓，再删数据库记录。
 */
const props = defineProps<{
  open: boolean
  /** 设备展示名（备注 || 设备名） */
  peerName: string
  /** 本机收到的文件数量；0 表示没有可勾选的文件 */
  attachmentCount: number
  /** 删除请求进行中 */
  busy: boolean
}>()

const emit = defineEmits<{
  confirm: [deleteFiles: boolean]
  cancel: []
}>()

/** 每次打开都恢复默认「不勾选」，避免上一次的选择被误用 */
const deleteFiles = ref(false)
watch(
  () => props.open,
  (open) => {
    if (open) deleteFiles.value = false
  },
)

function onOk() {
  if (props.busy) return
  emit('confirm', deleteFiles.value)
}

function onCancel() {
  if (props.busy) return
  emit('cancel')
}
</script>

<template>
  <Modal
    :open="open"
    title="删除聊天记录"
    :confirm-loading="busy"
    ok-text="删除"
    cancel-text="取消"
    :ok-button-props="{ danger: true }"
    @ok="onOk"
    @cancel="onCancel"
  >
    <p class="delete-tip">
      将删除与「{{ peerName }}」的本机聊天记录，对方设备上的记录不受影响。
    </p>
    <label v-if="attachmentCount > 0" class="delete-files">
      <input v-model="deleteFiles" type="checkbox" />
      <span>同时删除本机收到的 {{ attachmentCount }} 个文件（移入废纸篓）</span>
    </label>
  </Modal>
</template>

<style scoped>
.delete-tip {
  margin: 0 0 10px;
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1.6;
}

.delete-files {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  color: var(--text-primary);
  font-size: 13px;
  line-height: 1.6;
  cursor: pointer;
}

.delete-files input {
  margin-top: 3px;
  accent-color: var(--danger);
  cursor: pointer;
}
</style>
