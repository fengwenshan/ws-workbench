<script setup lang="ts">
import { formatListTime } from './format'
import type { MergedPeer } from './types'

/**
 * 左栏：LANLink 字标 + 设备列表 + 设置入口（Glass Deck 视觉，刷新 §2.2）。
 *
 * 只负责展示与抛事件，排序 / 合并 / 在线判定都在 LanSharePanel 里做，
 * 避免两处逻辑各算一遍导致顺序不一致。列表项用玻璃 hover + 白色选中卡 +
 * 左侧主色指示条；设置入口收到页头右侧齿轮，不再占用底部 footer。
 */
const { items, activeId, settingsActive, loading, now } = defineProps<{
  items: MergedPeer[]
  /** 当前选中的设备 id */
  activeId: string | null
  /** 右栏当前是设置页（齿轮高亮） */
  settingsActive: boolean
  /** 首次读取快照 / 会话中，空态显示「正在读取」 */
  loading: boolean
  /** 1 秒一跳的时间基准，用来刷新「今天 HH:mm / 昨天 / MM-DD」 */
  now: number
}>()

const emit = defineEmits<{
  select: [id: string]
  /** 右键列表项：打开会话管理菜单（x/y 是鼠标位置，菜单用 fixed 定位跟随） */
  'context-menu': [item: MergedPeer, x: number, y: number]
  'open-settings': []
}>()

/** 显示名：本机备注优先于设备名（设计 §2.3），备注只在本机显示 */
function displayName(item: MergedPeer): string {
  return item.remark || item.alias
}

function onSelect(item: MergedPeer) {
  emit('select', item.id)
}

/** 右键：不再用原生 title（长悬浮会弹黑色提示），删除等入口都收进自定义菜单 */
function onContextMenu(event: MouseEvent, item: MergedPeer) {
  event.preventDefault()
  emit('context-menu', item, event.clientX, event.clientY)
}

function onKeydown(event: KeyboardEvent, item: MergedPeer) {
  if (event.key !== 'Enter' && event.key !== ' ') return
  event.preventDefault()
  onSelect(item)
}
</script>

<template>
  <nav class="sidebar">
    <header class="side-head">
      <span class="brand-mark" aria-hidden="true">
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
        >
          <circle cx="5.5" cy="12" r="2.1" />
          <circle cx="18.5" cy="12" r="2.1" />
          <path d="M7.6 12h8.8" />
        </svg>
      </span>
      <h3 class="brand-name">LANLink</h3>
      <button
        class="settings-btn"
        :class="{ active: settingsActive }"
        type="button"
        title="设置"
        aria-label="设置"
        @click="emit('open-settings')"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.7"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <circle cx="12" cy="12" r="3" />
          <path
            d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9v.09a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
          />
        </svg>
      </button>
    </header>

    <div class="list-wrap">
      <ul v-if="items.length" class="peer-list">
        <li
          v-for="item in items"
          :key="item.id"
          class="peer-item"
          :class="{ active: item.id === activeId && !settingsActive }"
          role="button"
          tabindex="0"
          @click="onSelect(item)"
          @keydown="onKeydown($event, item)"
          @contextmenu.prevent="onContextMenu($event, item)"
        >
          <span class="online-dot" :class="item.online ? 'is-online' : 'is-offline'"></span>
          <span class="item-body">
            <span class="item-top">
              <span class="item-name">{{ displayName(item) }}</span>
              <span class="item-time">{{ formatListTime(item.sortTimeMs, now) }}</span>
            </span>
            <span class="item-bottom">
              <span class="item-preview" :class="{ 'is-muted': !item.preview }">
                {{ item.preview || '暂无消息' }}
              </span>
              <span v-if="item.offerCount > 0" class="offer-badge">
                待接收 {{ item.offerCount }}
              </span>
              <span v-if="item.unread > 0" class="unread-badge">
                {{ item.unread > 99 ? '99+' : item.unread }}
              </span>
            </span>
          </span>
        </li>
      </ul>

      <!-- 空态：读取中 / 等待同一网络中的设备 -->
      <div v-else class="peer-empty">
        <svg
          class="peer-empty-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <circle cx="6" cy="12" r="2.3" />
          <circle cx="18" cy="6" r="2.3" />
          <circle cx="18" cy="18" r="2.3" />
          <path d="M8.1 10.9l7.8-3.8M8.1 13.1l7.8 3.8" />
        </svg>
        <p class="peer-empty-title">
          {{ loading ? '正在读取…' : '等待同一网络中的设备' }}
        </p>
        <p class="peer-empty-hint">
          {{
            loading
              ? '正在获取设备信息。'
              : '在另一台电脑打开 LANLink，保持同一 Wi-Fi 即可互相发现。'
          }}
        </p>
      </div>
    </div>
  </nav>
