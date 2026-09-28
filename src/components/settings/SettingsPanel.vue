<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { platform } from '@/platform'
import { SOURCE_LABELS, sourceTone, type AppId, type AppLocation } from '@/types/apps'
import type { AppsApi } from '@/composables/useApps'

const props = defineProps<{
  apps: AppsApi
}>()

const apps = props.apps
const configPath = ref('')
const expanded = ref<string>('')

const canConfigurePaths = computed(() => platform.is_windows)

onMounted(async () => {
  try {
    configPath.value = await invoke<string>('get_config_path')
  } catch (err: any) {
    configPath.value = `读取失败：${err}`
  }
})

function toggleCandidates(id: string) {
  expanded.value = expanded.value === id ? '' : id
}

function otherCandidates(item: AppLocation) {
  return item.candidates.filter((candidate) => candidate.path !== item.path)
}

/** 统一把操作失败的原因提示到顶部，避免每个按钮各写一套 */
async function run(action: () => Promise<unknown>, failureLabel: string) {
  try {
    await action()
  } catch (err: any) {
    apps.lastMessage = `${failureLabel}：${err}`
  }
}

const pick = (id: AppId) => run(() => apps.pickExecutable(id), '设置失败')
const clear = (id: AppId) => run(() => apps.clearPath(id), '清除失败')
const importDir = (id: AppId) => run(() => apps.importDirectory(id), '导入失败')
const removeManaged = (id: AppId) => run(() => apps.removeManaged(id), '移除失败')
const reveal = (path: string) => run(() => apps.revealPath(path), '打开目录失败')
</script>

