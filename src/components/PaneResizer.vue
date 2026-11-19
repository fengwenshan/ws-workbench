<script setup lang="ts">
/**
 * 左栏拖拽条：项目树 / VPN / 系统 / LANLink / 设置共用。
 *
 * 宽度、拖拽和持久化都在 `useWorkbenchLayout` 里，App.vue provide 一份，
 * 这里只负责渲染 4px 的拖拽条并转发指针 / 键盘事件。
 */
import { inject } from 'vue'
import { PANE_RESIZE_KEY } from '@/composables/useWorkbenchLayout'

withDefaults(defineProps<{ label?: string }>(), { label: '调整左栏宽度' })

const resize = inject(PANE_RESIZE_KEY, null)
</script>

<template>
  <div
    class="pane-resizer"
    role="separator"
    aria-orientation="vertical"
    :aria-label="label"
    :title="`拖动${label}，双击恢复默认`"
    tabindex="0"
    @pointerdown="resize?.start"
    @keydown="resize?.keydown"
    @dblclick="resize?.reset"
  ></div>
</template>

<style scoped>
.pane-resizer {
  flex: 0 0 4px;
  width: 4px;
  cursor: col-resize;
  touch-action: none;
  background: transparent;
  transition: background 0.15s;
}
.pane-resizer:hover {
  background: var(--primary);
  opacity: 0.45;
}
.pane-resizer:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: -2px;
}
</style>
