<script setup lang="ts">
import { computed, ref } from 'vue'
import { Menu } from 'ant-design-vue'
import PaneResizer from '@/components/PaneResizer.vue'
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
  /** 左侧模块栏是否收起（再点一次「系统」图标切换，和项目树 / VPN 列表一致） */
  menuCollapsed?: boolean
}>()

type TabKey = 'apps' | 'popup' | 'sleep' | 'hosts' | 'clipboard'

const isWindows = computed(() => platform.is_windows)

const cleaner = useCleaner()
const tab = ref<TabKey>('sleep')

/** 已经打开过的分区：首次点开才挂载，之后保留在 DOM 里（不丢状态、不重复扫描） */
const visited = ref<Set<TabKey>>(new Set<TabKey>(['sleep']))

const navItems = computed(() => {
  const items = [
    { key: 'sleep', label: '屏幕休眠' },
    { key: 'hosts', label: 'Hosts' },
    { key: 'clipboard', label: '剪贴板' },
    { key: 'apps', label: '软件卸载' },
  ]
  if (isWindows.value) items.unshift({ key: 'popup', label: '弹窗拦截' })
  return items
})

function onSelect(key: string): void {
  tab.value = key as TabKey
  visited.value = new Set(visited.value).add(key as TabKey)
}

const popupTabActive = computed(() => props.active && tab.value === 'popup')
const sleepTabActive = computed(() => props.active && tab.value === 'sleep')
const hostsTabActive = computed(() => props.active && tab.value === 'hosts')
const clipboardTabActive = computed(() => props.active && tab.value === 'clipboard')
</script>

<template>
  <div class="cleaner" :class="{ 'is-collapsed': menuCollapsed }">
    <!-- 左栏：页头 + 分区导航，样式和项目树左栏同一套；收起后右栏自己补 4px -->
    <aside v-show="!menuCollapsed" class="cleaner-menu">
      <div class="menu-header">
        <span class="head-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 3.5l1.8 4.4 4.4 1.8-4.4 1.8L12 15.9l-1.8-4.4L5.8 9.7l4.4-1.8z" />
            <path d="M18.5 15l.8 1.9 1.9.8-1.9.8-.8 1.9-.8-1.9-1.9-.8 1.9-.8z" />
          </svg>
        </span>
        <span class="head-title">系统</span>
      </div>
      <div class="menu-body">
        <Menu
          class="menu-list"
          mode="inline"
          :items="navItems"
          :selected-keys="[tab]"
          @select="(info) => onSelect(info.key as string)"
        />
      </div>
    </aside>

    <PaneResizer v-show="!menuCollapsed" />

    <!-- 右栏：内容不套框，直接铺在工作区背景上 -->
    <section class="cleaner-main">
      <div v-show="tab === 'popup'" class="cleaner-pane">
        <PopupPanel v-if="isWindows && visited.has('popup')" :active="popupTabActive" />
      </div>
      <div v-show="tab === 'sleep'" class="cleaner-pane">
        <SleepPanel v-if="visited.has('sleep')" :active="sleepTabActive" />
      </div>
      <div v-show="tab === 'hosts'" class="cleaner-pane">
        <HostsEditor v-if="visited.has('hosts')" :active="hostsTabActive" />
      </div>
      <div v-show="tab === 'clipboard'" class="cleaner-pane">
        <ClipboardManager v-if="visited.has('clipboard')" :active="clipboardTabActive" />
      </div>
      <div v-show="tab === 'apps'" class="cleaner-pane">
        <InstalledAppsPanel v-if="visited.has('apps')" :cleaner="cleaner" />
      </div>
    </section>
  </div>
</template>

<style scoped>
/* 「系统」模块没有左栏，整页就是一张卡片：顶部贴标题栏，左右各 4px，下留 8px */
/* 「系统」模块：左栏是卡片（页头 + 分区导航，和项目树同一套），右栏内容铺在背景上不套框 */
.cleaner {
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 0;
  overflow: hidden;
}
/* 模块栏收起后不再占位，右栏自己补上和 rail 的 4px 间隙 */
.cleaner.is-collapsed {
  gap: 0;
}
.cleaner.is-collapsed .cleaner-main {
  margin-left: 4px;
}
.cleaner-menu {
  width: var(--pane-left-width);
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  margin-bottom: 8px;
  border: 1px solid var(--border-color);
  border-left: 0;
  border-radius: 0 var(--radius-lg) var(--radius-lg) 0;
  background: var(--bg-primary);
  overflow: hidden;
}
.menu-header {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  height: var(--pane-head-height);
  padding: 0 8px 0 20px;
  color: var(--text-primary);
}
.head-icon {
  display: inline-flex;
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  color: var(--primary);
}
.head-icon svg {
  width: 20px;
  height: 20px;
}
.head-title {
  font-size: 14px;
  font-weight: 700;
  letter-spacing: 0.2px;
}
.menu-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px;
}
/* 导航条目对齐项目树的 .list-item：30 高、6 圆角、选中浅底 + 加粗 */
.cleaner .menu-list {
  border-inline-end: 0;
  background: transparent;
}
.cleaner .menu-list :deep(.ant-menu-item) {
  height: 30px;
  line-height: 30px;
  margin: 0 0 2px;
  padding-inline: 12px !important;
  border-radius: var(--radius-md);
  font-size: 14px;
  color: var(--text-secondary);
}
.cleaner .menu-list :deep(.ant-menu-item:hover) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.cleaner .menu-list :deep(.ant-menu-item-selected) {
  background: var(--bg-active);
  color: var(--text-active);
  font-weight: 500;
}
.cleaner .menu-list :deep(.ant-menu-item::after) {
  display: none;
}

/* 右栏：一张内容卡 */
.cleaner-main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  margin: 0 4px 8px 0;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  overflow: hidden;
}
.cleaner-pane {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
</style>
