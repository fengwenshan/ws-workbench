<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { Button, Modal, message } from "ant-design-vue";
import {
  listOpenWithApps,
  resourceInstall,
  resourceList,
  resourceUninstall,
  type ResourceStatus,
} from "@/api/system";

const emit = defineEmits<{
  /** 检查/安装在忙，设置页顶部「重新检测」按钮据此显示检测中 */
  busy: [value: boolean];
}>();

const resources = ref<ResourceStatus[]>([]);
const loading = ref(false);
const busyId = ref("");
const root = ref<HTMLElement | null>(null);
const deepseekIcon = ref<string | null>(null);
let loaded = false;

/** DeepSeek 方块图标：复用「打开方式」里取到的应用图标 */
async function loadDeepseekIcon() {
  try {
    const apps = await listOpenWithApps();
    deepseekIcon.value = apps.find((app) => app.id === "deepseek")?.icon ?? null;
  } catch {
    deepseekIcon.value = null;
  }
}

async function load(force: boolean) {
  loading.value = true;
  emit("busy", true);
  try {
    resources.value = await resourceList(force);
    loaded = true;
  } catch (err) {
    message.error(`资源检查失败: ${err}`);
  } finally {
    loading.value = false;
    emit("busy", false);
  }
}

/** 设置页顶部「重新检测」调用：强制忽略 5 分钟缓存再查一遍 */
async function recheck() {
  await load(true);
}

async function install(resource: ResourceStatus) {
  busyId.value = resource.id;
  emit("busy", true);
  try {
    await resourceInstall(resource.id);
    message.success(resource.installed ? "插件已更新" : "插件已安装");
    await load(false);
  } catch (err) {
    message.error(`安装失败: ${err}`);
  } finally {
    busyId.value = "";
    emit("busy", false);
  }
}

function uninstall(resource: ResourceStatus) {
  Modal.confirm({
    title: `卸载 ${resource.name}？`,
    content: "会从 DeepSeek 配置里移除插件，重启 DeepSeek 后生效。",
    okText: "卸载",
    okButtonProps: { danger: true },
    cancelText: "取消",
    onOk: async () => {
      busyId.value = resource.id;
      emit("busy", true);
      try {
        await resourceUninstall(resource.id);
        message.success("已卸载，重启 DeepSeek 后生效");
        await load(false);
      } catch (err) {
        message.error(`卸载失败: ${err}`);
      } finally {
        busyId.value = "";
        emit("busy", false);
      }
    },
  });
}

function iconFor(resource: ResourceStatus) {
  return resource.kind === "dsh-plugin" ? deepseekIcon.value : null;
}

function fallbackText(resource: ResourceStatus) {
  return resource.kind === "dsh-plugin" ? "DS" : "C";
}

function statusText(resource: ResourceStatus) {
  if (!resource.available) return "暂未开放";
  if (resource.error) return "检查失败";
  if (resource.update_available) return "有更新";
  if (resource.installed) return "已安装";
  return "未安装";
}

function statusClass(resource: ResourceStatus) {
  if (!resource.available) return "is-muted";
  if (resource.error) return "is-error";
  if (resource.update_available) return "is-update";
  if (resource.installed) return "is-ok";
  return "is-muted";
}

function versionLine(resource: ResourceStatus) {
  if (!resource.available) return resource.note || "规划中";
  const local = resource.installed_version || "—";
  const latest = resource.latest_version || "—";
  return resource.update_available
    ? `本地 ${local} → 最新 ${latest}`
    : `本地 ${local} · 最新 ${latest}`;
}

function focus() {
  root.value?.scrollIntoView({ behavior: "smooth", block: "center" });
}

onMounted(() => {
  // 设置页只挂载一次（v-show 常驻），进来约 0.6s 后静默检查一次，别和首屏抢网络
  if (!loaded) setTimeout(() => void load(false), 600);
  void loadDeepseekIcon();
  window.addEventListener("ws-focus-resources", focus);
});
onUnmounted(() => window.removeEventListener("ws-focus-resources", focus));

defineExpose({ recheck, rechecking: loading });
</script>

