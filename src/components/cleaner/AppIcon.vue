<script setup lang="ts">
import { computed, ref, watch } from 'vue'

/**
 * 应用图标：后端给的是 `data:image/png;base64,...`。
 * 取不到图标（或图片解码失败）时退化成按名称散列颜色的首字母色块，
 * 保证列表里每一行都有视觉锚点，不会出现空洞。
 */
const props = withDefaults(
  defineProps<{
    /** data URL；null / 空串走首字母兜底 */
    icon?: string | null
    name: string
    /** 边长（px） */
    size?: number
    /** 圆角；不传按尺寸比例取 */
    radius?: number
  }>(),
  { icon: null, size: 28, radius: 0 },
)

const failed = ref(false)

watch(
  () => props.icon,
  () => {
    failed.value = false
  },
)

const showImage = computed(() => !!props.icon && !failed.value)

/** 中文名取第一个字，英文名取首字母大写 */
const initial = computed(() => {
  const name = (props.name ?? '').trim()
  if (!name) return '?'
  const first = Array.from(name)[0] ?? '?'
  return /[a-z]/i.test(first) ? first.toUpperCase() : first
})

/** 名称散列 → 固定色相：同一个应用每次都是同一个颜色 */
const hue = computed(() => {
  const text = props.name ?? ''
  let hash = 0
  for (let index = 0; index < text.length; index += 1) {
    hash = (hash * 31 + text.charCodeAt(index)) % 360
  }
  return hash
})

const boxStyle = computed(() => ({
  width: `${props.size}px`,
  height: `${props.size}px`,
  borderRadius: `${props.radius || Math.max(6, Math.round(props.size * 0.24))}px`,
}))

const fallbackStyle = computed(() => ({
  background: `hsl(${hue.value} 58% 52%)`,
  fontSize: `${Math.max(11, Math.round(props.size * 0.42))}px`,
}))
</script>

<template>
  <span class="app-icon" :style="boxStyle" :title="name">
    <img
      v-if="showImage"
      class="app-icon-img"
      :src="icon ?? ''"
      alt=""
      draggable="false"
      @error="failed = true"
    />
    <span v-else class="app-icon-fallback" :style="fallbackStyle">{{ initial }}</span>
  </span>
</template>

<style scoped>
.app-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  overflow: hidden;
  background: var(--bg-secondary);
}
.app-icon-img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}
.app-icon-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  color: #fff;
  font-weight: 600;
  line-height: 1;
  user-select: none;
}
</style>
