<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Button, Input, Modal } from 'ant-design-vue'
import AppIcon from './AppIcon.vue'
import type { PopupWindow } from '@/types/popup'

/**
 * 「从窗口添加规则」：列出当前可见的顶层窗口，挑一个就打开规则确认框。
 *
 * 比让用户手填进程名/类名可靠得多；系统外壳窗口会标成不可拦截，
 * 已经在规则里的窗口也标出来，避免重复添加。
 */
const props = defineProps<{
  open: boolean
  windows: PopupWindow[]
  loading: boolean
  appName: (path: string, process: string) => string
  appIcon: (path: string) => string | null
}>()

const emit = defineEmits<{
  'update:open': [boolean]
  refresh: []
  pick: [window: PopupWindow]
}>()

const keyword = ref('')

watch(
  () => props.open,
  (open) => {
    if (open) keyword.value = ''
  },
)

const filtered = computed(() => {
  const needle = keyword.value.trim().toLowerCase()
  if (!needle) return props.windows
  return props.windows.filter((window) =>
    [window.title, window.process, window.class_name, props.appName(window.path, window.process)]
      .join(' ')
      .toLowerCase()
      .includes(needle),
  )
})

function close(): void {
  emit('update:open', false)
}
</script>

<template>
  <Modal :open="open" title="从正在运行的窗口添加规则" :width="680" @cancel="close">
    <div class="picker">
      <div class="picker-toolbar">
        <Input
          v-model:value="keyword"
          size="small"
          allow-clear
          placeholder="搜索程序名、窗口标题或类名"
          class="picker-search"
        />
        <Button size="small" :loading="loading" @click="emit('refresh')">刷新列表</Button>
      </div>

      <div v-if="loading && !windows.length" class="picker-empty">正在读取窗口列表…</div>
      <div v-else-if="!filtered.length" class="picker-empty">
        {{ keyword ? '没有匹配的窗口' : '当前没有可添加的窗口' }}
      </div>
      <div v-else class="picker-list">
        <div v-for="(window, index) in filtered" :key="`${window.process}-${window.title}-${index}`" class="picker-row">
          <AppIcon
            :name="appName(window.path, window.process)"
            :icon="appIcon(window.path)"
            :size="30"
          />
          <div class="picker-main">
            <div class="picker-title" :title="window.title">{{ window.title || '（无标题窗口）' }}</div>
            <div class="picker-sub">
              {{ appName(window.path, window.process) }} · {{ window.process }} ·
              {{ window.class_name || '无类名' }} · {{ window.width }}×{{ window.height }} ·
              {{ window.position }}
            </div>
          </div>
          <span v-if="window.ruled" class="tag tag-rule">已有规则</span>
          <span v-if="window.score > 0" class="tag tag-score" :title="window.reasons.join('；')">
            可疑 {{ window.score }}
          </span>
          <span v-if="window.main" class="picker-protected" title="应用主窗口不会被拦截">
            主窗口
          </span>
          <button
            v-else-if="window.blockable"
            type="button"
            class="btn btn-secondary btn-sm"
            @click="emit('pick', window)"
          >
            拦截
          </button>
          <span v-else-if="window.ignored" class="picker-protected" title="已在忽略名单里，移出后才能加规则">
            已忽略
          </span>
          <span v-else class="picker-protected">系统窗口</span>
        </div>
      </div>
    </div>

    <template #footer>
      <div class="picker-footer">
        <span class="picker-hint">
          只列当前可见的普通窗口；应用主窗口、系统外壳窗口和本应用自身不会被拦截。
        </span>
        <Button @click="close">关闭</Button>
      </div>
    </template>
  </Modal>
</template>

<style scoped>
.picker {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.picker-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
}

.picker-search {
  flex: 1;
  min-width: 0;
}

.picker-list {
  max-height: 380px;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: 8px;
}

.picker-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  border-bottom: 1px solid var(--border-color);
}

.picker-row:last-child {
  border-bottom: none;
}

.picker-row:hover {
  background: var(--bg-hover);
}

.picker-main {
  flex: 1;
  min-width: 0;
}

.picker-title {
  color: var(--text-primary);
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.picker-sub {
  margin-top: 2px;
  color: var(--text-muted);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.picker-empty {
  padding: 36px 0;
  color: var(--text-muted);
  font-size: 12px;
  text-align: center;
}

.picker-protected {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 11px;
}

.btn-sm {
  padding: 2px 10px;
  font-size: 12px;
}

.tag {
  flex-shrink: 0;
  padding: 1px 6px;
  border-radius: 4px;
  font-size: 11px;
}

.tag-rule {
  background: rgba(59, 130, 246, 0.12);
  color: var(--primary);
}

.tag-score {
  background: rgba(245, 158, 11, 0.14);
  color: #b45309;
}

.picker-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.picker-hint {
  color: var(--text-muted);
  font-size: 11px;
}
</style>
