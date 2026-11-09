<script setup lang="ts">
import { ref, computed, nextTick, onMounted, onUnmounted, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Modal, Checkbox, ConfigProvider, message, Form, FormItem, Input, Button } from "ant-design-vue";
import type { FormInstance } from "ant-design-vue";
import zhCN from "ant-design-vue/es/locale/zh_CN";
import { useConfig } from "@/composables/useConfig";
import { usePack } from "@/composables/usePack";
import { useUpdater } from "@/composables/useUpdater";
import { useVpn } from "@/composables/useVpn";
import {
  useWorkbenchLayout,
  type AppView,
} from "@/composables/useWorkbenchLayout";
import { useEntitySelection } from "@/composables/useEntitySelection";
import { useProjectForm } from "@/composables/useProjectForm";
import { useDeleteConfirm } from "@/composables/useDeleteConfirm";
import { initPlatform, platform } from "@/platform";
import appIcon from "@/assets/app-icon.png";
import type { AttachmentMeta, EntityKind, QuickAttachmentItem } from "@/types";
import ProjectList from "@/components/ProjectList.vue";
import GroupDetail from "@/components/GroupDetail.vue";
import ProjectDetail from "@/components/ProjectDetail.vue";
import PackProgress from "@/components/PackProgress.vue";
import VpnPanel from "@/components/vpn/VpnPanel.vue";
import VpnStatusDot from "@/components/vpn/VpnStatusDot.vue";
import SettingsPanel from "@/components/settings/SettingsPanel.vue";
import CleanerPanel from "@/components/cleaner/CleanerPanel.vue";
import QuickOpenPalette from "@/components/QuickOpenPalette.vue";
import AttachmentPreview from "@/components/AttachmentPreview.vue";

const {
  config,
  loading,
  loadConfig,
  saveConfig,
  addGroup,
  deleteGroup,
  renameGroup,
  updateGroup,
  reorderGroups,
  reorderProjects,
  moveProject,
  addProject,
  updateProject,
  deleteProject,
  findGroup,
  findProject,
  updateNote,
  setDefaultUploadDir,
  addAttachment,
  removeAttachment,
  renameAttachment,
  collectAttachments,
} = useConfig();

const { packing, progress, result: packResult, error: packError, pack } = usePack();

const {
  status: updateStatus,
  info: updateInfo,
  errorMessage: updateError,
  percentage: updatePercentage,
  hasUpdatePrompt,
  checkUpdate,
  installUpdate,
  restartApp,
  startDailyCheck,
  stopDailyCheck,
  dismissCurrent,
} = useUpdater();

// VPN 与项目打包共用同一份 config.json，这里复用 useConfig 的 saveConfig 落盘
const vpn = useVpn(config, saveConfig);

// VPN 状态点：图标栏 VPN 图标右下角的连接指示
const vpnDotState = computed<"off" | "on" | "busy">(() => {
  if (vpn.isConnected) return "on";
  if (vpn.phaseBusy || vpn.connecting) return "busy";
  return "off";
});
const vpnTitle = computed(() => {
  if (vpn.isConnected) return `VPN 已连接：${vpn.status.message}`;
  if (vpn.phaseBusy) return "VPN 连接中…";
  return "VPN 未连接，点击进入代理";
});

// 工作台布局：工具切换、项目树尺寸与持久化
const {
  view,
  isMac,
  shortcutTitle,
  selectTool,
  treeWidth,
  treeCollapsed,
  vpnMenuCollapsed,
  startTreeResize,
  resetTreeWidth,
  onTreeResizerKeydown,
} = useWorkbenchLayout();

// Windows 使用自绘标题栏；macOS 仍用系统红绿灯，Linux 保留系统边框
const isWindows = computed(() => platform.is_windows);

const proxyTitle = computed(() => {
  const action =
    view.value === "proxy"
      ? vpnMenuCollapsed.value
        ? "展开 VPN 配置"
        : "收起 VPN 配置"
      : "代理";
  return vpn.isConnected ? `${action}（VPN 已连接）` : shortcutTitle(action, "2");
});

// 分组 / 项目选中状态
const {
  selection,
  selectedId,
  selectedGroup,
  selectedProject,
  ensureSelection,
  handleSelect,
} = useEntitySelection(config, loading);

