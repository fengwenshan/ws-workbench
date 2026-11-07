<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Switch, message } from 'ant-design-vue'
import { useStartup } from '@/composables/useStartup'
import { platform } from '@/platform'
import type { StartupItem } from '@/types/startup'

/**
 * 开机自启页签（设计 4.2）。
 *
 * 顶部是「本应用登录时启动」开关，下面是启动项列表：名称 / 来源 / 范围 / 状态 /
 * 操作。系统级与 RunOnce 条目 `can_toggle=false`，置灰展示只读标签并给出原因；
 * macOS 首次读登录项会触发「自动化」授权，被拒绝时后端降级返回一条提示项，
 * 这里在列表上方显示提示条，不影响 LaunchAgents 的启停。
 *
 * 数据懒加载：整个工具页不可见或页签没切过来时不去碰注册表 / launchctl。
 */
const props = defineProps<{
  /** 当前页签是否激活（父级 CleanerPanel 传入） */
  active: boolean
}>()

const startup = useStartup()
/** 首次真正进入页签时才拉数据 */
const started = ref(false)

watch(
  () => props.active,
  (active) => {
    if (!active || started.value) return
    started.value = true
    void startup.load()
  },
  { immediate: true },
)

// ============================ 展示映射 ============================

const SOURCE_LABELS: Record<string, string> = {
  run: '注册表 Run',
  run_once: '注册表 RunOnce',
  startup_folder: '启动文件夹',
  launch_agent: 'LaunchAgent',
  login_item: '登录项',
}

function sourceLabel(source: string): string {
  return SOURCE_LABELS[source] ?? source
}

function scopeLabel(scope: string): string {
  return scope === 'machine' ? '所有用户' : '当前用户'
}

function isToggling(item: StartupItem): boolean {
  return startup.togglingIds.includes(item.id)
}

/** 本应用自启写在哪里：给用户一个可核对的路径 */
const autostartLocation = computed(() =>
  platform.is_macos
    ? '~/Library/LaunchAgents/ws-workbench.plist'
    : 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\\ws-workbench',
)

const readonlyCount = computed(
  () => startup.items.length - startup.toggleableCount,
)

// ============================ 动作 ============================

async function toggleItem(item: StartupItem, checked: boolean | string | number) {
  const enabled = !!checked
  const ok = await startup.setEnabled(item, enabled)
  if (!ok && startup.actionError) message.warning(startup.actionError)
}

async function toggleAppAutostart(checked: boolean | string | number) {
  const ok = await startup.setAppAutostart(!!checked)
  if (!ok && startup.appAutostartError) message.warning(startup.appAutostartError)
}
</script>

<template>
  <div class="startup-panel">
    <!-- 本应用登录启动 -->
    <div class="startup-card">
      <div class="startup-card-text">
        <span class="startup-card-title">本应用登录时启动</span>
        <span class="startup-card-desc">
          登录系统后自动打开 ws-workbench（写入 {{ autostartLocation }}）。
        </span>
        <span v-if="startup.appAutostartError" class="startup-card-error">
          {{ startup.appAutostartError }}
        </span>
      </div>
      <Switch
        :checked="startup.appAutostart"
        :loading="startup.appAutostartLoading || startup.appAutostartSaving"
        :disabled="startup.appAutostartLoading || startup.appAutostartSaving"
        :title="startup.appAutostart ? '关闭登录时启动' : '登录时自动启动'"
        @change="(checked: any) => toggleAppAutostart(checked)"
      />
    </div>

    <!-- 登录项自动化授权被拒（macOS）：只提示，不阻塞 LaunchAgents -->
    <p v-if="startup.loginItemsUnavailable" class="login-warn">
      <span>{{ startup.loginItemsDetail }}</span>
    </p>

    <!-- 工具条 -->
    <div class="startup-toolbar">
      <span class="startup-count">
        共 {{ startup.items.length }} 项 · 可切换 {{ startup.toggleableCount }} 项<template
          v-if="readonlyCount > 0"
        >
          · 只读 {{ readonlyCount }} 项</template
        >
      </span>
      <button
        type="button"
        class="btn btn-secondary btn-xs"
        :disabled="startup.loading"
        @click="startup.refresh()"
      >
        {{ startup.loading ? '刷新中…' : '刷新' }}
      </button>
    </div>

    <!-- 错误条 -->
    <p v-if="startup.error" class="error-bar">
      <span>{{ startup.error }}</span>
      <button class="btn btn-secondary btn-xs" @click="startup.loadItems()">重试</button>
    </p>
    <p v-if="startup.actionError" class="error-bar">
      <span>{{ startup.actionError }}</span>
    </p>

    <!-- 加载 / 空态 -->
    <div v-if="startup.loading && startup.items.length === 0" class="startup-empty">
      正在读取开机启动项…
    </div>
    <div v-else-if="startup.items.length === 0" class="startup-empty">
      没有发现开机启动项。
    </div>

    <!-- 列表 -->
    <div v-else class="startup-scroll">
      <div class="startup-table">
        <div class="startup-row startup-header">
          <span>启动项</span>
          <span>来源</span>
          <span>范围</span>
          <span>状态</span>
          <span class="col-action">操作</span>
        </div>
        <div
          v-for="item in startup.items"
          :key="item.id"
          class="startup-row"
          :class="{ 'startup-row-readonly': !item.can_toggle }"
          :title="item.detail || undefined"
        >
          <div class="col-item">
            <span class="item-name">{{ item.name }}</span>
            <span class="item-command mono" :title="item.command">
              {{ item.command || '—' }}
            </span>
          </div>
          <span class="col-source">{{ sourceLabel(item.source) }}</span>
          <span class="col-scope">
            <span class="tag" :class="item.scope === 'machine' ? 'tag-muted' : 'tag-ok'">
              {{ scopeLabel(item.scope) }}
            </span>
          </span>
          <span class="col-status">
            <span class="tag" :class="item.enabled ? 'tag-ok' : 'tag-muted'">
              {{ item.enabled ? '已启用' : '已禁用' }}
            </span>
          </span>
          <div class="col-action">
            <span
              v-if="!item.can_toggle"
              class="tag tag-warn"
              :title="item.detail"
            >
              只读
            </span>
            <Switch
              v-else
              size="small"
              :checked="item.enabled"
              :loading="isToggling(item)"
              :disabled="isToggling(item)"
              :title="item.enabled ? '禁用（不删除启动项）' : '启用'"
              @change="(checked: any) => toggleItem(item, checked)"
            />
          </div>
        </div>
      </div>
    </div>

    <p class="startup-note">
      禁用只切换启用状态，不会删除启动项本身；系统级条目（HKLM / 公共启动文件夹 /
      /Library/LaunchAgents）与 RunOnce 需要更高权限，本版本只读展示。
      鼠标悬停行可查看来源位置与只读原因。
    </p>
  </div>
