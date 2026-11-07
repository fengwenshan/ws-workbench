<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Modal, message } from 'ant-design-vue'
import { platform } from '@/platform'
import type { DefaultAppCandidate, DefaultAppEntry } from '@/types/defaultapps'
import { useDefaultApps } from '@/composables/useDefaultApps'

/**
 * 默认应用页签：列出常见文件类型的当前默认打开应用。
 *
 * macOS 可以直接改（LaunchServices）；Windows 由于系统对 UserChoice 的保护，
 * 只能只读展示 + 引导用户走「打开方式」对话框或系统设置。
 */
const props = defineProps<{
  active: boolean
}>()

const defaults = useDefaultApps()
const isWindows = computed(() => platform.is_windows)

const pickerOpen = ref(false)
const pickerLoading = ref(false)
const pickerEntry = ref<DefaultAppEntry | null>(null)
const candidates = ref<DefaultAppCandidate[]>([])

const pickerTitle = computed(() =>
  pickerEntry.value ? `选择打开 ${pickerEntry.value.extension} 的应用` : '选择应用',
)

const loaded = ref(false)

watch(
  () => props.active,
  (active) => {
    if (!active || loaded.value) return
    loaded.value = true
    void defaults.load()
  },
  { immediate: true },
)

async function openPicker(entry: DefaultAppEntry): Promise<void> {
  pickerEntry.value = entry
  candidates.value = []
  pickerLoading.value = true
  pickerOpen.value = true
  try {
    candidates.value = await defaults.candidates(entry.extension)
  } finally {
    pickerLoading.value = false
  }
}

async function chooseCandidate(candidate: DefaultAppCandidate): Promise<void> {
  const entry = pickerEntry.value
  if (!entry) return
  const ok = await defaults.apply(entry.extension, candidate.app_id)
  if (ok) {
    message.success(`已把 ${entry.extension} 的默认应用改为 ${candidate.name}`)
    pickerOpen.value = false
  } else if (defaults.errorMessage) {
    message.error(defaults.errorMessage)
  }
}
</script>

<template>
  <div class="defaultapps">
    <div class="defaultapps-toolbar">
      <span class="defaultapps-count">
        {{ defaults.loading ? '正在读取…' : `共 ${defaults.entries.length} 种文件类型` }}
      </span>
      <div class="defaultapps-actions">
        <button
          v-if="isWindows"
          class="btn btn-secondary"
          @click="defaults.revealSettings()"
        >
          系统默认应用设置
        </button>
        <button class="btn btn-secondary" :disabled="defaults.loading" @click="defaults.load()">
          刷新
        </button>
      </div>
    </div>

    <p v-if="defaults.errorMessage" class="defaultapps-error">{{ defaults.errorMessage }}</p>

    <div v-if="defaults.loading" class="defaultapps-empty">正在读取默认应用…</div>
    <div v-else-if="!defaults.entries.length" class="defaultapps-empty">
      没有可展示的文件类型
    </div>

    <div v-else class="defaultapps-table" role="table" aria-label="默认应用列表">
      <div class="defaultapps-row defaultapps-head" role="row">
        <span>文件类型</span>
        <span>当前默认应用</span>
        <span>说明</span>
        <span>操作</span>
      </div>
      <div
        v-for="entry in defaults.entries"
        :key="entry.extension"
        class="defaultapps-row"
        role="row"
      >
        <span class="defaultapps-ext">{{ entry.extension }}</span>
        <span class="defaultapps-name">{{ entry.current_app_name }}</span>
        <span class="defaultapps-detail">{{ entry.detail }}</span>
        <span class="defaultapps-row-actions">
          <button
            v-if="entry.can_change"
            class="btn btn-secondary btn-sm"
            @click="openPicker(entry)"
          >
            更换
          </button>
          <button
            v-else-if="isWindows"
            class="btn btn-secondary btn-sm"
            @click="defaults.openWith(entry.extension)"
          >
            选择默认应用…
          </button>
          <span v-else class="defaultapps-readonly">只读</span>
        </span>
      </div>
    </div>

    <Modal
      v-model:open="pickerOpen"
      :title="pickerTitle"
      :footer="null"
      @cancel="pickerOpen = false"
    >
      <div v-if="pickerLoading" class="defaultapps-empty">正在查找可打开的应用…</div>
      <div v-else-if="!candidates.length" class="defaultapps-empty">
        没有找到能打开该类型的应用
      </div>
      <div v-else class="defaultapps-picker">
        <button
          v-for="candidate in candidates"
          :key="candidate.app_id"
          type="button"
          class="defaultapps-picker-row"
          @click="chooseCandidate(candidate)"
        >
          <span class="defaultapps-picker-name">{{ candidate.name }}</span>
          <span class="defaultapps-picker-path">{{ candidate.path }}</span>
        </button>
      </div>
    </Modal>
  </div>
</template>

<style scoped>
.btn-sm {
  padding: 3px 10px;
  font-size: 12px;
}

.defaultapps {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 20px 16px;
  overflow: hidden;
}

.defaultapps-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-shrink: 0;
}

.defaultapps-count {
  font-size: 12px;
  color: var(--text-muted);
}

.defaultapps-actions {
  display: flex;
  gap: 8px;
}

.defaultapps-error {
  margin: 0;
  padding: 8px 10px;
  border-radius: 6px;
  background: rgba(239, 68, 68, 0.08);
  color: var(--danger);
  font-size: 12px;
  flex-shrink: 0;
}

.defaultapps-empty {
  padding: 32px 0;
  text-align: center;
  font-size: 13px;
  color: var(--text-muted);
}

.defaultapps-table {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
}

.defaultapps-row {
  display: grid;
  grid-template-columns: 110px 200px minmax(120px, 1fr) 140px;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-color);
  font-size: 13px;
}

.defaultapps-row:last-child {
  border-bottom: none;
}

.defaultapps-head {
  position: sticky;
  top: 0;
  z-index: 1;
  background: var(--bg-secondary);
  color: var(--text-muted);
  font-size: 12px;
}

.defaultapps-ext {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  color: var(--text-secondary);
}

.defaultapps-name {
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.defaultapps-detail {
  color: var(--text-muted);
  font-size: 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.defaultapps-row-actions {
  display: flex;
  justify-content: flex-end;
}

.defaultapps-readonly {
  font-size: 12px;
  color: var(--text-muted);
}

.defaultapps-picker {
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 380px;
  overflow-y: auto;
}

.defaultapps-picker-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px 10px;
  border: 1px solid transparent;
  border-radius: 6px;
  background: transparent;
  text-align: left;
  cursor: pointer;
}

.defaultapps-picker-row:hover {
  border-color: var(--border-color);
  background: var(--bg-hover);
}

.defaultapps-picker-name {
  font-size: 13px;
  color: var(--text-primary);
}

.defaultapps-picker-path {
  font-size: 12px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