// 新建分组 / 添加项目表单
const {
  showAddGroupForm,
  newGroupName,
  openAddGroupForm,
  handleAddGroup,
  showAddForm,
  newProjectName,
  newProjectSource,
  newProjectOutput,
  openAddForm,
  selectNewSource,
  selectNewOutput,
  handleAdd,
} = useProjectForm({
  addGroup,
  addProject,
  onProjectCreated: (project) => {
    selection.value = { kind: "project", id: project.id };
  },
});

// 两个弹窗都交给 antd Form 校验，校验通过后再走原有的创建逻辑
const groupFormRef = ref<FormInstance | null>(null);
const projectFormRef = ref<FormInstance | null>(null);
const groupFormModel = computed(() => ({ name: newGroupName.value }));
const projectFormModel = computed(() => ({
  name: newProjectName.value,
  source: newProjectSource.value,
  output: newProjectOutput.value,
}));

async function submitGroupForm() {
  try {
    await groupFormRef.value?.validate();
  } catch {
    return;
  }
  await handleAddGroup();
}

async function submitProjectForm() {
  try {
    await projectFormRef.value?.validate();
  } catch {
    return;
  }
  await handleAdd();
}

// 附件面板提示上传目录不可用时，跳到设置页去改
function handleOpenSettings() {
  selectTool("settings");
}

// ========== 快速搜索（⌘/Ctrl+P）：VS Code 式居中浮层 ==========
const quickOpenVisible = ref(false);

/** 把所有分组/项目的附件拍平，带上所属位置，供快速搜索展示与跳转 */
const quickItems = computed<QuickAttachmentItem[]>(() => {
  const out: QuickAttachmentItem[] = [];
  for (const group of config.value.groups) {
    for (const attachment of group.attachments ?? []) {
      out.push({
        ...attachment,
        entityKind: "group",
        entityId: group.id,
        entityPath: group.name,
      });
    }
    for (const project of group.projects ?? []) {
      for (const attachment of project.attachments ?? []) {
        out.push({
          ...attachment,
          entityKind: "project",
          entityId: project.id,
          entityPath: `${group.name} / ${project.name}`,
        });
      }
    }
  }
  return out;
});

function openQuickSearch() {
  quickOpenVisible.value = true;
}

function handleQuickKeydown(event: KeyboardEvent) {
  if (!(event.metaKey || event.ctrlKey) || event.altKey || event.shiftKey) return;
  if (event.key.toLowerCase() !== "p") return;
  // 和 VS Code 一样：无论焦点在哪个输入框，⌘P 都拉起快速搜索
  event.preventDefault();
  openQuickSearch();
}

// ⌘P 快速搜索里选中的附件直接在应用内预览（走 wst-img 协议按需解密，不落临时文件）
const quickPreviewOpen = ref(false);
const quickPreviewMeta = ref<AttachmentMeta | null>(null);

function handleQuickChoose(item: QuickAttachmentItem) {
  quickOpenVisible.value = false;
  // 附件属于别的分组/项目时先切过去，关掉预览后看到的就是它所在的页面
  if (
    selection.value?.kind !== item.entityKind ||
    selection.value?.id !== item.entityId
  ) {
    selection.value = { kind: item.entityKind, id: item.entityId };
    selectTool("home");
  }
  quickPreviewMeta.value = item;
  quickPreviewOpen.value = true;
}

onMounted(() => window.addEventListener("keydown", handleQuickKeydown));
onUnmounted(() => window.removeEventListener("keydown", handleQuickKeydown));

// 新建分组 / 添加项目弹窗都是「先选目录、后弹窗」，打开后把焦点落到名称输入框（已带出目录名，全选便于直接改写）
const groupNameInputEl = ref<{ focus: () => void; select?: () => void } | null>(
  null,
);
const projectNameInputEl = ref<{ focus: () => void; select?: () => void } | null>(
  null,
);
watch(showAddGroupForm, async (open) => {
  if (!open) return;
  await nextTick();
  groupNameInputEl.value?.focus();
  groupNameInputEl.value?.select?.();
});
watch(showAddForm, async (open) => {
  if (!open) return;
  await nextTick();
  projectNameInputEl.value?.focus();
  projectNameInputEl.value?.select?.();
});

// 删除确认弹窗
const {
  deleteModalOpen,
  deleteWithFiles,
  deleteTarget,
  requestDeleteGroup,
  requestDeleteProject,
  confirmDelete,
} = useDeleteConfirm({
  findGroup,
  findProject,
  deleteGroup,
  deleteProject,
  collectAttachments,
  ensureSelection,
});

