<script setup lang="ts">
import { computed } from 'vue'

type DotState = 'off' | 'on' | 'busy'

const props = withDefaults(
  defineProps<{
    state: DotState
    size?: 'sm' | 'md'
    /** 是否带一圈淡淡的同色光晕，图标栏这种小面积位置用 */
    glow?: boolean
    label?: string
  }>(),
  {
    size: 'sm',
    glow: false,
    label: '',
  },
)

const classes = computed(() => [
  `is-${props.state}`,
  `size-${props.size}`,
  { glow: props.glow },
])
</script>

<template>
  <span
    class="vpn-status-dot"
    :class="classes"
    role="img"
    :title="label || undefined"
    :aria-label="label || undefined"
    :aria-hidden="label ? undefined : 'true'"
  />
</template>

<style scoped>
.vpn-status-dot {
  display: inline-block;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--text-muted);
  transition: background 0.15s, box-shadow 0.15s, opacity 0.15s;
}
.size-sm {
  width: 8px;
  height: 8px;
}
.size-md {
  width: 8px;
  height: 8px;
}
.is-on {
  background: var(--success);
}
.is-busy {
  background: var(--warning);
  animation: vpn-dot-pulse 1s ease-in-out infinite;
}
.glow.is-on {
  box-shadow: 0 0 0 4px rgba(16, 185, 129, 0.15);
}
.glow.is-busy {
  box-shadow: 0 0 0 4px rgba(245, 158, 11, 0.16);
}
@keyframes vpn-dot-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.3;
  }
}
@media (prefers-reduced-motion: reduce) {
  .is-busy {
    animation: none;
    opacity: 1;
  }
}
</style>
