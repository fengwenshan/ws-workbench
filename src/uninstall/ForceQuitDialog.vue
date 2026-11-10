<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import AppIcon from '@/components/cleaner/AppIcon.vue'

/**
 * 强制退出确认模态（设计稿截图 2）。
 *
 * 独立卸载窗口里没有 antd，这个模态自己画：半透明遮罩 + 浅色圆角卡片、
 * 应用图标、加粗标题、说明文字、两个灰色按钮。Esc = 取消。
 */
const props = defineProps<{
  open: boolean
  name: string
  icon?: string | null
}>()

const emit = defineEmits<{
  (event: 'cancel'): void
  (event: 'confirm'): void
}>()

function onKeydown(event: KeyboardEvent): void {
  if (!props.open) return
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    emit('cancel')
  }
}

onMounted(() => {
  document.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  document.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="fq-overlay">
      <div
        class="fq-card"
        role="alertdialog"
        aria-modal="true"
        :aria-label="`${name} 正在运行中，是否要强制退出？`"
      >
        <div class="fq-icon">
          <AppIcon :icon="icon ?? null" :name="name" :size="50" :radius="11" />
        </div>
        <p class="fq-title"><b>{{ name }}</b> 正在运行中，是否要强制退出？</p>
        <p class="fq-desc">强制退出可能会导致正在编辑的数据丢失</p>
        <div class="fq-actions">
          <button type="button" class="fq-btn" @click="emit('cancel')">取消</button>
          <button type="button" class="fq-btn" @click="emit('confirm')">强制退出</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