// 托盘菜单只负责转发动作，具体行为在这里决定
let trayUnlisten: UnlistenFn | null = null;
let navUnlisten: UnlistenFn | null = null;
onMounted(async () => {
  // 托盘里的页面入口：系统 / 代理 / 设置
  navUnlisten = await listen<string>("nav-tray", (event) => {
    const map: Record<string, AppView> = {
      "nav-cleaner": "cleaner",
      "nav-proxy": "proxy",
      "nav-settings": "settings",
    };
    const target = map[event.payload];
    if (target) selectTool(target);
  });

  trayUnlisten = await listen<string>("vpn-tray", async (event) => {
    // VPN 动作统一落到「代理」页展示
    selectTool("proxy");
    switch (event.payload) {
      case "vpn-connect":
        await vpn.toggleConnection();
        break;
      case "vpn-disconnect":
        await vpn.disconnect();
        break;
    }
  });
});
onUnmounted(() => {
  trayUnlisten?.();
  trayUnlisten = null;
  navUnlisten?.();
  navUnlisten = null;
});

// 打开应用先静默检查一次；之后每天 10:00 / 16:00 自动校验
onMounted(async () => {
  await checkUpdate({ silent: true });
});
onMounted(startDailyCheck);
onUnmounted(stopDailyCheck);

onMounted(async () => {
  // 用后端的编译期平台信息覆盖 userAgent 兜底值
  await initPlatform();
});

// ========== Windows 自绘标题栏 ==========
const appWindow = getCurrentWindow();
const isMaximized = ref(false);
let resizeUnlisten: UnlistenFn | null = null;

async function syncMaximized() {
  try {
    isMaximized.value = await appWindow.isMaximized();
  } catch {
    // 窗口 API 不可用时保留上一次状态，不打断界面
  }
}

function minimizeWindow() {
  void appWindow.minimize().catch((err) => {
    message.error(`最小化窗口失败: ${err}`);
  });
}

async function toggleMaximizeWindow() {
  try {
    await appWindow.toggleMaximize();
    await syncMaximized();
  } catch (err) {
    message.error(`切换窗口大小失败: ${err}`);
  }
}

function closeWindow() {
  void appWindow.close();
}

onMounted(async () => {
  await syncMaximized();
  try {
    resizeUnlisten = await appWindow.onResized(() => {
      void syncMaximized();
    });
  } catch {
    // 窗口事件不可用时不注册，按钮仍可正常切换
  }
});
onUnmounted(() => {
  resizeUnlisten?.();
  resizeUnlisten = null;
});

const updateBusy = computed(
  () =>
    updateStatus.value === "checking" || updateStatus.value === "downloading",
);

// 只有出现新版本时才在标题栏原位置显示蓝色「更新」；平时这块留空
const updateActionable = computed(
  () =>
    updateStatus.value === "available" ||
    updateStatus.value === "downloading" ||
    updateStatus.value === "installed" ||
    // 下载/安装失败但版本信息还在时保留按钮，给一个「重试」入口
    (updateStatus.value === "error" && updateInfo.value !== null),
);

const updateButtonLabel = computed(() => {
  if (updateStatus.value === "downloading") {
    return `下载中 ${Math.floor(updatePercentage.value)}%`;
  }
  if (updateStatus.value === "installed") return "重启生效";
  if (updateStatus.value === "error") return "重试";
  return "更新";
});

const updateButtonTitle = computed(() => {
  if (updateStatus.value === "available" && updateInfo.value) {
    return `有新版本 v${updateInfo.value.version}，点击更新`;
  }
  if (updateStatus.value === "installed") return "重启应用以完成更新";
  if (updateStatus.value === "downloading") return "正在下载更新";
  if (updateStatus.value === "error") return updateError.value;
  return "更新";
});

async function installPendingUpdate() {
  const ok = await installUpdate();
  if (!ok) {
    message.error(`更新失败: ${updateError.value}`);
  }
}

async function handleUpdateAction() {
  if (updateStatus.value === "available") {
    await installPendingUpdate();
    return;
  }

  // 上一次下载失败，重试时直接从已找到的版本继续
  if (updateStatus.value === "error" && updateInfo.value) {
    await installPendingUpdate();
    return;
  }

  if (updateStatus.value === "installed") {
    await restartApp();
  }
}

