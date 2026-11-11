<script setup lang="ts">
/**
 * 弹窗拦截的盾牌图标：主开关关闭时是灰色描边，开启后填充主色。
 * 主开关区和空列表共用，避免两处 SVG 路径不一致。
 */
withDefaults(
  defineProps<{
    /** 边长（px） */
    size?: number
    /** 拦截是否生效 */
    active?: boolean
  }>(),
  { size: 40, active: false },
)
</script>

<template>
  <svg
    class="shield"
    :class="{ on: active }"
    :width="size"
    :height="size"
    viewBox="0 0 48 48"
    aria-hidden="true"
    focusable="false"
  >
    <path
      d="M24 4 8 10v12c0 10.5 6.8 18.3 16 22 9.2-3.7 16-11.5 16-22V10L24 4Z"
      class="shield-fill"
    />
    <path
      d="M24 4 8 10v12c0 10.5 6.8 18.3 16 22 9.2-3.7 16-11.5 16-22V10L24 4Z"
      class="shield-line"
      fill="none"
      stroke-width="2"
      stroke-linejoin="round"
    />
    <path
      d="m16.5 24 5 5 10-10"
      class="shield-check"
      fill="none"
      stroke-width="2.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
  </svg>
</template>

<style scoped>
.shield {
  display: block;
  flex-shrink: 0;
}
.shield-fill {
  fill: var(--bg-tag);
  opacity: 0.55;
}
.shield.on .shield-fill {
  fill: var(--primary);
  opacity: 0.13;
}
.shield-line,
.shield-check {
  stroke: var(--text-muted);
}
.shield.on .shield-line,
.shield.on .shield-check {
  stroke: var(--primary);
}
</style>
