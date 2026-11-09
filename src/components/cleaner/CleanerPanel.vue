<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { platform } from '@/platform'
import { useCleaner } from '@/composables/useCleaner'
import InstalledAppsPanel from './InstalledAppsPanel.vue'
import PopupPanel from './PopupPanel.vue'
import SleepPanel from './SleepPanel.vue'
import HostsEditor from '@/components/hosts/HostsEditor.vue'
import ClipboardManager from '@/components/clipboard/ClipboardManager.vue'

/**
 * 「系统」工具页：页头 + 页签。
 *
 * 页签顺序：软件卸载 / 弹窗拦截（仅 Windows）/ 屏幕休眠 / Hosts 在线编辑。
 * 各页签用 v-show 保留自己的状态；只有落在激活页签上才启动轮询或监听
 * （垃圾弹窗检测线程由用户手动开关，不随页签切换停止）。
 */
const props = defineProps<{
  /** 当前工具页是否激活（App.vue 的 view === 'cleaner'） */
  active: boolean
}>()

type TabKey = 'apps' | 'popup' | 'sleep' | 'hosts' | 'clipboard'

const isWindows = computed(() => platform.is_windows)

const cleaner = useCleaner()
const tab = ref<TabKey>('apps')
/** 首次真正进入清理页时才拉安装列表，避免应用启动就扫一遍注册表/Applications */
const started = ref(false)

watch(
  () => props.active,
  (active) => {
    if (!active || started.value) return
    started.value = true
    void cleaner.loadApps(false)
  },
  { immediate: true },
)

const popupTabActive = computed(() => props.active && tab.value === 'popup')
const sleepTabActive = computed(() => props.active && tab.value === 'sleep')
const hostsTabActive = computed(() => props.active && tab.value === 'hosts')
const clipboardTabActive = computed(() => props.active && tab.value === 'clipboard')
</script>

<template>
  <div class="cleaner">
    <div class="cleaner-head">
      <div class="cleaner-title">
        <h3>系统</h3>
      </div>
    </div>

    <div class="cleaner-tabs" role="tablist" aria-label="系统工具页签">
      <button
        type="button"
        class="cleaner-tab"
        :class="{ active: tab === 'apps' }"
        role="tab"
        :aria-selected="tab === 'apps'"
        @click="tab = 'apps'"
      >
        软件卸载
        <span v-if="cleaner.apps.length" class="tab-count">{{ cleaner.apps.length }}</span>
      </button>
      <button
        v-if="isWindows"
        type="button"
        class="cleaner-tab"
        :class="{ active: tab === 'popup' }"
        role="tab"
        :aria-selected="tab === 'popup'"
        @click="tab = 'popup'"
      >
        弹窗拦截
      </button>
      <button
        type="button"
        class="cleaner-tab"
        :class="{ active: tab === 'sleep' }"
        role="tab"
        :aria-selected="tab === 'sleep'"
        @click="tab = 'sleep'"
      >
        屏幕休眠
      </button>
      <button
        type="button"
        class="cleaner-tab"
        :class="{ active: tab === 'hosts' }"
        role="tab"
        :aria-selected="tab === 'hosts'"
        @click="tab = 'hosts'"
      >
        Hosts
      </button>
      <button
        type="button"
        class="cleaner-tab"
        :class="{ active: tab === 'clipboard' }"
        role="tab"
        :aria-selected="tab === 'clipboard'"
        @click="tab = 'clipboard'"
      >
        剪贴板
      </button>
    </div>

    <div v-show="tab === 'apps'" class="cleaner-pane">
      <InstalledAppsPanel :cleaner="cleaner" />
    </div>
    <div v-if="isWindows" v-show="tab === 'popup'" class="cleaner-pane">
      <PopupPanel :active="popupTabActive" />
    </div>
    <div v-show="tab === 'sleep'" class="cleaner-pane">
      <SleepPanel :active="sleepTabActive" />
    </div>
    <div v-show="tab === 'hosts'" class="cleaner-pane">
      <HostsEditor :active="hostsTabActive" />
    </div>
    <div v-show="tab === 'clipboard'" class="cleaner-pane">
      <ClipboardManager :active="clipboardTabActive" />
    </div>
  </div>
</template>

<style scoped>
.cleaner {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* 页头：标题 + 说明，和设置页同一套排版 */
.cleaner-head {
  flex-shrink: 0;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 20px 0;
}
.cleaner-title h3 {
  font-size: 16px;
  color: var(--text-primary);
  margin-bottom: 4px;
}
.cleaner-title p {
  font-size: 12px;
  color: var(--text-muted);
}
/* 页签：沿用 ProjectDetail 的 40px 标题栏高度与下划线激活态 */
.cleaner-tabs {
  flex-shrink: 0;
  display: flex;
  align-items: stretch;
  gap: 28px;
  height: var(--panel-header-height);
  margin-top: 10px;
  padding: 0 20px;
  border-bottom: 1px solid var(--border-color);
}
.cleaner-tab {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 0;
  border: 0;
  border-radius: 0;
  background: transparent;
  color: var(--text-secondary);
  font-size: 13px;
  cursor: pointer;
}
.cleaner-tab:hover {
  color: var(--text-primary);
}
.cleaner-tab.active {
  color: var(--primary);
  font-weight: 600;
}
.cleaner-tab.active::after {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  bottom: -1px;
  height: 2px;
  border-radius: 1px;
  background: var(--primary);
}
.cleaner-tab:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: 2px;
}
.tab-count {
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  border-radius: 9px;
  background: var(--bg-tag);
  color: var(--text-secondary);
  font-size: 11px;
  font-weight: 500;
  line-height: 18px;
  text-align: center;
}
.cleaner-tab.active .tab-count {
  background: rgba(59, 130, 246, 0.12);
  color: var(--primary);
}

.cleaner-pane {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
</style>