function handleDismissUpdate() {
  dismissCurrent();
}

// 重命名分组（默认分组不允许重命名）
async function handleRenameGroup(id: string, name: string) {
  const group = config.value.groups.find((g) => g.id === id);
  if (!group) return;
  if (group.is_default) {
    message.warning(`默认分组「${group.name}」不可重命名`);
    return;
  }
  try {
    await renameGroup(id, name);
  } catch (err) {
    message.error(`重命名分组失败: ${err}`);
  }
}

// 笔记 / 附件（分组与项目共用，按当前选中实体落盘）
// 笔记带实体 id：切换实体时旧编辑器卸载补交，不能落到新选中项上
async function handleEntityNote(kind: EntityKind, id: string, note: string) {
  await updateNote(kind, id, note);
}

async function handleEntityAddAttachment(meta: AttachmentMeta) {
  const sel = selection.value;
  if (!sel) return;
  await addAttachment(sel.kind, sel.id, meta);
}

async function handleEntityRemoveAttachment(id: string) {
  const sel = selection.value;
  if (!sel) return;
  await removeAttachment(sel.kind, sel.id, id);
}

async function handleEntityRenameAttachment(id: string, name: string) {
  const sel = selection.value;
  if (!sel) return;
  await renameAttachment(sel.kind, sel.id, id, name);
}

async function handleUpdate(id: string, data: any) {
  await updateProject(id, data);
}

async function handleUpdateDefaultInclude(rules: string[]) {
  config.value.default_include = rules;
  await saveConfig();
}

async function handleSetDefaultUploadDir(dir: string | null) {
  try {
    await setDefaultUploadDir(dir);
    message.success(
      dir ? `全局默认上传目录已设为: ${dir}` : "已清除全局默认上传目录",
    );
  } catch (err) {
    message.error(`设置全局默认上传目录失败: ${err}`);
  }
}

async function handlePack(project: any, asZip: boolean = false) {
  await pack(project, config.value.default_include, asZip);
}
</script>