<template>
  <section ref="root" class="card">
    <div class="card-head res-head">
      <div>
        <h4>资源</h4>
        <p class="res-sub">可下载的插件与扩展；装好后重启对应宿主应用生效。</p>
      </div>
      <button
        type="button"
        class="btn btn-secondary res-refresh"
        :disabled="loading"
        @click="load(true)"
      >
        {{ loading ? "检测中…" : "重新检测" }}
      </button>
    </div>

    <p v-if="loading && resources.length === 0" class="res-empty">正在检查资源版本…</p>

    <div class="res-grid">
      <article
        v-for="resource in resources"
        :key="resource.id"
        class="res-tile"
        :class="{ 'is-unavailable': !resource.available }"
      >
        <div class="res-tile-head">
          <div class="res-icon" :class="`res-icon-${resource.kind}`">
            <img v-if="iconFor(resource)" :src="iconFor(resource)!" alt="" />
            <span v-else class="res-icon-text">{{ fallbackText(resource) }}</span>
          </div>
          <div class="res-title">
            <span class="res-name" :title="resource.name">{{ resource.name }}</span>
            <span class="res-version">{{ versionLine(resource) }}</span>
          </div>
        </div>

        <p class="res-desc">{{ resource.description }}</p>

        <div class="res-foot">
          <span class="res-badge" :class="statusClass(resource)">
            <span class="res-dot"></span>
            {{ statusText(resource) }}
          </span>
          <div class="res-actions">
            <Button
              v-if="resource.available && (!resource.installed || resource.update_available)"
              type="primary"
              size="small"
              :loading="busyId === resource.id"
              @click="install(resource)"
            >{{ resource.installed ? "更新" : "下载安装" }}</Button>
            <Button
              v-if="resource.available && resource.installed"
              size="small"
              danger
              :loading="busyId === resource.id"
              @click="uninstall(resource)"
            >卸载</Button>
            <Button v-if="!resource.available" size="small" disabled>敬请期待</Button>
          </div>
        </div>

        <p v-if="resource.error" class="res-error">{{ resource.error }}</p>
        <p v-else-if="resource.note && resource.available" class="res-note">{{ resource.note }}</p>
      </article>
    </div>
  </section>
</template>

<style scoped>
/* 卡片外壳（.card）由设置页的样式提供；这里只管内部布局 */
.res-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}
.res-head h4 {
  margin: 0;
  font-size: 14px;
  color: var(--text-primary);
}
.res-sub {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--text-muted);
}
.res-refresh {
  flex-shrink: 0;
  font-size: 12px;
}

.res-empty {
  margin: 8px 0 0;
  font-size: 12px;
  color: var(--text-muted);
}

/* 应用方块：自适应网格，窄了自动换行 */
.res-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 12px;
  margin-top: 12px;
}

.res-tile {
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 140px;
  padding: 14px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-secondary);
  transition:
    border-color 0.15s ease,
    box-shadow 0.15s ease,
    transform 0.15s ease;
}
.res-tile:hover {
  border-color: var(--primary);
  box-shadow: 0 6px 16px rgba(15, 23, 42, 0.08);
  transform: translateY(-1px);
}
.res-tile.is-unavailable {
  opacity: 0.72;
}

.res-tile-head {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}

/* 方块图标，像启动器里的应用图标 */
.res-icon {
  flex-shrink: 0;
  width: 44px;
  height: 44px;
  border-radius: 11px;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background: linear-gradient(160deg, #e2e8f0, #cbd5e1);
  box-shadow: inset 0 0 0 1px rgba(15, 23, 42, 0.05);
}
.res-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.res-icon-text {
  font-size: 15px;
  font-weight: 600;
  letter-spacing: 0.5px;
  color: #475569;
}
.res-icon-dsh-plugin {
  background: linear-gradient(160deg, #4f7cff, #2b4fd6);
}
.res-icon-dsh-plugin .res-icon-text {
  color: #fff;
}
.res-icon-chrome-extensions {
  background: linear-gradient(160deg, #f5b64a, #e07c31);
}
.res-icon-chrome-extensions .res-icon-text {
  color: #fff;
}

.res-title {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}
.res-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.res-version {
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.res-desc {
  margin: 0;
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-secondary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* 状态 + 操作固定在方块底部 */
.res-foot {
  margin-top: auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}
.res-badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 22px;
  padding: 0 9px;
  border-radius: 11px;
  font-size: 11px;
  white-space: nowrap;
  background: var(--bg-hover);
  color: var(--text-secondary);
}
.res-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: currentColor;
  opacity: 0.9;
}
.res-badge.is-ok {
  color: #059669;
  background: rgba(16, 185, 129, 0.12);
}
.res-badge.is-update {
  color: #b45309;
  background: rgba(245, 158, 11, 0.16);
}
.res-badge.is-error {
  color: #dc2626;
  background: rgba(239, 68, 68, 0.12);
}
.res-badge.is-muted {
  color: var(--text-muted);
}

.res-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.res-error,
.res-note {
  margin: 0;
  font-size: 12px;
  line-height: 1.5;
}
.res-error {
  color: #dc2626;
}
.res-note {
  color: #b45309;
}
</style>
