<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { Modal, Checkbox, ConfigProvider } from "ant-design-vue";
import zhCN from "ant-design-vue/es/locale/zh_CN";
import { useConfig } from "@/composables/useConfig";
import { usePack } from "@/composables/usePack";
import { useUpdater } from "@/composables/useUpdater";
import { useVpn } from "@/composables/useVpn";
import { useApps } from "@/composables/useApps";
import {
  useWorkbenchLayout,
  type AppView,
} from "@/composables/useWorkbenchLayout";
import { useEntitySelection } from "@/composables/useEntitySelection";
import { useProjectForm } from "@/composables/useProjectForm";
import { useDeleteConfirm } from "@/composables/useDeleteConfirm";
import { initPlatform } from "@/platform";
import type { AttachmentMeta } from "@/types";
import ProjectList from "@/components/ProjectList.vue";
import GroupDetail from "@/components/GroupDetail.vue";
import ProjectDetail from "@/components/ProjectDetail.vue";
import PackProgress from "@/components/PackProgress.vue";
import LogPanel from "@/components/LogPanel.vue";
import VpnPanel from "@/components/vpn/VpnPanel.vue";
import SettingsPanel from "@/components/settings/SettingsPanel.vue";

const {
  config,
  loading,
  loadConfig,
  saveConfig,
  addGroup,
  deleteGroup,
  renameGroup,
  reorderGroups,
  reorderProjects,
  moveProject,
  addProject,
  updateProject,
  deleteProject,
  findGroup,
  findProject,
  updateNote,
  updateUploadDir,
  setDefaultUploadDir,
  addAttachment,
  removeAttachment,
  renameAttachment,
  collectAttachments,
} = useConfig();

const { packing, progress, logs, addLog, clearLogs, pack } = usePack();

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

// 外部应用定位（设置页用），与项目打包、VPN 共用同一套查找结果
const apps = useApps();

// VPN 状态点：图标栏底部的连接指示
const vpnDotClass = computed(() => {
  if (vpn.isConnected) return "connected";
  if (vpn.phaseBusy) return "busy";
  return "";
});
const vpnTitle = computed(() => {
  if (vpn.isConnected) return `VPN 已连接：${vpn.status.message}`;
  if (vpn.phaseBusy) return "VPN 连接中…";
  return "VPN 未连接，点击进入代理";
});

// 工作台布局：工具切换、项目树 / 日志面板尺寸与持久化
const {
  view,
  toolTitle,
  isMac,
  shortcutTitle,
  selectTool,
  treeWidth,
  treeCollapsed,
  setTreeCollapsed,
  logHeight,
  logOpen,
  setLogOpen,
  toggleLogs,
  startTreeResize,
  startLogResize,
  resetTreeWidth,
  resetLogHeight,
  onTreeResizerKeydown,
  onLogResizerKeydown,
} = useWorkbenchLayout();

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
  newGroupAddress,
  openAddGroupForm,
  selectGroupAddress,
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
  addLog,
  onProjectCreated: (project) => {
    selection.value = { kind: "project", id: project.id };
  },
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
  addLog,
});

// 日志面板：应用日志与 VPN 日志共用底部面板
const logSource = ref<"app" | "vpn">("app");
const activeLogs = computed(() =>
  logSource.value === "app" ? logs.value : vpn.logs,
);
function clearActiveLogs() {
  if (logSource.value === "app") clearLogs();
  else vpn.clearLogs();
}