<template>
  <ConfigProvider :locale="zhCN">
  <div class="app-layout">
    <!-- 标题栏：整条可拖动窗口；macOS 下红绿灯浮在左侧，留白由 CSS 处理 -->
    <header
      class="titlebar"
      :class="{ 'titlebar-mac': isMac, 'titlebar-windows': isWindows }"
      :data-tauri-drag-region="isMac ? 'deep' : undefined"
    >
      <!-- Windows 去掉原生标题栏后，用一层透明区域负责拖动；按钮在上层不触发拖动 -->
      <!-- 空白处交给 Tauri 的 drag region：单击拖动、双击最大化都由原生处理 -->
      <div
        v-if="isWindows"
        class="titlebar-drag-layer"
        data-tauri-drag-region
      ></div>
      <!-- macOS 有原生红绿灯、应用名也在菜单栏里，标题栏不再重复品牌；
           Windows 自绘标题栏需要品牌标识，保持原样 -->
      <div v-if="!isMac" class="titlebar-brand">
        <img class="app-icon" :src="appIcon" alt="" draggable="false" />
        <span class="app-name">ws-workbench</span>
      </div>
      <!-- 居中搜索：仿 VS Code 命令中心，点击或 ⌘/Ctrl+P 呼出快速搜索 -->
      <button
        class="titlebar-search"
        :title="shortcutTitle('搜索附件', 'P')"
        aria-label="搜索附件"
        @click="openQuickSearch"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
          <circle cx="11" cy="11" r="6" />
          <path d="M15.5 15.5L20 20" />
        </svg>
        <span class="titlebar-search-label">搜索附件</span>
        <span class="titlebar-search-hint">{{ isMac ? '⌘P' : 'Ctrl+P' }}</span>
      </button>
      <div class="titlebar-actions">
        <button
          v-if="updateActionable"
          class="update-btn update-btn-titlebar update-btn-primary"
          :disabled="updateBusy"
          :title="updateButtonTitle"
          :aria-label="updateButtonTitle"
          @click="handleUpdateAction"
        >
          {{ updateButtonLabel }}
        </button>
      </div>
      <div v-if="isWindows" class="window-controls" aria-label="窗口控制">
        <button
          class="window-control"
          type="button"
          title="最小化"
          aria-label="最小化"
          @click="minimizeWindow"
        >
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M2 6h8" />
          </svg>
        </button>
        <button
          class="window-control"
          type="button"
          :title="isMaximized ? '还原' : '最大化'"
          :aria-label="isMaximized ? '还原' : '最大化'"
          @click="toggleMaximizeWindow"
        >
          <svg v-if="isMaximized" viewBox="0 0 12 12" aria-hidden="true">
            <rect x="2.2" y="3.2" width="6" height="6" rx="0.6" />
            <path d="M4.2 3.2V2.1h5.7v5.7H8.8" />
          </svg>
          <svg v-else viewBox="0 0 12 12" aria-hidden="true">
            <rect x="2" y="2" width="8" height="8" rx="0.6" />
          </svg>
        </button>
        <button
          class="window-control window-control-close"
          type="button"
          title="关闭"
          aria-label="关闭"
          @click="closeWindow"
        >
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
          </svg>
        </button>
      </div>
    </header>

    <div class="workbench">
      <!-- 工具图标栏 -->
      <nav class="rail" aria-label="工具">
        <div class="rail-group">
          <button
            class="rail-btn"
            :class="{ active: view === 'home' }"
            :aria-current="view === 'home' ? 'page' : undefined"
            :title="
              view === 'home'
                ? shortcutTitle(treeCollapsed ? '展开项目树' : '折叠项目树', '1')
                : shortcutTitle('项目', '1')
            "
            :aria-label="
              view === 'home'
                ? treeCollapsed
                  ? '展开项目树'
                  : '折叠项目树'
                : '项目'
            "
            @click="selectTool('home')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M3 7a2 2 0 0 1 2-2h4.2a2 2 0 0 1 1.6.8L12 7h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
            </svg>
          </button>
          <button
            class="rail-btn"
            :class="{ active: view === 'proxy', 'vpn-connected': vpn.isConnected }"
            :aria-current="view === 'proxy' ? 'page' : undefined"
            :aria-expanded="view === 'proxy' ? !vpnMenuCollapsed : undefined"
            aria-controls="vpn-profile-menu"
            :title="proxyTitle"
            :aria-label="
              view === 'proxy'
                ? vpnMenuCollapsed
                  ? '展开 VPN 配置'
                  : '收起 VPN 配置'
                : '代理'
            "
            @click="selectTool('proxy')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M12 3l7 3v5.2c0 4.5-3 7.9-7 9.8-4-1.9-7-5.3-7-9.8V6z" />
              <path d="M9.5 12.2l1.8 1.9 3.4-3.7" />
            </svg>
            <span class="rail-vpn-badge" :title="vpnTitle">
              <VpnStatusDot :state="vpnDotState" size="md" glow />
            </span>
          </button>
          <!-- 清理工具：系统清理（软件卸载 + 内存占用） -->
          <button
            class="rail-btn"
            :class="{ active: view === 'cleaner' }"
            :aria-current="view === 'cleaner' ? 'page' : undefined"
            :title="shortcutTitle('系统', '4')"
            aria-label="系统"
            @click="selectTool('cleaner')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M12 3.5l1.8 4.4 4.4 1.8-4.4 1.8L12 15.9l-1.8-4.4L5.8 9.7l4.4-1.8z" />
              <path d="M18.5 15l.8 1.9 1.9.8-1.9.8-.8 1.9-.8-1.9-1.9-.8 1.9-.8z" />
            </svg>
          </button>
        </div>
        <div class="rail-bottom">
          <button
            class="rail-btn"
            :class="{ active: view === 'settings' }"
            :aria-current="view === 'settings' ? 'page' : undefined"
            :title="shortcutTitle('设置', '3')"
            aria-label="设置"
            @click="selectTool('settings')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
          </button>
        </div>
      </nav>

      <!-- 项目树面板：可拖拽宽度、可折叠 -->
      <aside
        v-show="view === 'home' && !treeCollapsed"
        class="tree-panel"
        :style="{ width: treeWidth + 'px' }"
      >
        <ProjectList
          :groups="config.groups"
          :selected-id="selectedId"
          @select="handleSelect"
          @add-group="openAddGroupForm"
          @delete-group="requestDeleteGroup"
          @rename-group="handleRenameGroup"
          @add-project="openAddForm"
          @delete-project="requestDeleteProject"
          @reorder-groups="reorderGroups"
          @reorder-projects="reorderProjects"
          @move-project="moveProject"
        />
      </aside>
      <div
        v-show="view === 'home' && !treeCollapsed"
        class="resizer resizer-tree"
        role="separator"
        aria-orientation="vertical"
        aria-label="调整项目树宽度"
        tabindex="0"
        title="拖动调整项目树宽度，双击恢复默认"
        @pointerdown="startTreeResize"
        @keydown="onTreeResizerKeydown"
        @dblclick="resetTreeWidth"
      ></div>

      <!-- 工作区：按工具切换内容 -->
      <main class="workspace">
        <div v-if="hasUpdatePrompt" class="update-banner">
          <span class="update-banner-text">
            发现新版本 <strong>v{{ updateInfo?.version }}</strong>
            <span class="update-banner-sub"
              >（当前 v{{ updateInfo?.currentVersion }}）</span
            >
          </span>
          <div class="update-banner-actions">
            <button
              class="btn btn-primary"
              :disabled="updateBusy"
              @click="handleUpdateAction"
            >
              {{ updateBusy ? "下载中…" : "立即更新" }}
            </button>
            <button class="btn btn-secondary" @click="handleDismissUpdate">
              稍后
            </button>
          </div>
        </div>

        <!-- 项目工具 -->
        <div v-show="view === 'home'" class="workspace-body home-body">
          <div class="config-section">
            <GroupDetail
              v-if="selection && selection.kind === 'group' && selectedGroup"
              :key="'group-' + selection.id"
              :group="selectedGroup"
              :default-upload-dir="config.default_upload_dir"
              @update-note="(id, note) => handleEntityNote('group', id, note)"
              @open-settings="handleOpenSettings"
              @add-attachment="handleEntityAddAttachment"
              @remove-attachment="handleEntityRemoveAttachment"
              @rename-attachment="handleEntityRenameAttachment"
            />
            <ProjectDetail
              v-else-if="selection && selection.kind === 'project' && selectedProject"
              :key="'project-' + selection.id"
              :project="selectedProject"
              :default-upload-dir="config.default_upload_dir"
              @update="handleUpdate"
              @pack="handlePack"
              @update-note="(id, note) => handleEntityNote('project', id, note)"
              @open-settings="handleOpenSettings"
              @add-attachment="handleEntityAddAttachment"
              @remove-attachment="handleEntityRemoveAttachment"
              @rename-attachment="handleEntityRenameAttachment"
            />
            <div v-else class="no-selection">
              <p>请选择左侧的分组或项目</p>
            </div>
          </div>

          <PackProgress :progress="progress" :packing="packing" :result="packResult" :error="packError" />
        </div>

        <!-- 代理工具 -->
        <div v-show="view === 'proxy'" class="workspace-body">
          <VpnPanel
            :vpn="vpn"
            :active="view === 'proxy'"
            :menu-collapsed="vpnMenuCollapsed"
          />
        </div>

        <!-- 清理工具 -->
        <div v-show="view === 'cleaner'" class="workspace-body">
          <CleanerPanel :active="view === 'cleaner'" />
        </div>

        <!-- 设置工具 -->
        <div v-show="view === 'settings'" class="workspace-body">
          <SettingsPanel
            :default-upload-dir="config.default_upload_dir"
            :default-include="config.default_include"
            @update-default-upload-dir="handleSetDefaultUploadDir"
            @update-default-include="handleUpdateDefaultInclude"
          />
        </div>

      </main>
    </div>

      <!-- 新建分组弹窗：分组只需要名称 -->
      <Modal
        v-model:open="showAddGroupForm"
        title="新建分组"
        ok-text="确定"
        cancel-text="取消"
        :mask-closable="false"
        @ok="submitGroupForm"
      >
        <Form
          ref="groupFormRef"
          :model="groupFormModel"
          layout="vertical"
          @submit.prevent
        >
          <FormItem
            label="分组名称"
            name="name"
            :rules="[
              { required: true, whitespace: true, message: '请填写分组名称' },
            ]"
          >
            <Input
              ref="groupNameInputEl"
              v-model:value="newGroupName"
              placeholder="请输入分组名称"
              @keydown.enter.prevent="submitGroupForm"
            />
          </FormItem>
        </Form>
      </Modal>

      <!-- 添加项目弹窗：目录已在点击「添加项目」时选好，这里补名称与输出目录 -->
      <Modal
        v-model:open="showAddForm"
        title="添加项目"
        :width="560"
        ok-text="确定"
        cancel-text="取消"
        :mask-closable="false"
        @ok="submitProjectForm"
      >
        <Form
          ref="projectFormRef"
          :model="projectFormModel"
          layout="vertical"
          @submit.prevent
        >
          <FormItem label="项目目录">
            <div class="dir-row">
              <Input
                :value="newProjectSource"
                :title="newProjectSource"
                readonly
              />
              <Button @click="selectNewSource">重新选择</Button>
            </div>
          </FormItem>
          <FormItem
            label="项目名称"
            name="name"
            :rules="[
              { required: true, whitespace: true, message: '请填写项目名称' },
            ]"
          >
            <Input
              ref="projectNameInputEl"
              v-model:value="newProjectName"
              placeholder="项目名称"
              @keydown.enter.prevent="submitProjectForm"
            />
          </FormItem>
          <FormItem label="输出目录（可选，默认为项目目录-dist）">
            <div class="dir-row">
              <Input
                v-model:value="newProjectOutput"
                placeholder="选择输出目录"
                @keydown.enter.prevent="submitProjectForm"
              />
              <Button @click="selectNewOutput">选择</Button>
            </div>
          </FormItem>
        </Form>
      </Modal>

      <!-- 删除确认弹窗：单次确认 + 默认不勾选的「同时删除附件文件」 -->
      <Modal
        v-model:open="deleteModalOpen"
        title="删除确认"
        ok-text="确认删除"
        cancel-text="取消"
        ok-type="danger"
        :mask-closable="false"
        @ok="confirmDelete"
      >
        <p class="delete-modal-text">
          确认删除{{ deleteTarget?.kind === "group" ? "分组" : "项目" }}「{{
            deleteTarget?.name
          }}」{{ deleteTarget?.suffix }}？此操作不可恢复。
        </p>
        <Checkbox v-model:checked="deleteWithFiles">同时删除附件文件</Checkbox>
      </Modal>

      <!-- VS Code 式快速搜索：⌘/Ctrl+P 或标题栏搜索按钮呼出 -->
      <QuickOpenPalette
        v-if="quickOpenVisible"
        :items="quickItems"
        @close="quickOpenVisible = false"
        @choose="handleQuickChoose"
      />

      <!-- 快速搜索选中的附件：应用内预览，不再解密到临时文件 -->
      <AttachmentPreview v-model:open="quickPreviewOpen" :meta="quickPreviewMeta" />
  </div>
  </ConfigProvider>
