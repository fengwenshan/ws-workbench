<script setup lang="ts">
import { computed, watch } from 'vue'
import { message } from 'ant-design-vue'
import { usePopup } from '@/composables/usePopup'
import type { PopupHit } from '@/types/popup'

/**
 * 弹窗拦截页签（仅 Windows）：检测模式。
 *
 * 只记录 + 展示可疑广告弹窗，不自动关闭；每条记录由用户手动「关闭 / 隐藏 / 忽略」，
 * 同进程 5 秒内只处理一次（后端限流）。能力边界写在页面底部。
 */
const props = defineProps<{
  active: boolean
}>()

const popup = usePopup()
const loading = computed(() => popup.loading)

watch(
  () => props.active,
  (active) => {
    if (active) {
      void popup.load()
      popup.startPolling()
    } else {
      popup.stopPolling()
    }
  },
  { immediate: true },
)

async function toggleDetection(): Promise<void> {
  const ok = await popup.toggle(!popup.running)
  if (!ok && popup.errorMessage) message.error(popup.errorMessage)
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString('zh-CN', { hour12: false })
}

async function handleDismiss(hit: PopupHit, action: 'close' | 'hide'): Promise<void> {
  const ok = await popup.dismiss(hit.id, action)
  if (ok) {
    message.success(action === 'close' ? `已请求关闭「${hit.title || hit.process}」` : '已隐藏该窗口')
  } else if (popup.errorMessage) {
    message.error(popup.errorMessage)
  }
}

async function toggleIntercept(): Promise<void> {
  const ok = await popup.saveSettings({ intercept: !popup.settings.intercept })
  if (!ok && popup.errorMessage) message.error(popup.errorMessage)
}

async function onActionChange(event: Event): Promise<void> {
  const action = (event.target as HTMLSelectElement).value === 'hide' ? 'hide' : 'close'
  await popup.saveSettings({ action })
}

async function onThresholdChange(event: Event): Promise<void> {
  const value = Number((event.target as HTMLSelectElement).value)
  if (Number.isFinite(value)) await popup.saveSettings({ min_score: value })
}

async function handleIgnore(hit: PopupHit): Promise<void> {
  const ok = await popup.ignore(hit.process, true)
  if (ok) message.success(`已忽略 ${hit.process} 的弹窗`)
}
</script>

<template>
  <div class="popup">
    <div class="popup-toolbar">
      <div class="popup-toolbar-left">
        <button
          class="popup-switch"
          :class="{ on: popup.running }"
          role="switch"
          :aria-checked="popup.running"
          :disabled="popup.busy || !popup.supported"
          @click="toggleDetection"
        >
          <span class="popup-switch-dot" />
          {{ popup.running ? '检测中' : '检测已关闭' }}
        </button>
        <span class="popup-count">
          {{ popup.running ? `已记录 ${popup.hits.length} 条` : '开启后只记录，不会自动关闭任何窗口' }}
        </span>
      </div>
      <div class="popup-toolbar-right">
        <button
          class="popup-switch"
          :class="{ on: popup.settings.intercept }"
          role="switch"
          :aria-checked="popup.settings.intercept"
          :disabled="popup.busy || !popup.running"
          @click="toggleIntercept"
        >
          <span class="popup-switch-dot" />
          拦截模式{{ popup.settings.intercept ? '开' : '关' }}
        </button>
        <button class="btn btn-secondary" :disabled="popup.busy" @click="popup.clear()">
          清空
        </button>
      </div>
    </div>

    <div v-if="popup.settings.intercept" class="popup-intercept-bar">
      <span class="popup-intercept-label">自动处理</span>
      <select class="popup-select" :value="popup.settings.action" @change="onActionChange">
        <option value="close">关闭窗口（WM_CLOSE）</option>
        <option value="hide">隐藏窗口（SW_HIDE）</option>
      </select>
      <span class="popup-intercept-label">可疑分 ≥</span>
      <select
        class="popup-select"
        :value="String(popup.settings.min_score)"
        @change="onThresholdChange"
      >
        <option v-for="score in [6, 8, 10, 12]" :key="score" :value="String(score)">
          {{ score }}
        </option>
      </select>
      <span class="popup-intercept-hint">
        系统进程/外壳窗口、忽略名单、同进程 5 秒冷却、每分钟上限
        {{ popup.settings.max_per_minute }} 个，都由后端兜底
      </span>
    </div>

    <p v-if="popup.errorMessage" class="popup-error">{{ popup.errorMessage }}</p>

    <div v-if="!popup.supported && !loading" class="popup-empty">
      仅 Windows 支持弹窗检测。
    </div>
    <div v-else-if="loading" class="popup-empty">正在读取检测状态…</div>
    <div v-else-if="!popup.hits.length" class="popup-empty">
      {{ popup.running ? '暂时没有检测到可疑弹窗' : '开启检测后，可疑广告弹窗会出现在这里' }}
    </div>

    <div v-else class="popup-table">
      <div class="popup-row popup-head">
        <span>时间</span>
        <span>进程</span>
        <span>标题</span>
        <span>可疑度</span>
        <span>操作</span>
      </div>
      <div v-for="hit in popup.hits" :key="hit.id" class="popup-row">
        <span class="popup-time">{{ formatTime(hit.at_ms) }}</span>
        <span class="popup-process">{{ hit.process }}</span>
        <span class="popup-title">
          {{ hit.title || '（无标题）' }}
          <span class="popup-class">
            <span v-if="hit.auto_action" class="popup-auto-tag">
              {{ hit.auto_action === 'hide' ? '已自动隐藏' : '已自动关闭' }}
            </span>
            {{ hit.class_name }} · {{ hit.width }}×{{ hit.height }}
          </span>
        </span>
        <span class="popup-score">
          <span class="popup-score-value">{{ hit.score }}</span>
          <span class="popup-reasons" :title="hit.reasons.join('；')">{{ hit.reasons.join('；') }}</span>
        </span>
        <span class="popup-actions">
          <button class="btn btn-secondary btn-sm" @click="handleDismiss(hit, 'close')">关闭</button>
          <button class="btn btn-secondary btn-sm" @click="handleDismiss(hit, 'hide')">隐藏</button>
          <button class="btn btn-secondary btn-sm" @click="handleIgnore(hit)">忽略</button>
        </span>
      </div>
    </div>

    <p class="popup-note">
      检测模式只记录，点「关闭 / 隐藏 / 忽略」才动手；拦截模式由后端闸门自动处理达标窗口，
      随时可关。两种模式都只能处理「已经显示出来」的窗口（可能闪一下），管理员权限窗口、UAC、
      锁屏和网页内嵌广告看不到。本工具不注入进程、不装驱动、不结束进程。
    </p>
  </div>