// 托盘菜单只负责转发动作，具体行为在这里决定
let trayUnlisten: UnlistenFn | null = null;
let navUnlisten: UnlistenFn | null = null;
onMounted(async () => {
  // 托盘里的页面入口：代理 / 设置 / 日志
  navUnlisten = await listen<string>("nav-tray", (event) => {
    if (event.payload === "nav-logs") {
      setLogOpen(true);
      return;
    }
    const map: Record<string, AppView> = {
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

// 每天 10:00 / 15:00 自动校验更新
onMounted(startDailyCheck);
onUnmounted(stopDailyCheck);

// 客户端版本号（取自 tauri.conf.json）
const appVersion = ref("");
onMounted(async () => {
  try {
    appVersion.value = await getVersion();
  } catch (err) {
    addLog("warn", `读取版本号失败: ${err}`);
  }
  // 用后端的编译期平台信息覆盖 userAgent 兜底值
  await initPlatform();
});

const updateBusy = computed(
  () =>
    updateStatus.value === "checking" || updateStatus.value === "downloading",
);

const updateButtonLabel = computed(() => {
  switch (updateStatus.value) {
    case "checking":
      return "检查中…";
    case "downloading":
      return `下载中 ${Math.floor(updatePercentage.value)}%`;
    case "available":
      return `更新到 v${updateInfo.value?.version}`;
    case "up-to-date":
      return "已是最新版本";
    case "installed":
      return "重启生效";
    case "error":
      return "重试";
    default:
      return "检查更新";
  }
});

async function handleUpdateAction() {
  if (updateStatus.value === "available") {
    addLog("info", `开始下载 v${updateInfo.value?.version} …`);
    const ok = await installUpdate();
    if (ok) {
      addLog("success", `v${updateInfo.value?.version} 已安装，重启后生效`);
    } else {
      addLog("error", `更新失败: ${updateError.value}`);
    }
    return;
  }

  if (updateStatus.value === "installed") {
    await restartApp();
    return;
  }

  const found = await checkUpdate();
  if (found) {
    addLog(
      "info",
      `发现新版本 v${found.version}（当前 v${found.currentVersion}）`,
    );
  } else if (updateStatus.value === "up-to-date") {
    addLog("info", "当前已是最新版本");
  } else {
    addLog("error", `检查更新失败: ${updateError.value}`);
  }
}

function handleDismissUpdate() {
  const version = updateInfo.value?.version;
  dismissCurrent();
  addLog("info", `已忽略 v${version}，本次不再提示`);
}

// 重命名分组（默认分组不允许重命名）
async function handleRenameGroup(id: string, name: string) {
  const group = config.value.groups.find((g) => g.id === id);
  if (!group) return;
  if (group.is_default) {
    addLog("warn", `默认分组「${group.name}」不可重命名`);
    return;
  }
  const oldName = group.name;
  try {
    await renameGroup(id, name);
    addLog("info", `分组已重命名: ${oldName} → ${name}`);
  } catch (err) {
    addLog("error", `重命名分组失败: ${err}`);
  }
}

// 笔记 / 上传位置 / 附件（分组与项目共用，按当前选中实体落盘）
async function handleEntityNote(note: string) {
  const sel = selection.value;
  if (!sel) return;
  await updateNote(sel.kind, sel.id, note);
}

async function handleEntityUploadDir(dir: string | null) {
  const sel = selection.value;
  if (!sel) return;
  await updateUploadDir(sel.kind, sel.id, dir);
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

async function handleUpdateDefaultExclude(rules: string[]) {
  config.value.default_exclude = rules;
  await saveConfig();
}

async function handleSetDefaultUploadDir(dir: string | null) {
  try {
    await setDefaultUploadDir(dir);
    addLog(
      "info",
      dir ? `全局默认上传目录已设为: ${dir}` : "已清除全局默认上传目录",
    );
  } catch (err) {
    addLog("error", `设置全局默认上传目录失败: ${err}`);
  }
}

async function handlePack(project: any, asZip: boolean = false) {
  await pack(project, config.value.default_exclude, asZip);
}
</script>

<template>
  <ConfigProvider :locale="zhCN">
  <div class="app-layout">
    <!-- 标题栏：整条可拖动窗口；macOS 下红绿灯浮在左侧，留白由 CSS 处理 -->
    <header
      class="titlebar"
      :class="{ 'titlebar-mac': isMac }"
      :data-tauri-drag-region="isMac ? 'deep' : undefined"
    >
      <div class="titlebar-brand">
        <span class="app-name">ws-workbench</span>
        <span class="tool-name">{{ toolTitle }}</span>
      </div>
      <div class="titlebar-actions">
        <span v-if="appVersion" class="version-text">v{{ appVersion }}</span>
        <button
          class="update-btn update-btn-titlebar"
          :class="{
            'update-btn-primary':
              updateStatus === 'available' || updateStatus === 'installed',
          }"
          :disabled="updateBusy"
          @click="handleUpdateAction"
        >
          {{ updateButtonLabel }}
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
            :title="shortcutTitle('项目', '1')"
            aria-label="项目"
            @click="selectTool('home')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M3 7a2 2 0 0 1 2-2h4.2a2 2 0 0 1 1.6.8L12 7h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
            </svg>
          </button>
          <button
            class="rail-btn"
            :class="{ active: view === 'proxy' }"
            :aria-current="view === 'proxy' ? 'page' : undefined"
            :title="shortcutTitle('代理', '2')"
            aria-label="代理"
            @click="selectTool('proxy')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M12 3l7 3v5.2c0 4.5-3 7.9-7 9.8-4-1.9-7-5.3-7-9.8V6z" />
              <path d="M9.5 12.2l1.8 1.9 3.4-3.7" />
            </svg>
          </button>
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
        <div class="rail-bottom">
          <button
            class="rail-btn"
            :class="{ active: logOpen }"
            :aria-pressed="logOpen"
            :title="shortcutTitle('日志', 'J')"
            aria-label="日志"
            @click="toggleLogs"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <rect x="3" y="5" width="18" height="14" rx="2" />
              <path d="M7 9l3 3-3 3M13 15h4" />
            </svg>
          </button>
          <button
            class="rail-btn rail-vpn"
            :title="vpnTitle"
            aria-label="VPN 状态"
            @click="selectTool('proxy')"
          >
            <span class="rail-vpn-dot" :class="vpnDotClass"></span>
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
        <button
          class="tree-collapse"
          :title="shortcutTitle('折叠项目树', 'B')"
          aria-label="折叠项目树"
          @click="setTreeCollapsed(true)"
        >
          ‹
        </button>
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

      <!-- 工作区：按工具切换内容，日志是底部可折叠面板 -->
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
              @update-note="handleEntityNote"
              @update-upload-dir="handleEntityUploadDir"
              @add-attachment="handleEntityAddAttachment"
              @remove-attachment="handleEntityRemoveAttachment"
              @rename-attachment="handleEntityRenameAttachment"
              @log="addLog"
            />
            <ProjectDetail
              v-else-if="selection && selection.kind === 'project' && selectedProject"
              :key="'project-' + selection.id"
              :project="selectedProject"
              :default-exclude="config.default_exclude"
              :default-upload-dir="config.default_upload_dir"
              @update="handleUpdate"
              @pack="handlePack"
              @update-default-exclude="handleUpdateDefaultExclude"
              @update-note="handleEntityNote"
              @update-upload-dir="handleEntityUploadDir"
              @add-attachment="handleEntityAddAttachment"
              @remove-attachment="handleEntityRemoveAttachment"
              @rename-attachment="handleEntityRenameAttachment"
              @log="addLog"
            />
            <div v-else class="no-selection">
              <p>请选择左侧的分组或项目</p>
            </div>
          </div>

          <PackProgress :progress="progress" :packing="packing" />
        </div>

        <!-- 代理工具 -->
        <div v-show="view === 'proxy'" class="workspace-body">
          <VpnPanel :vpn="vpn" :active="view === 'proxy'" />
        </div>

        <!-- 设置工具 -->
        <div v-show="view === 'settings'" class="workspace-body">
          <SettingsPanel
            :apps="apps"
            :default-upload-dir="config.default_upload_dir"
            @update-default-upload-dir="handleSetDefaultUploadDir"
          />
        </div>

        <!-- 日志面板 -->
        <div
          v-show="logOpen"
          class="resizer resizer-log"
          role="separator"
          aria-orientation="horizontal"
          aria-label="调整日志面板高度"
          tabindex="0"
          title="拖动调整日志高度，双击恢复默认"
          @pointerdown="startLogResize"
          @keydown="onLogResizerKeydown"
          @dblclick="resetLogHeight"
        ></div>
        <section
          v-show="logOpen"
          class="log-dock"
          :style="{ height: logHeight + 'px' }"
        >
          <div class="log-dock-header">
            <div class="log-dock-tabs">
              <button
                class="log-dock-tab"
                :class="{ active: logSource === 'app' }"
                @click="logSource = 'app'"
              >
                应用
              </button>
              <button
                class="log-dock-tab"
                :class="{ active: logSource === 'vpn' }"
                @click="logSource = 'vpn'"
              >
                VPN
              </button>
            </div>
            <div class="log-dock-actions">
              <button class="log-dock-action" @click="clearActiveLogs">
                清空
              </button>
              <button
                class="log-dock-action log-dock-close"
                :title="shortcutTitle('关闭日志', 'J')"
                aria-label="关闭日志"
                @click="setLogOpen(false)"
              >
                ×
              </button>
            </div>
          </div>
          <div class="log-dock-body">
            <LogPanel :logs="activeLogs" @clear="clearActiveLogs" />
          </div>
        </section>
      </main>
    </div>

      <!-- 新建分组弹窗 -->
      <div v-if="showAddGroupForm" class="add-form-overlay">
        <div class="add-form add-form-sm">
          <h3>新建分组</h3>
          <div class="form-row">
            <label>分组名称</label>
            <input
              v-model="newGroupName"
              class="form-input"
              placeholder="请输入分组名称"
              @keydown.enter="handleAddGroup"
              autofocus
            />
          </div>
          <div class="form-row">
            <label>地址（可选）</label>
            <div class="dir-row">
              <input
                v-model="newGroupAddress"
                class="form-input"
                placeholder="留空则使用系统默认上传位置"
                @keydown.enter="handleAddGroup"
              />
              <button class="btn btn-pick" @click="selectGroupAddress">
                选择
              </button>
            </div>
          </div>
          <div class="form-actions">
            <button class="btn btn-secondary" @click="showAddGroupForm = false">
              取消
            </button>
            <button class="btn btn-primary" @click="handleAddGroup">
              确定
            </button>
          </div>
        </div>
      </div>

      <!-- 添加项目表单 -->
      <div v-if="showAddForm" class="add-form-overlay">
        <div class="add-form">
          <h3>添加项目</h3>
          <div v-if="newProjectSource" class="form-row">
            <label>项目名称</label>
            <input
              v-model="newProjectName"
              class="form-input"
              placeholder="项目名称"
            />
          </div>
          <div class="form-row">
            <label>源目录</label>
            <div class="dir-row">
              <input
                v-model="newProjectSource"
                class="form-input"
                placeholder="选择源目录"
                readonly
              />
              <button class="btn btn-pick" @click="selectNewSource">
                选择
              </button>
            </div>
            <p v-if="!newProjectSource" class="form-hint">
              选完源目录会自动带出项目名称与输出目录，两者之后都能改
            </p>
          </div>
          <div v-if="newProjectSource" class="form-row">
            <label>输出目录 (可选，默认为源目录-dist)</label>
            <div class="dir-row">
              <input
                v-model="newProjectOutput"
                class="form-input"
                placeholder="选择输出目录"
              />
              <button class="btn btn-pick" @click="selectNewOutput">
                选择
              </button>
            </div>
          </div>
          <div class="form-actions">
            <button class="btn btn-secondary" @click="showAddForm = false">
              取消
            </button>
            <button class="btn btn-primary" @click="handleAdd">确定</button>
          </div>
        </div>
      </div>

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
.titlebar-brand {
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
}
.app-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
}
.tool-name {
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
}
.titlebar-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
.version-text {
  font-size: 12px;
  color: var(--text-muted);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
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
  width: 3px;
  border-radius: 0 2px 2px 0;
  background: var(--primary);
}
.rail-btn:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 2px;
}
.rail-btn svg {
  width: 20px;
  height: 20px;
}
.rail-vpn {
  height: 32px;
}
.rail-vpn:hover {
  background: transparent;
}
.rail-vpn-dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: var(--text-muted);
  transition: background 0.15s, box-shadow 0.15s;
}
.rail-vpn-dot.connected {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.15);
}
.rail-vpn-dot.busy {
  background: var(--warning);
  animation: vpn-pulse 1.2s ease-in-out infinite;
}
@keyframes vpn-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
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
.tree-collapse {
  position: absolute;
  top: 10px;
  right: 8px;
  width: 24px;
  height: 24px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 14px;
  line-height: 1;
  cursor: pointer;
  transition: all 0.15s;
}
.tree-collapse:hover {
  border-color: var(--primary);
  color: var(--primary);
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
  width: 5px;
  cursor: col-resize;
}
.resizer-log {
  height: 5px;
  cursor: row-resize;
  border-bottom: 1px solid var(--border-color);
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

/* 日志面板（外层容器，与 LogPanel 组件自身的 .log-panel 区分） */
.log-dock {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--border-color);
  background: var(--bg-primary);
  overflow: hidden;
}
.log-dock-header {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 40px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--border-color);
}
.log-dock-tabs {
  display: flex;
  gap: 4px;
}
.log-dock-tab {
  padding: 4px 12px;
  border: 1px solid transparent;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s;
}
.log-dock-tab:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.log-dock-tab.active {
  border-color: var(--primary);
  color: var(--primary);
  font-weight: 500;
}
.log-dock-actions {
  display: flex;
  align-items: center;
  gap: 4px;
}
.log-dock-action {
  padding: 4px 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s;
}
.log-dock-action:hover {
  border-color: var(--primary);
  color: var(--primary);
}
.log-dock-close {
  width: 28px;
  padding: 4px 0;
  font-size: 16px;
  line-height: 1;
  text-align: center;
}
.log-dock-body {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
/* 日志内容自带「日志 / 清空」表头，与外层标签栏重复，这里只保留内容区 */
.log-dock-body :deep(.log-header) {
  display: none;
}

/* 窄窗口：先收起项目树，再让日志面板覆盖工作区 */
@media (max-width: 900px) {
  .tree-panel,
  .resizer-tree {
    display: none !important;
  }
}
@media (max-width: 720px) {
  .log-dock {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    max-height: 50vh;
    box-shadow: 0 -8px 24px rgba(0, 0, 0, 0.12);
    z-index: 5;
  }
  .resizer-log {
    display: none;
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
.update-btn-primary {
  border-color: var(--primary);
  color: var(--primary);
  font-weight: 500;
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
  font-size: 13px;
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

/* 弹窗 */
.add-form-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.3);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}
.add-form {
  background: var(--bg-primary);
  border-radius: 12px;
  padding: 24px;
  width: 500px;
  max-width: 90vw;
}
.add-form-sm {
  width: 420px;
}
.add-form-sm .form-actions {
  margin-top: 24px;
  padding-top: 16px;
  border-top: 1px solid var(--border-color);
}
/* 表单内的说明文字 */
.form-hint {
  margin: 8px 0 0;
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-muted);
}
.add-form h3 {
  margin-bottom: 20px;
  font-size: 18px;
  color: var(--text-primary);
}
.form-row {
  margin-bottom: 16px;
}
.form-row label {
  display: block;
  margin-bottom: 6px;
  font-size: 13px;
  color: var(--text-secondary);
}
.form-input {
  width: 100%;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  font-size: 14px;
  background: var(--bg-input);
  color: var(--text-primary);
  outline: none;
}
.form-input:focus {
  border-color: var(--primary);
}
.dir-row {
  display: flex;
  gap: 8px;
}
.dir-row .form-input {
  flex: 1;
}
.form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
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