</template>

<style scoped>
.app-layout {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: var(--bg-primary);
}

/* 标题栏：承载窗口拖动、当前工具、版本与更新按钮 */
.titlebar {
  position: relative;
  flex-shrink: 0;
  height: 38px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 0 12px 0 16px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-primary);
}
.titlebar-mac {
  padding-left: 78px;
}
.titlebar-windows {
  padding-right: 0;
}
/* 透明拖动层在内容下面；品牌、搜索、更新按钮和窗口按钮都在上层 */
.titlebar-drag-layer {
  position: absolute;
  inset: 0;
  z-index: 0;
}
.titlebar > * {
  z-index: 1;
}
.titlebar > .titlebar-drag-layer {
  z-index: 0;
}
.titlebar-brand {
  display: flex;
  align-items: center;
  min-width: 0;
  z-index: 2;
  /* 品牌区域只做展示，鼠标事件交给下面的拖动层 */
  pointer-events: none;
  user-select: none;
}
.app-icon {
  width: 16px;
  height: 16px;
  margin-right: 8px;
  object-fit: contain;
  flex-shrink: 0;
}
.app-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
}
.titlebar-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  /* 窗口按钮是第三个 flex 子项，必须把 actions 推到右侧，否则 space-between
     会把它放在标题栏正中 */
  margin-left: auto;
  z-index: 2;
}
.titlebar-windows .titlebar-actions {
  margin-right: 8px;
}
.window-controls {
  display: flex;
  align-self: stretch;
  flex-shrink: 0;
  z-index: 2;
}
.window-control {
  width: 46px;
  height: 100%;
  display: grid;
  place-items: center;
  padding: 0;
  border: 0;
  border-radius: 0;
  background: transparent;
  color: var(--text-secondary);
  cursor: default;
}
.window-control:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.window-control:focus-visible {
  outline: 1px solid var(--primary);
  outline-offset: -2px;
}
.window-control svg {
  width: 12px;
  height: 12px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1;
}
.window-control-close:hover {
  background: #c42b1c;
  color: #fff;
}
.titlebar-search {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  z-index: 2;
  display: flex;
  align-items: center;
  gap: 6px;
  width: min(440px, 38vw);
  height: 24px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-secondary);
  color: var(--text-muted);
  cursor: pointer;
}
.titlebar-search:hover {
  border-color: var(--primary);
  color: var(--text-secondary);
}
.titlebar-search svg {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
}
.titlebar-search-label {
  font-size: 12px;
}
.titlebar-search-hint {
  margin-left: auto;
  font-size: 12px;
  color: var(--text-muted);
}