</template>

<style scoped>
.startup-panel {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 20px 16px;
  overflow: hidden;
}

/* 顶部：本应用登录启动 */
.startup-card {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 10px 14px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
}
.startup-card-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.startup-card-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}
.startup-card-desc {
  font-size: 12px;
  color: var(--text-muted);
  word-break: break-all;
}
.startup-card-error {
  font-size: 12px;
  color: var(--danger);
}

/* 登录项授权提示条 */
.login-warn {
  flex-shrink: 0;
  padding: 7px 12px;
  border: 1px solid rgba(245, 158, 11, 0.32);
  border-radius: 6px;
  background: #fffbeb;
  color: #b45309;
  font-size: 12px;
  line-height: 1.5;
}

/* 工具条 */
.startup-toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 10px;
}
.startup-count {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--text-muted);
}
.btn-xs {
  height: 24px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 5px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}
.btn-xs:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--primary);
}
.btn-xs:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* 错误条 */
.error-bar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 12px;
  border: 1px solid rgba(239, 68, 68, 0.28);
  border-radius: 6px;
  background: #fef2f2;
  color: var(--danger);
  font-size: 12px;
}
.error-bar span {
  flex: 1;
  min-width: 0;
  word-break: break-all;
}

/* 空态 / 加载态 */
.startup-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px 12px;
  font-size: 13px;
  color: var(--text-muted);
  text-align: center;
}

/* 列表 */
.startup-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
}
.startup-table {
  /* 5 列 + 间距的最小宽度；窗口更窄时横向滚动 */
  min-width: 680px;
}
.startup-row {
  display: grid;
  grid-template-columns: minmax(180px, 2fr) 110px 90px 80px 72px;
  align-items: center;
  gap: 10px;
  height: 48px;
  padding: 0 12px;
  border-bottom: 1px solid var(--border-color);
  font-size: 12px;
  color: var(--text-secondary);
}
.startup-row:last-child {
  border-bottom: none;
}
.startup-row:not(.startup-header):hover {
  background: var(--bg-hover);
}
.startup-row-readonly {
  background: var(--bg-readonly);
}
.startup-header {
  position: sticky;
  top: 0;
  z-index: 1;
  height: 32px;
  background: var(--bg-secondary);
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.2px;
}
.col-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  line-height: 1.35;
}
.item-name {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.item-command {
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.col-source {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.col-action {
  display: flex;
  justify-content: flex-end;
}
.tag {
  display: inline-block;
  font-size: 11px;
  border-radius: 9px;
  padding: 1px 8px;
  white-space: nowrap;
}
.tag-ok {
  color: var(--success);
  background: rgba(16, 185, 129, 0.1);
}
.tag-warn {
  color: var(--warning);
  background: rgba(245, 158, 11, 0.12);
}
.tag-muted {
  color: var(--text-secondary);
  background: var(--bg-tag);
}
.mono {
  font-family: 'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace;
}
.startup-note {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-muted);
  line-height: 1.5;
}
</style>
