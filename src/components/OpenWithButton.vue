<script setup lang="ts">
/**
 * 「用其它应用打开」分体按钮。
 *
 * 左半边：固定用默认应用打开目录（访达 / 资源管理器），不随菜单选择变化；
 * 右半边：菜单每一项都是「用这个应用打开当前目录」的动作，点了立刻用该应用
 * 打开，不是切换器，也不记住上次选了谁。图标由后端从 .icns（macOS）或
 * exe（Windows）转成 PNG。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { message, Modal } from 'ant-design-vue'
import {
  deepseekPluginStatus,
  installDeepseekPlugin,
  listOpenWithApps,
  openDeepseekApp,
  openDeepseekWorkspace,
  openWith,
  type DeepseekPluginStatus,
  type OpenWithApp,
} from '@/api/system'
import type { LogLevel } from '@/types'

const props = defineProps<{ path: string }>()
const emit = defineEmits<{
  log: [level: LogLevel, message: string]
}>()

const apps = ref<OpenWithApp[]>([])
const loading = ref(true)
const busy = ref(false)
const open = ref(false)
const root = ref<HTMLElement | null>(null)

const hasPath = computed(() => props.path.trim().length > 0)

/** 左半边固定用列表第一个（后端把默认应用排在第一位） */
const current = computed<OpenWithApp | null>(() => apps.value[0] ?? null)

const mainTitle = computed(() => {
  if (!hasPath.value) return '还没有选择目录'
  const name = current.value?.name ?? '默认应用'
  return `用 ${name} 打开`
})

onMounted(async () => {
  try {
    apps.value = await listOpenWithApps()
  } catch (err) {
    emit('log', 'warn', `读取打开方式失败: ${err}`)
  } finally {
    loading.value = false
  }
  document.addEventListener('mousedown', onDocumentClick)
  document.addEventListener('keydown', onDocumentKeydown)
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocumentClick)
  document.removeEventListener('keydown', onDocumentKeydown)
})

function onDocumentClick(event: MouseEvent) {
  if (!open.value) return
  const target = event.target as Node | null
  if (root.value && target && !root.value.contains(target)) {
    open.value = false
  }
}

function onDocumentKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') open.value = false
}

function toggle() {
  if (loading.value || apps.value.length === 0) return
  open.value = !open.value
}

/** 弹「是否安装插件」确认：true=安装并继续打开工作区，false=直接打开 DeepSeek */
function askInstall(status: DeepseekPluginStatus, action: string): Promise<boolean> {
  return new Promise((resolve) => {
    Modal.confirm({
      title: `检测到 DeepSeek 还没装工作区插件，是否${action}？`,
      content:
        `装好之后，点「DeepSeek」会在左侧自动新建或选中对应工作区并打开空白新会话。` +
        `插件一次性安装、离线、不改 DeepSeek 安装目录、首次安装会备份 profile 清单，` +
        `装完需要退出并重新打开一次 DeepSeek 才生效。` +
        `安装位置：${status.dsh_home}/profiles/desktop 与 ${status.dsh_home}/plugins/dsh-open-workspace。`,
      okText: `${action}插件并打开工作区`,
      cancelText: '直接打开 DeepSeek',
      onOk: () => resolve(true),
      onCancel: () => resolve(false),
    })
  })
}

/** DSH 首次启动才会生成 profile：确认后只打开 DeepSeek，让用户完成初始化 */
function askOpenFirst(): Promise<boolean> {
  return new Promise((resolve) => {
    Modal.confirm({
      title: 'DeepSeek 还没初始化',
      content: '先打开一次 DeepSeek Harness，等它完成初始化后再回来点「DeepSeek」，就能直接打开工作区。',
      okText: '先打开 DeepSeek',
      cancelText: '取消',
      onOk: () => resolve(true),
      onCancel: () => resolve(false),
    })
  })
}

/** 「打开方式 → DeepSeek」的完整流程 */
async function runDeepseek() {
  let status = await deepseekPluginStatus()
  // 1) 没装 DeepSeek：先提示去装，不尝试启动
  if (!status.app_installed) {
    Modal.warning({
      title: '还没安装 DeepSeek Harness',
      content: '请先安装 DeepSeek Harness 桌面版，打开一次完成初始化，再回来点「DeepSeek」。',
    })
    return
  }
  // 2) 装了但还没初始化过 profile：帮用户先打开一次
  if (!status.profile_ready) {
    if (await askOpenFirst()) await openDeepseekApp()
    return
  }

  // 3) 没装插件/版本旧：二次确认；不装就只打开 DeepSeek，装完继续打开工作区
  const action = status.plugin_installed ? '更新' : '安装'
  let installedNow = false
  if (!status.plugin_installed || status.needs_update) {
    const installIt = await askInstall(status, action)
    if (!installIt) {
      await openDeepseekApp()
      emit('log', 'info', `直接打开 DeepSeek（未${action}工作区插件）: ${props.path}`)
      return
    }
    status = await installDeepseekPlugin()
    installedNow = true
    message.success(`DeepSeek 工作区插件已${action}`)
    emit('log', 'info', `DeepSeek 工作区插件已${action}: ${status.installed_version}`)
  }

  // 4) 写请求 + 唤起；只有本次刚装/刚更新且 DSH 正开着才提示重启
  const restartNeeded = installedNow && status.dsh_running
  await openDeepseekWorkspace(props.path)
  emit('log', 'info', `用 DeepSeek 打开: ${props.path}`)
  if (restartNeeded) {
    Modal.info({
      title: '重启 DeepSeek 后工作区才生效',
      content:
        '插件已就绪。请退出并重新打开 DeepSeek Harness：10 分钟内重启会自动打开这个目录；' +
        '超过 10 分钟的话，重启后再点一次「DeepSeek」即可。',
    })
  }
}