/* 图标栏 + 项目树 + 工作区 */
.workbench {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
}
.rail {
  width: 56px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  padding: 8px 0;
  border-right: 1px solid var(--border-color);
  background: var(--bg-primary);
}
.rail-group,
.rail-bottom {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
}
.rail-btn {
  position: relative;
  width: 40px;
  height: 40px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.rail-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.rail-btn.active {
  background: rgba(59, 130, 246, 0.1);
  color: var(--primary);
}
.rail-btn.active::before {
  content: "";
  position: absolute;
  left: -8px;
  top: 8px;
  bottom: 8px;
  width: 4px;
  border-radius: 0 2px 2px 0;
  background: var(--primary);
}
.rail-btn.vpn-connected,
.rail-btn.vpn-connected:hover {
  color: var(--success);
}
.rail-btn.vpn-connected.active {
  background: rgba(16, 185, 129, 0.1);
}
.rail-btn.vpn-connected.active::before {
  background: var(--success);
}
.rail-btn:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 2px;
}
.rail-btn svg {
  width: 20px;
  height: 20px;
}
.rail-vpn-badge {
  position: absolute;
  right: 4px;
  bottom: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: var(--bg-primary);
  line-height: 0;
}

/* 项目树面板 */
.tree-panel {
  position: relative;
  flex-shrink: 0;
  min-width: 0;
  display: flex;
  flex-direction: column;
  border-right: 1px solid var(--border-color);
  background: var(--bg-primary);
  overflow: hidden;
}
/* 拖拽分隔条 */
.resizer {
  flex-shrink: 0;
  position: relative;
  background: transparent;
  touch-action: none;
  transition: background 0.15s;
}
.resizer:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: -2px;
}
.resizer-tree {
  width: 6px;
  cursor: col-resize;
}
.resizer:hover {
  background: var(--primary);
  opacity: 0.45;
}

