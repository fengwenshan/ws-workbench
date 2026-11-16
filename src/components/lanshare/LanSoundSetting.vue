<script setup lang="ts">
import { computed } from 'vue'
import { Switch } from 'ant-design-vue'
import { useLanShareNotify } from './useLanShareNotify'

/**
 * 「消息提示音」开关（设计 §4）。
 *
 * 开关值 = 非免打扰：开 = 收到对方消息时播放提示音；关 = 免打扰（静音）。
 * 状态存 localStorage，和左栏右键菜单的「免打扰模式」共享同一份单例，
 * 两处勾选态实时同步。纯前端开关，不请求后端。
 */
const { dnd, setDnd } = useLanShareNotify()

const soundEnabled = computed(() => !dnd.value)

function onToggle(enabled: boolean) {
  setDnd(!enabled)
}
</script>

<template>
  <div class="settings-card">
    <div class="sound-row">
      <span class="sound-body">
        <span class="sound-title">消息提示音</span>
        <span class="sound-desc">收到对方消息时播放提示音（关闭即免打扰）</span>
      </span>
      <Switch
        :checked="soundEnabled"
        size="small"
        @change="(checked) => onToggle(checked === true)"
      />
    </div>
  </div>
</template>

<style scoped>
/* 白色悬浮卡片：和设置页其它卡片同一套阴影，不用边框 */
.settings-card {
  border-radius: 14px;
  background: #fff;
  padding: 16px 18px;
  box-shadow:
    0 1px 2px rgba(15, 23, 42, 0.04),
    0 10px 30px -18px rgba(15, 23, 42, 0.28);
}
.sound-row {
  display: flex;
  align-items: flex-start;
  gap: 10px;
}
.sound-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.sound-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}
.sound-desc {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
</style>
