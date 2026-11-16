<script setup lang="ts">
import { formatListTime } from './format'
import type { MergedPeer } from './types'

/**
 * 左栏：机器列表 + 底部设置入口。
 *
 * 只负责展示与抛事件，排序 / 合并 / 在线判定都在 LanSharePanel 里做，
 * 避免两处逻辑各算一遍导致顺序不一致。
 */
const { items, activeId, settingsActive, enabled, loading, statusText, statusClass, now } =
  defineProps<{
    items: MergedPeer[]
    /** 当前选中的设备 id */
    activeId: string | null
    /** 右栏当前是设置页（设置按钮高亮） */
    settingsActive: boolean
    /** 分享功能是否已开启（决定列表空态文案） */
    enabled: boolean
    /** 首次读取快照 / 会话中，空态显示「读取中」而不是「未开启」 */
    loading: boolean
    /** 底部运行状态文案 / 样式：运行中 / 未开启 / 启动失败 */
    statusText: string
    statusClass: string
    /** 1 秒一跳的时间基准，用来刷新「今天 HH:mm / 昨天 / MM-DD」 */
    now: number
  }>()

const emit = defineEmits<{
  select: [id: string]
  delete: [id: string]
  'open-settings': []
}>()

function onSelect(item: MergedPeer) {
  emit('select', item.id)
}

function onDelete(event: MouseEvent, item: MergedPeer) {
  event.stopPropagation()
  emit('delete', item.id)
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
      <h3>局域网分享</h3>
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
          :title="`${item.alias}${item.hasConversation ? '' : '（在线）'}`"
          @click="onSelect(item)"
          @keydown="onKeydown($event, item)"
        >
          <span
            class="online-dot"
            :class="item.online ? 'is-online' : 'is-offline'"
            :title="item.online ? '在线' : '离线'"
          ></span>
          <span class="item-body">
            <span class="item-top">
              <span class="item-name">{{ item.alias }}</span>
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
          <!-- 只给有历史会话的设备显示删除入口；hover / 键盘聚焦时露出 -->
          <button
            v-if="item.hasConversation"
            class="delete-btn"
            type="button"
            title="删除会话"
            aria-label="删除会话"
            @click="onDelete($event, item)"
          >
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="M3 6h18" />
              <path d="M8 6V4h8v2" />
              <path d="M19 6l-1 14H6L5 6" />
              <path d="M10 11v5M14 11v5" />
            </svg>
          </button>
        </li>
      </ul>

      <!-- 空态：没开启 / 已开启但还没发现设备，提示不一样 -->
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
          {{ loading ? '正在读取…' : enabled ? '暂时没有发现设备' : '局域网分享未开启' }}
        </p>
        <p class="peer-empty-hint">
          {{
            loading
              ? '正在获取局域网分享状态。'
              : enabled
                ? '同一局域网内打开本应用的设备会出现在这里。'
                : '在下方「设置」里开启后，才能发现局域网内的设备。'
          }}
        </p>
      </div>
    </div>

    <footer class="side-foot">
      <button
        class="settings-entry"
        :class="{ active: settingsActive }"
        type="button"
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
        <span>设置</span>
      </button>
      <span class="run-status" :class="statusClass">
        <span class="run-dot"></span>
        {{ statusText }}
      </span>
    </footer>
  </nav>
</template>

<style scoped>
.sidebar {
  width: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

/* 与右栏头部同高，标题基线一致 */
.side-head {
  flex-shrink: 0;
  height: var(--panel-header-height, 40px);
  display: flex;
  align-items: center;
  padding: 0 16px;
  border-bottom: 1px solid var(--border-color);
}
.side-head h3 {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}

.list-wrap {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px 8px 12px;
}
.peer-list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* 列表项：整行可点；hover 时右侧删除按钮浮出来（绝对定位，避免文字换行跳动） */
.peer-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 10px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
  outline: none;
}
.peer-item:hover {
  background: var(--bg-hover);
}
.peer-item:focus-visible {
  box-shadow: inset 0 0 0 2px var(--primary);
}
.peer-item.active {
  background: var(--bg-active);
}
.peer-item.active .item-name {
  color: var(--text-active);
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
.item-bottom {
  /* 给 hover 时冒出来的删除按钮留位，摘要不压到按钮下面 */
  padding-right: 2px;
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

/* 删除按钮：hover / 键盘聚焦时出现 */
.delete-btn {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.92);
  color: var(--text-muted);
  cursor: pointer;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.15s, color 0.15s, background 0.15s;
}
.peer-item:hover .delete-btn,
.peer-item:focus-within .delete-btn {
  opacity: 1;
  pointer-events: auto;
}
.delete-btn:hover {
  background: #fef2f2;
  color: var(--danger);
}
.delete-btn svg {
  width: 14px;
  height: 14px;
}
/* 有删除按钮的行，摘要 / 时间给按钮让出宽度 */
.peer-item:hover .item-bottom,
.peer-item:focus-within .item-bottom {
  padding-right: 26px;
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
  font-size: 13px;
  font-weight: 600;
  color: var(--text-secondary);
}
.peer-empty-hint {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}

/* 底部：设置入口 + 运行状态点 */
.side-foot {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-top: 1px solid var(--border-color);
}
.settings-entry {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 10px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 13px;
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.settings-entry:hover {
  background: var(--bg-hover);
  color: var(--primary);
}
.settings-entry.active {
  background: var(--bg-active);
  color: var(--text-active);
  font-weight: 600;
}
.settings-entry svg {
  width: 16px;
  height: 16px;
}
.run-status {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
}
.run-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-muted);
  flex-shrink: 0;
}
.run-status.is-on {
  color: var(--success);
}
.run-status.is-on .run-dot {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.16);
}
.run-status.is-busy {
  color: var(--warning);
}
.run-status.is-busy .run-dot {
  background: var(--warning);
}
.run-status.is-error {
  color: var(--danger);
}
.run-status.is-error .run-dot {
  background: var(--danger);
}
</style>