/* 工作区 */
.workspace {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.workspace-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.home-body {
  overflow: hidden;
}

/* 窄窗口：先收起项目树 */
@media (max-width: 900px) {
  .tree-panel,
  .resizer-tree {
    display: none !important;
  }
}
/* 软件更新按钮（标题栏内） */
.update-btn {
  padding: 4px 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s;
  white-space: nowrap;
}
.update-btn:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--primary);
}
.update-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
/* 有更新时用「开始打包」同一套主色实心样式，避免标题栏里冒出第二个强调色 */
.update-btn-primary {
  border-color: var(--primary);
  background: var(--primary);
  color: #fff;
  font-weight: 500;
}
.update-btn-primary:hover:not(:disabled) {
  border-color: var(--primary-hover);
  background: var(--primary-hover);
  color: #fff;
}
.update-btn-titlebar {
  width: auto;
}

/* 新版本提示横幅 */
.update-banner {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 20px;
  background: rgba(37, 99, 235, 0.08);
  border-bottom: 1px solid rgba(37, 99, 235, 0.25);
}
.update-banner-text {
  font-size: 14px;
  color: var(--text-primary);
}
.update-banner-text strong {
  color: var(--primary);
}
.update-banner-sub {
  color: var(--text-muted);
}
.update-banner-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
.config-section {
  flex: 1;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

/* 添加项目弹窗里的目录选择行 */
.dir-row {
  display: flex;
  gap: 8px;
}
.dir-row .ant-input,
.dir-row .ant-input-affix-wrapper {
  flex: 1;
  min-width: 0;
}

/* 删除确认弹窗正文 */
.delete-modal-text {
  font-size: 14px;
  color: var(--text-secondary);
  line-height: 1.6;
  margin-bottom: 12px;
}

/* 未选中任何分组/项目时的占位 */
.no-selection {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  font-size: 14px;
}
</style>
