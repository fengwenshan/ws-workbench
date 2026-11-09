<script setup lang="ts">
import { computed, ref } from 'vue'
import { Tabs, TabPane } from 'ant-design-vue'
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
 * 页签顺序：弹窗拦截（仅 Windows）/ 屏幕休眠 / Hosts 在线编辑 / 剪贴板 / 软件卸载（放最后）。
 *
 * 页签用 antd Tabs：每个页签下面是独立组件，默认只有首次切到该页签时才渲染挂载
 * （之后保留在 DOM 里，切回来不丢状态）。因此「软件卸载」的安装列表扫描放在
 * InstalledAppsPanel 自己的 onMounted 里，打开「系统」页不会提前扫。
 *
 * 轮询类页签（屏幕休眠 / Hosts / 剪贴板）仍靠 `active` prop 控制：
 * 整个工具页不可见或切到别的页签就停（垃圾弹窗检测线程由用户手动开关，不随页签切换停止）。
 */
const props = defineProps<{
  /** 当前工具页是否激活（App.vue 的 view === 'cleaner'） */
  active: boolean
}>()

type TabKey = 'apps' | 'popup' | 'sleep' | 'hosts' | 'clipboard'

const isWindows = computed(() => platform.is_windows)

const cleaner = useCleaner()
const tab = ref<TabKey>('sleep')

/** antd 的 activeKey 类型是 string | number，这里收窄回页签联合类型 */
function onTabChange(key: string | number): void {
  tab.value = key as TabKey
}

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

    <Tabs class="cleaner-tabs" :active-key="tab" @change="onTabChange">
      <TabPane v-if="isWindows" key="popup" tab="弹窗拦截">
        <PopupPanel :active="popupTabActive" />
      </TabPane>
      <TabPane key="sleep" tab="屏幕休眠">
        <SleepPanel :active="sleepTabActive" />
      </TabPane>
      <TabPane key="hosts" tab="Hosts">
        <HostsEditor :active="hostsTabActive" />
      </TabPane>
      <TabPane key="clipboard" tab="剪贴板">
        <ClipboardManager :active="clipboardTabActive" />
      </TabPane>
      <TabPane key="apps" tab="软件卸载">
        <InstalledAppsPanel :cleaner="cleaner" />
      </TabPane>
    </Tabs>
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

/* 页签栏：沿用 ProjectDetail 的 40px 标题栏高度与下划线激活态 */
.cleaner-tabs {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  margin-top: 10px;
}
.cleaner-tabs :deep(.ant-tabs-nav) {
  height: var(--panel-header-height);
  margin: 0;
  padding: 0 20px;
  flex-shrink: 0;
}
.cleaner-tabs :deep(.ant-tabs-nav::before) {
  border-bottom-color: var(--border-color);
}
.cleaner-tabs :deep(.ant-tabs-nav-wrap),
.cleaner-tabs :deep(.ant-tabs-nav-list) {
  height: 100%;
}
/* 选择器多带一层 .ant-tabs-nav-list：antd 的样式由 cssinjs 后注入，同权重会输给它 */
.cleaner-tabs :deep(.ant-tabs-nav-list > .ant-tabs-tab) {
  height: 100%;
  padding: 0;
  margin: 0 28px 0 0;
  font-size: 13px;
}
.cleaner-tabs :deep(.ant-tabs-nav-list > .ant-tabs-tab .ant-tabs-tab-btn) {
  color: var(--text-secondary);
}
.cleaner-tabs :deep(.ant-tabs-nav-list > .ant-tabs-tab:hover .ant-tabs-tab-btn) {
  color: var(--text-primary);
}
.cleaner-tabs :deep(.ant-tabs-nav-list > .ant-tabs-tab.ant-tabs-tab-active .ant-tabs-tab-btn) {
  color: var(--primary);
  font-weight: 600;
}
.cleaner-tabs :deep(.ant-tabs-nav-list > .ant-tabs-ink-bar) {
  bottom: 0;
  background: var(--primary);
}
.cleaner-tabs :deep(.ant-tabs-content-holder) {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.cleaner-tabs :deep(.ant-tabs-content) {
  height: 100%;
}
.cleaner-tabs :deep(.ant-tabs-tabpane) {
  height: 100%;
  display: flex;
  flex-direction: column;
}
</style>
