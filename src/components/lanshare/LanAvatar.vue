<script setup lang="ts">
/**
 * 聊天头像：系统图标，不引图片资源（设计 §2.1）。
 *
 * 自己用「人形」、对方用「显示器」，底色统一淡色块、图标用主文字色，
 * 靠图标形状区分方向；纯展示，不处理点击。
 */
defineProps<{
  /** true = 自己（人形图标），false = 对方（显示器图标） */
  self: boolean
}>()
</script>

<template>
  <span class="lan-avatar" :class="self ? 'is-self' : 'is-peer'" :title="self ? '我' : '对方设备'">
    <!-- 自己：人形图标 -->
    <svg
      v-if="self"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.7"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="8" r="4" />
      <path d="M4 21a8 8 0 0 1 16 0" />
    </svg>
    <!-- 对方：显示器图标 -->
    <svg
      v-else
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.7"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <rect x="3" y="4" width="18" height="13" rx="2" />
      <path d="M8 21h8" />
      <path d="M12 17v4" />
    </svg>
  </span>
</template>

<style scoped>
.lan-avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  flex-shrink: 0;
  border-radius: 50%;
  /* 底色统一淡色块，图标用主文字色；自己 / 对方只靠图标形状区分 */
  background: var(--bg-tag);
  color: var(--text-primary);
}
.lan-avatar svg {
  width: 17px;
  height: 17px;
}
</style>
