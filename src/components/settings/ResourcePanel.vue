<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { Button, Modal, message } from "ant-design-vue";
import {
  resourceInstall,
  resourceList,
  resourceUninstall,
  type ResourceStatus,
} from "@/api/system";

const resources = ref<ResourceStatus[]>([]);
const loading = ref(false);
const busyId = ref("");
const root = ref<HTMLElement | null>(null);
let loaded = false;

async function load(force: boolean) {
  loading.value = true;
  try {
    resources.value = await resourceList(force);
    loaded = true;
  } catch (err) {
    message.error(`资源检查失败: ${err}`);
  } finally {
    loading.value = false;
  }
}

async function install(resource: ResourceStatus) {
  busyId.value = resource.id;
  try {
    await resourceInstall(resource.id);
    message.success(resource.installed ? "插件已更新" : "插件已安装");
    await load(false);
  } catch (err) {
    message.error(`安装失败: ${err}`);
  } finally {
    busyId.value = "";
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
      try {
        await resourceUninstall(resource.id);
        message.success("已卸载，重启 DeepSeek 后生效");
        await load(false);
      } catch (err) {
        message.error(`卸载失败: ${err}`);
      } finally {
        busyId.value = "";
      }
    },
  });
}

function statusText(resource: ResourceStatus) {
  if (!resource.available) return resource.note || "暂不支持";
  if (resource.error) return "检查失败";
  if (resource.update_available) {
    return `可更新 ${resource.installed_version} → ${resource.latest_version}`;
  }
  if (resource.installed) return `已安装 ${resource.installed_version}`;
  return "未安装";
}

function focus() {
  root.value?.scrollIntoView({ behavior: "smooth", block: "center" });
}

let mountedTimer: ReturnType<typeof setTimeout> | null = null;

onMounted(() => {
  // 设置页只挂载一次（v-show 常驻），进来约 0.6s 后静默检查一次，别和首屏抢网络
  if (!loaded) mountedTimer = setTimeout(() => void load(false), 600);
  window.addEventListener("ws-focus-resources", focus);
});
onUnmounted(() => {
  if (mountedTimer) clearTimeout(mountedTimer);
  window.removeEventListener("ws-focus-resources", focus);
});
</script>

<template>
  <section ref="root" class="card">
    <div class="res-head">
      <div>
        <h3>资源</h3>
        <p class="res-sub">可下载的插件与扩展；装好后重启对应宿主应用生效。</p>
      </div>
      <Button size="small" :loading="loading" @click="load(true)">刷新</Button>
    </div>

    <div v-for="resource in resources" :key="resource.id" class="res-row">
      <div class="res-main">
        <div class="res-title">
          {{ resource.name }}
          <span class="res-state">{{ statusText(resource) }}</span>
        </div>
        <div class="res-desc">{{ resource.description }}</div>
        <div class="res-meta">
          本地 {{ resource.installed_version || "—" }} · 最新 {{ resource.latest_version || "—" }}
        </div>
        <div v-if="resource.error" class="res-error">{{ resource.error }}</div>
        <div v-if="resource.note" class="res-note">{{ resource.note }}</div>
      </div>
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
  </section>
</template>

<style scoped>
.res-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
}
.res-sub {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--text-muted);
}
.res-row {
  display: flex;
  gap: 16px;
  padding: 12px 0;
  border-top: 1px solid var(--border-color, #2a2a2a);
}
.res-row:first-of-type {
  border-top: none;
}
.res-main {
  flex: 1;
  min-width: 0;
}
.res-title {
  font-size: 13px;
  color: var(--text-primary);
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}
.res-state {
  font-size: 12px;
  color: var(--text-muted);
}
.res-desc {
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-secondary);
}
.res-meta {
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-muted);
}
.res-error {
  margin-top: 4px;
  font-size: 12px;
  color: #ff7875;
}
.res-note {
  margin-top: 4px;
  font-size: 12px;
  color: #d89614;
}
.res-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}
</style>
