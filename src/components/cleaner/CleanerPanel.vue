<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useCleaner } from '@/composables/useCleaner'
import InstalledAppsPanel from './InstalledAppsPanel.vue'
import MemoryPanel from './MemoryPanel.vue'

/**
 * 「清理」工具页：页头 + 两个页签。
 *
 * 两个页签用 v-show 保留各自状态（搜索词、排序、勾选的应用、弹窗），
 * 只有内存页签激活并且整个工具页可见时，内部才按 2 秒轮询采样。
 */
const props = defineProps<{
  /** 当前工具页是否激活（App.vue 的 view === 'cleaner'） */
  active: boolean
}>()

type TabKey = 'apps' | 'memory'

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

const memoryTabActive = computed(() => props.active && tab.value === 'memory')
const rescanning = computed(() =>
  tab.value === 'apps' ? cleaner.appsLoading : cleaner.sampling,
)

/** 「重新扫描」：软件卸载页重扫安装列表，内存页立即重新采样一次 */
async function handleRescan() {
  if (rescanning.value) return
  if (tab.value === 'apps') {
    await cleaner.loadApps(true)
  } else {
    await cleaner.sampleMemory()
  }
}
</script>

<template>
  <div class="cleaner">
    <div class="cleaner-head">
      <div class="cleaner-title">
        <h3>系统清理</h3>
        <p>卸载软件并清理残留，或查看内存占用、结束进程、深度释放。</p>
      </div>
      <button
        class="btn btn-secondary"
        :disabled="rescanning"
        :title="tab === 'apps' ? '重新枚举已安装应用' : '立即重新采样内存'"
        @click="handleRescan"
      >
        {{ rescanning ? '扫描中…' : '重新扫描' }}
      </button>
    </div>

    <div class="cleaner-tabs" role="tablist" aria-label="清理工具页签">
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
        type="button"
        class="cleaner-tab"
        :class="{ active: tab === 'memory' }"
        role="tab"
        :aria-selected="tab === 'memory'"
        @click="tab = 'memory'"
      >
        内存占用
      </button>
    </div>

    <div v-show="tab === 'apps'" class="cleaner-pane">
      <InstalledAppsPanel :cleaner="cleaner" />
    </div>
    <div v-show="tab === 'memory'" class="cleaner-pane">
      <MemoryPanel :cleaner="cleaner" :active="memoryTabActive" />
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

/* 页头：标题 + 说明 + 重新扫描，和设置页同一套排版 */
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
.cleaner-head .btn {
  flex-shrink: 0;
  height: 28px;
  padding: 0 12px;
  font-size: 12px;
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