</template>

<style scoped>
.btn-sm {
  padding: 3px 10px;
  font-size: 12px;
}

.popup {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 20px 16px;
  overflow: hidden;
}

.popup-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-shrink: 0;
}

.popup-toolbar-right {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}

.popup-toolbar-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.popup-switch {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 999px;
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}

.popup-switch.on {
  border-color: var(--primary);
  background: var(--bg-active);
  color: var(--text-active);
}

.popup-switch:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.popup-switch-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-muted);
}

.popup-switch.on .popup-switch-dot {
  background: var(--success);
}

.popup-count {
  font-size: 12px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.popup-intercept-bar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  flex-shrink: 0;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
}

.popup-intercept-label {
  font-size: 12px;
  color: var(--text-secondary);
}

.popup-select {
  height: 26px;
  padding: 0 6px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-primary);
  font-size: 12px;
}

.popup-intercept-hint {
  font-size: 11px;
  color: var(--text-muted);
}

.popup-auto-tag {
  display: inline-block;
  margin-right: 6px;
  padding: 0 6px;
  border-radius: 4px;
  background: rgba(16, 185, 129, 0.12);
  color: var(--success);
  font-size: 11px;
}

.popup-error {
  margin: 0;
  padding: 8px 10px;
  border-radius: 6px;
  background: rgba(239, 68, 68, 0.08);
  color: var(--danger);
  font-size: 12px;
  flex-shrink: 0;
}

.popup-empty {
  padding: 32px 0;
  text-align: center;
  font-size: 13px;
  color: var(--text-muted);
}

.popup-table {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
}

.popup-row {
  display: grid;
  grid-template-columns: 80px 150px minmax(160px, 1fr) 220px 190px;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-color);
  font-size: 13px;
}

.popup-row:last-child {
  border-bottom: none;
}

.popup-head {
  position: sticky;
  top: 0;
  z-index: 1;
  background: var(--bg-secondary);
  color: var(--text-muted);
  font-size: 12px;
}

.popup-time {
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.popup-process {
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.popup-title {
  display: flex;
  flex-direction: column;
  gap: 2px;
  color: var(--text-primary);
  overflow: hidden;
}

.popup-class {
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.popup-score {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.popup-score-value {
  flex-shrink: 0;
  min-width: 22px;
  height: 22px;
  padding: 0 6px;
  border-radius: 11px;
  background: rgba(245, 158, 11, 0.12);
  color: var(--warning);
  font-size: 12px;
  font-weight: 600;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.popup-reasons {
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.popup-actions {
  display: flex;
  gap: 6px;
  justify-content: flex-end;
}

.popup-note {
  margin: 0;
  flex-shrink: 0;
  font-size: 11px;
  line-height: 1.6;
  color: var(--text-muted);
}
</style>