<template>
  <div class="settings">
    <div class="settings-head">
      <div>
        <h3>设置</h3>
        <p>运行环境与应用启动文件的位置。</p>
      </div>
      <button class="btn btn-secondary" :disabled="apps.loading" @click="apps.refresh()">
        {{ apps.loading ? '检测中…' : '重新检测' }}
      </button>
    </div>

    <p v-if="apps.lastMessage" class="toast">{{ apps.lastMessage }}</p>

    <!-- 运行环境 -->
    <section class="card">
      <h4>运行环境</h4>
      <div class="info-grid">
        <div class="info-row">
          <span class="info-label">操作系统</span>
          <span class="info-value">{{ platform.label }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">架构</span>
          <span class="info-value mono">{{ platform.arch || '—' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">可执行文件后缀</span>
          <span class="info-value mono">{{ platform.exe_suffix || '（无）' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">VPN 需要提权</span>
          <span class="info-value">{{ platform.vpn_needs_admin ? '是' : '否' }}</span>
        </div>
      </div>
      <div class="info-row">
        <span class="info-label">配置文件</span>
        <span class="info-value mono">{{ configPath || '—' }}</span>
      </div>
    </section>

    <!-- 应用路径 -->
    <section class="card">
      <div class="card-head">
        <h4>应用启动文件</h4>
        <span v-if="canConfigurePaths" class="card-sub">
          共 {{ apps.locations.length }} 项，{{ apps.missing.length }} 项未找到
        </span>
      </div>

      <div v-if="!canConfigurePaths" class="platform-note">
        <strong>该设置仅在 Windows 提供</strong>
        <p>
          Windows 上的应用未必装在默认目录、也未必进 PATH，所以需要在这里手动指定。
          {{ platform.label }} 下本程序会直接按系统约定查找：macOS 看
          <code>/Applications</code> 与 Homebrew 前缀，Linux 看 PATH 与
          <code>/usr/bin</code>、<code>/usr/local/bin</code>。
          下面仍然列出各项的定位结果，出问题时方便排查。
        </p>
      </div>

      <div class="app-list">
        <div v-for="item in apps.locations" :key="item.id" class="app-item">
          <div class="app-row">
            <span class="dot" :class="item.available ? 'dot-ok' : 'dot-idle'"></span>
            <div class="app-main">
              <div class="app-title">
                <strong>{{ item.name }}</strong>
                <span class="tag" :class="'tag-' + sourceTone(item.source)">
                  {{ SOURCE_LABELS[item.source] ?? item.source }}
                </span>
                <span v-if="item.override_path && item.source !== 'custom'" class="tag tag-warn">
                  指定路径未生效
                </span>
              </div>
              <p class="app-desc">{{ item.description }}</p>

              <p class="app-path mono">{{ item.path || '未找到可执行文件' }}</p>
              <p v-if="item.expected_names.length > 0" class="app-expected">
                该选的文件：<span class="mono">{{ item.expected_names.join(' / ') }}</span>
              </p>
              <p v-if="item.note" class="app-note">{{ item.note }}</p>

              <!-- 内置副本：位置编译期就定了，不需要找 -->
              <div v-if="item.builtin_path" class="extra-row">
                <span class="tag tag-ok">已内置</span>
                <span class="mono extra-path">{{ item.builtin_path }}</span>
              </div>
              <p v-if="item.builtin_path" class="app-expected">
                随程序一起分发，不用配置也没有可找的余地；连接时默认用它。
              </p>
            </div>

            <div class="app-actions">
              <button
                v-if="item.available"
                class="btn btn-secondary btn-xs"
                @click="reveal(item.path)"
              >
                打开所在目录
              </button>
              <template v-if="canConfigurePaths">
                <button class="btn btn-secondary btn-xs" @click="pick(item.id as AppId)">
                  {{ item.override_path ? '重新指定' : '手动指定' }}
                </button>
                <button
                  v-if="item.override_path"
                  class="btn btn-secondary btn-xs"
                  @click="clear(item.id as AppId)"
                >
                  清除指定
                </button>
                <button
                  v-if="item.supports_import"
                  class="btn btn-secondary btn-xs"
                  @click="importDir(item.id as AppId)"
                >
                  导入目录
                </button>
                <button
                  v-if="item.source === 'managed'"
                  class="btn btn-danger btn-xs"
                  @click="removeManaged(item.id as AppId)"
                >
                  移除托管
                </button>
              </template>
            </div>
          </div>

          <div v-if="otherCandidates(item).length > 0" class="candidates">
            <button class="candidates-toggle" @click="toggleCandidates(item.id)">
              {{ expanded === item.id ? '收起' : `其他候选（${otherCandidates(item).length}）` }}
            </button>
            <div v-if="expanded === item.id" class="candidate-list">
              <div v-for="candidate in otherCandidates(item)" :key="candidate.path" class="candidate-row">
                <span class="tag tag-muted">{{ SOURCE_LABELS[candidate.source] ?? candidate.source }}</span>
                <span class="mono candidate-path">{{ candidate.path }}</span>
                <span v-if="!candidate.exists" class="candidate-missing">不存在</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<style scoped>
.settings {
  flex: 1;
  overflow-y: auto;
  padding: 16px 20px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.settings-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}
.settings-head h3 {
  font-size: 16px;
  color: var(--text-primary);
  margin-bottom: 4px;
}
.settings-head p {
  font-size: 12px;
  color: var(--text-muted);
}
.toast {
  font-size: 12px;
  color: var(--primary);
  background: rgba(59, 130, 246, 0.08);
  border: 1px solid rgba(59, 130, 246, 0.25);
  border-radius: 6px;
  padding: 8px 12px;
}
.card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  padding: 14px;
}
.card h4 {
  font-size: 14px;
  color: var(--text-primary);
  margin-bottom: 10px;
}
.card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
}
.card-head h4 {
  margin-bottom: 0;
}
.card-sub {
  font-size: 12px;
  color: var(--text-muted);
}
.info-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px 16px;
}
.info-row {
  display: flex;
  gap: 8px;
  font-size: 12px;
  padding: 3px 0;
}
.info-label {
  color: var(--text-muted);
  width: 104px;
  flex-shrink: 0;
}
.info-value {
  color: var(--text-secondary);
  word-break: break-all;
}
.platform-note {
  border: 1px solid rgba(59, 130, 246, 0.3);
  background: rgba(59, 130, 246, 0.06);
  border-radius: 6px;
  padding: 10px 12px;
  margin-bottom: 12px;
}
.platform-note strong {
  font-size: 13px;
  color: var(--text-primary);
}
.platform-note p {
  margin-top: 4px;
  font-size: 12px;
  line-height: 1.65;
  color: var(--text-secondary);
}
.platform-note code {
  background: var(--bg-tag);
  border-radius: 3px;
  padding: 0 4px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.app-list {
  display: flex;
  flex-direction: column;
}
.app-item {
  border-top: 1px solid var(--border-color);
  padding: 10px 0;
}
.app-item:first-child {
  border-top: none;
}
.app-row {
  display: flex;
  gap: 10px;
  align-items: flex-start;
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  margin-top: 6px;
  flex-shrink: 0;
}
.dot-ok {
  background: var(--success);
}
.dot-idle {
  background: var(--text-muted);
}
.app-main {
  flex: 1;
  min-width: 0;
}
.app-title {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.app-title strong {
  font-size: 13px;
  color: var(--text-primary);
}
.app-desc {
  font-size: 12px;
  color: var(--text-muted);
  margin-top: 2px;
}
.app-path {
  font-size: 12px;
  color: var(--text-secondary);
  margin-top: 3px;
  word-break: break-all;
}
.app-note {
  font-size: 12px;
  color: var(--warning);
  margin-top: 3px;
  line-height: 1.55;
}
.app-expected {
  font-size: 12px;
  color: var(--text-muted);
  margin-top: 3px;
}
.extra-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 6px;
}
.extra-path {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--text-secondary);
  word-break: break-all;
}
.app-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  justify-content: flex-end;
  max-width: 280px;
  flex-shrink: 0;
}
.tag {
  font-size: 11px;
  border-radius: 9px;
  padding: 1px 8px;
  white-space: nowrap;
}
.tag-ok {
  color: var(--success);
  background: rgba(16, 185, 129, 0.1);
}
.tag-muted {
  color: var(--text-secondary);
  background: var(--bg-tag);
}
.tag-warn {
  color: var(--warning);
  background: rgba(245, 158, 11, 0.12);
}
.candidates {
  margin-left: 18px;
  margin-top: 6px;
}
.candidates-toggle {
  border: none;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}
.candidate-list {
  margin-top: 6px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  overflow: hidden;
}
.candidate-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  font-size: 12px;
  border-top: 1px solid var(--border-color);
}
.candidate-row:first-child {
  border-top: none;
}
.candidate-path {
  flex: 1;
  color: var(--text-secondary);
  word-break: break-all;
}
.candidate-missing {
  color: var(--text-muted);
  flex-shrink: 0;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}
</style>