/** 用指定应用打开当前目录：纯动作，不改变按钮上显示的应用 */
async function run(app: OpenWithApp) {
  if (!hasPath.value || busy.value) return
  busy.value = true
  open.value = false
  try {
    if (app.id === 'deepseek') {
      await runDeepseek()
    } else {
      await openWith(props.path, app.id)
      emit('log', 'info', `用 ${app.name} 打开: ${props.path}`)
    }
  } catch (err) {
    // 失败时给个可见反馈，否则用户只会觉得「点了没反应」
    message.error(`用 ${app.name} 打开失败: ${err}`)
    emit('log', 'error', `用 ${app.name} 打开失败: ${err}`)
  } finally {
    busy.value = false
  }
}

function openCurrent() {
  if (current.value) run(current.value)
}

/** 取不到图标时的兜底字形 */
function glyph(app: OpenWithApp | null) {
  return app && app.name ? app.name.slice(0, 1).toUpperCase() : '?'
}
</script>

<template>
  <div ref="root" class="open-with">
    <div class="ow-split" :class="{ 'is-open': open, 'is-busy': busy }">
      <button
        type="button"
        class="ow-main"
        :disabled="!hasPath || busy || !current"
        :title="mainTitle"
        @click="openCurrent"
      >
        <img v-if="current?.icon" class="ow-icon" :src="current.icon" alt="" />
        <span v-else class="ow-glyph">{{ glyph(current) }}</span>
      </button>
      <button
        type="button"
        class="ow-arrow"
        :disabled="loading || apps.length === 0"
        aria-haspopup="menu"
        :aria-expanded="open"
        title="用其它应用打开"
        @click="toggle"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path
            d="M2 3.5 5 6.5 8 3.5"
            fill="none"
            stroke="currentColor"
            stroke-width="1.4"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </button>
    </div>

    <div v-if="open" class="ow-menu" role="menu">
      <button
        v-for="app in apps"
        :key="app.id"
        type="button"
        role="menuitem"
        class="ow-item"
        @click="run(app)"
      >
        <img v-if="app.icon" class="ow-icon" :src="app.icon" alt="" />
        <span v-else class="ow-glyph">{{ glyph(app) }}</span>
        <span class="ow-name">{{ app.name }}</span>
        <span v-if="app.is_default" class="ow-tag">（默认）</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.open-with {
  position: relative;
  flex-shrink: 0;
}

/* 分体按钮：左半边用当前应用，右半边展开菜单 */
.ow-split {
  display: flex;
  align-items: stretch;
  height: 36px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  overflow: hidden;
  transition: border-color 0.15s;
}
.ow-split:hover,
.ow-split.is-open {
  border-color: var(--primary);
}

.ow-main,
.ow-arrow {
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.15s;
}
.ow-main {
  width: 38px;
  flex-shrink: 0;
}
.ow-arrow {
  width: 22px;
  border-left: 1px solid var(--border-color);
}
.ow-main:hover:not(:disabled),
.ow-arrow:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--primary);
}
.ow-main:disabled,
.ow-arrow:disabled {
  cursor: default;
  opacity: 0.45;
}
.ow-split.is-busy {
  opacity: 0.7;
}

.ow-icon {
  width: 18px;
  height: 18px;
  object-fit: contain;
  border-radius: 4px;
}
.ow-glyph {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border-radius: 4px;
  background: #e5e7eb;
  color: #4b5563;
  font-size: 12px;
  font-weight: 600;
  line-height: 1;
}

/* 下拉菜单：右对齐到按钮，避免挡住「选择」 */
.ow-menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: 40;
  min-width: 200px;
  padding: 4px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-input);
  box-shadow: 0 10px 24px rgba(17, 24, 39, 0.12);
}
.ow-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 8px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  font-size: 14px;
  text-align: left;
  white-space: nowrap;
}
.ow-item:hover {
  background: var(--bg-hover);
  color: var(--primary);
}
.ow-name {
  flex: 1;
  min-width: 0;
}
.ow-tag {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-secondary);
}
</style>