</template>

<style scoped>
.sidebar {
  position: relative;
  width: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

/* 页头：LANLink 字标 + 右侧齿轮；无下边框，用留白分层 */
.side-head {
  flex-shrink: 0;
  height: 56px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 12px 0 18px;
}
.brand-mark {
  display: inline-flex;
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  color: var(--primary);
}
.brand-mark svg {
  width: 20px;
  height: 20px;
}
.brand-name {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  letter-spacing: 0.12em;
  color: var(--text-primary);
}
.settings-btn {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  padding: 0;
  border: none;
  border-radius: 9px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  flex-shrink: 0;
  transition: background 0.16s, color 0.16s, box-shadow 0.16s;
}
.settings-btn:hover {
  background: rgba(255, 255, 255, 0.8);
  color: var(--text-primary);
}
.settings-btn.active {
  background: rgba(255, 255, 255, 0.95);
  color: var(--primary);
  box-shadow: 0 1px 2px rgba(15, 23, 42, 0.05);
}
.settings-btn svg {
  width: 17px;
  height: 17px;
}

.list-wrap {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 2px 10px 14px;
}
.peer-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

/* 列表项：默认透明；hover 玻璃上浮；选中白色卡片 + 左侧主色指示条 */
.peer-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 12px 10px 14px;
  border-radius: 12px;
  cursor: pointer;
  outline: none;
  transition: background 0.16s, box-shadow 0.16s, transform 0.16s;
}
.peer-item:hover {
  background: rgba(255, 255, 255, 0.75);
  transform: translateY(-1px);
}
.peer-item:focus-visible {
  box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.35);
}
.peer-item.active {
  background: rgba(255, 255, 255, 0.95);
  box-shadow:
    0 1px 2px rgba(15, 23, 42, 0.05),
    0 12px 28px -18px rgba(37, 99, 235, 0.45);
}
.peer-item.active::before {
  content: '';
  position: absolute;
  left: 4px;
  top: 50%;
  transform: translateY(-50%);
  width: 3px;
  height: 18px;
  border-radius: 3px;
  background: var(--primary);
}
.peer-item.active .online-dot {
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.16);
}

.online-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--text-muted);
}
.online-dot.is-online {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.14);
}
.online-dot.is-offline {
  background: var(--text-muted);
}

.item-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.item-top,
.item-bottom {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.item-name {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.item-time {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-muted);
}
.item-preview {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.item-preview.is-muted {
  color: var(--text-muted);
}
.unread-badge {
  flex-shrink: 0;
  min-width: 17px;
  height: 17px;
  padding: 0 5px;
  border-radius: 9px;
  background: var(--danger);
  color: #fff;
  font-size: 11px;
  line-height: 17px;
  text-align: center;
}
/* 待接收文件提醒：比未读更急，用橙色描边小标签 */
.offer-badge {
  flex-shrink: 0;
  height: 17px;
  padding: 0 6px;
  border-radius: 9px;
  border: 1px solid var(--warning);
  color: var(--warning);
  font-size: 11px;
  line-height: 15px;
  white-space: nowrap;
}

/* 空态 */
.peer-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 48px 14px;
  text-align: center;
}
.peer-empty-icon {
  width: 48px;
  height: 48px;
  color: var(--text-muted);
  opacity: 0.6;
}
.peer-empty-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-secondary);
}
.peer-empty-hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}

/* 减少动效：关掉上浮，只保留颜色变化 */
@media (prefers-reduced-motion: reduce) {
  .peer-item,
  .settings-btn {
    transition: none;
  }
  .peer-item:hover {
    transform: none;
  }
}
</style>
