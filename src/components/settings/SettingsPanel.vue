<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { getConfigPath, getDefaultIncludeRules, getStorageInfo, type StorageInfo } from '@/api/config'
import { resolveUploadDir } from '@/api/attachments'
import { ensureDir, getNetworkInfo, openDir, type NetworkInfo } from '@/api/system'
import { platform } from '@/platform'
import { pickDirectory } from '@/utils/dialog'
import RulesEditor from '@/components/RulesEditor.vue'

const props = defineProps<{
  defaultUploadDir: string | null
  defaultInclude: string[]
}>()

const emit = defineEmits<{
  updateDefaultUploadDir: [dir: string | null]
  updateDefaultInclude: [rules: string[]]
}>()

const configPath = ref('')
const storageInfo = ref<StorageInfo | null>(null)
const networkInfo = ref<NetworkInfo | null>(null)
const networkError = ref('')
const networkLoading = ref(false)
/** 操作失败时的提示（贴在设置页顶部） */
const message = ref('')
/** 附件实际落盘目录：设置里的地址，未设置时解析成 App 私有 attachments 目录 */
const resolvedUploadDir = ref('')

/** 打包包含规则：跟设置一起保存，改动立即回写配置 */
const localIncludeRules = ref<string[]>([])

watch(
  () => props.defaultInclude,
  (rules) => {
    localIncludeRules.value = [...(rules ?? [])]
  },
  { immediate: true },
)

function onIncludeRulesChange(rules: string[]) {
  localIncludeRules.value = rules
  emit('updateDefaultInclude', rules)
}

async function resetIncludeRules() {
  try {
    const defaults = await getDefaultIncludeRules()
    onIncludeRulesChange([...defaults])
  } catch (err: any) {
    message.value = `恢复默认包含规则失败：${err}`
  }
}

const uploadDirSource = computed(() =>
  props.defaultUploadDir ? '设置里指定的地址' : 'App 私有目录（未指定时的默认位置）',
)

async function loadStorageInfo() {
  try {
    configPath.value = await getConfigPath()
  } catch (err: any) {
    configPath.value = `读取失败：${err}`
  }
  try {
    storageInfo.value = await getStorageInfo()
  } catch {
    storageInfo.value = null
  }
}

/** MAC / IP 会随插拔网线、连 VPN 变化，所以跟着右上角的「重新检测」一起刷新 */
async function loadNetworkInfo() {
  networkLoading.value = true
  try {
    networkInfo.value = await getNetworkInfo()
    networkError.value = ''
  } catch (err: any) {
    networkInfo.value = null
    networkError.value = String(err)
  } finally {
    networkLoading.value = false
  }
}

/** 始终展示真实路径，而不是「未设置」——用户要知道附件到底存在哪 */
async function loadUploadDir() {
  try {
    resolvedUploadDir.value = await resolveUploadDir(props.defaultUploadDir)
  } catch {
    resolvedUploadDir.value = ''
  }
}

onMounted(() => {
  loadStorageInfo()
  loadNetworkInfo()
  loadUploadDir()
})

// 父组件改完默认目录后立刻重算，避免继续显示旧路径
watch(() => props.defaultUploadDir, loadUploadDir)

async function refreshAll() {
  await Promise.all([loadNetworkInfo(), loadStorageInfo(), loadUploadDir()])
}

async function openUploadDir() {
  const path = resolvedUploadDir.value
  if (!path) return
  await run(async () => {
    // 首次上传前目录还不存在，先建出来再打开
    const dir = await ensureDir(path)
    await openDir(dir)
  }, '打开上传目录失败')
}

async function changeDefaultDir() {
  await run(async () => {
    const selected = await pickDirectory()
    if (!selected) return
    emit('updateDefaultUploadDir', selected)
  }, '选择目录失败')
}

function resetDefaultDir() {
  emit('updateDefaultUploadDir', null)
}

/** 统一把操作失败的原因提示到顶部，避免每个按钮各写一套 */
async function run(action: () => Promise<unknown>, failureLabel: string) {
  try {
    await action()
  } catch (err: any) {
    message.value = `${failureLabel}：${err}`
  }
}
</script>

<template>
  <div class="settings">
    <div class="settings-head">
      <div>
        <h3>设置</h3>
        <p>本机运行环境、网络与附件位置。</p>
      </div>
      <button class="btn btn-secondary" :disabled="networkLoading" @click="refreshAll">
        {{ networkLoading ? '检测中…' : '重新检测' }}
      </button>
    </div>

    <p v-if="message" class="toast">{{ message }}</p>

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
        <span class="info-label">数据库</span>
        <span class="info-value mono">{{ storageInfo?.db_path || configPath || '—' }}</span>
      </div>
      <div class="info-row">
        <span class="info-label">设置文件</span>
        <span class="info-value mono">{{ storageInfo?.settings_path || '—' }}</span>
      </div>
      <div v-if="storageInfo && !storageInfo.ok" class="info-row">
        <span class="info-label">存储状态</span>
        <span class="info-value">异常：{{ storageInfo.error }}</span>
      </div>

      <!-- 网络：按网卡列出 MAC 和该网卡的全部 IP，多 IP 逐条显示 -->
      <div class="net-block">
        <div class="net-head">
          <span class="net-title">网络</span>
          <span class="net-sub">网卡的 MAC 与全部 IP</span>
        </div>
        <p v-if="networkLoading && !networkInfo" class="net-empty">正在读取网络信息…</p>
        <p v-else-if="networkError" class="net-empty">读取失败：{{ networkError }}</p>
        <p v-else-if="!networkInfo || networkInfo.interfaces.length === 0" class="net-empty">
          未检测到可用的网络接口
        </p>
        <div v-else class="net-list">
          <div v-for="iface in networkInfo.interfaces" :key="iface.name" class="net-item">
            <span class="net-name mono">{{ iface.name }}</span>
            <span class="net-mac mono">{{ iface.mac ? `MAC ${iface.mac}` : '无 MAC' }}</span>
            <span v-for="address in iface.addresses" :key="address.ip" class="net-address">
              <span class="mono">
                {{ address.ip }}<template v-if="address.prefix !== null">/{{ address.prefix }}</template>
              </span>
              <span class="tag" :class="address.family === 'IPv4' ? 'tag-ok' : 'tag-muted'">
                {{ address.family }}
              </span>
            </span>
            <span v-if="iface.addresses.length === 0" class="net-empty">暂无 IP</span>
          </div>
        </div>
      </div>
    </section>

    <!-- 附件上传 -->
    <section class="card">
      <h4>附件上传</h4>
      <p class="muted">
        所有分组/项目的附件都加密存放到这里（磁盘文件名是附件 id，原始文件名存在数据库里）；下方显示的是实际落盘位置，未单独设置时是应用私有目录。
      </p>
      <div class="upload-dir-row">
        <div class="upload-dir-main">
          <span class="upload-dir-path mono" :title="resolvedUploadDir">
            {{ resolvedUploadDir || '正在解析上传目录…' }}
          </span>
          <span class="upload-dir-source">来源：{{ uploadDirSource }}</span>
        </div>
        <div class="upload-dir-actions">
          <button
            class="btn btn-secondary btn-xs"
            :disabled="!resolvedUploadDir"
            title="在文件管理器中打开该目录"
            @click="openUploadDir"
          >
            打开
          </button>
          <button class="btn btn-secondary btn-xs" @click="changeDefaultDir">更改</button>
          <button
            class="btn btn-secondary btn-xs"
            :disabled="!defaultUploadDir"
            @click="resetDefaultDir"
          >
            恢复默认
          </button>
        </div>
      </div>
    </section>

    <!-- 打包包含规则：白名单，只有命中的文件才进包 -->
    <section class="card">
      <div class="card-head">
        <h4>layui 项目打包规则</h4>
        <button class="btn btn-secondary btn-xs" @click="resetIncludeRules">恢复默认</button>
      </div>
      <p class="rule-tip">
        只有命中下面规则的文件才会进包，没命中的一律不打包；以 <code>.</code> 或 <code>_</code>
        开头的文件/目录始终跳过，路径里含空格的也跳过。默认已包含 layui 工程运行时用到的
        <code>.thm</code> 模板、<code>.tjs</code> 模板脚本、WPS 控件的 <code>.dotm</code>，
        以及 WebOffice 新建正文用的 <code>.doc</code> / <code>.docx</code> 空白模板。
      </p>
      <RulesEditor
        v-model="localIncludeRules"
        placeholder="输入规则后回车添加，如 *.html、js/**"
        @update:modelValue="onIncludeRulesChange"
      />
      <p class="rule-tip rule-tip-dim">
        不带 <code>/</code> 的规则按文件名匹配（任意层级都算，如 <code>*.png</code>）；带
        <code>/</code> 的按相对源目录的路径匹配（如 <code>assets/**</code>、<code>js/app.js</code>）。
        规则为空时视为未配置：除 <code>.</code>/<code>_</code> 开头的条目外全部打包。
      </p>
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
.card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 8px;
}
.rule-tip {
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-muted);
  margin-bottom: 8px;
}
.rule-tip-dim {
  margin-top: 8px;
  margin-bottom: 0;
}
.rule-tip code {
  padding: 1px 4px;
  border-radius: 4px;
  background: var(--bg-tag);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  color: var(--text-tag);
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
.info-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px 16px;
}
.info-row {
  display: flex;
  gap: 8px;
  font-size: 12px;
  padding: 4px 0;
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
.tag {
  font-size: 12px;
  border-radius: 10px;
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
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}
.muted {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
  margin-bottom: 10px;
}
/* 网络：MAC / 全部 IP */
.net-block {
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px dashed var(--border-color);
}
.net-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 6px;
}
.net-title {
  font-size: 12px;
  color: var(--text-muted);
  width: 104px;
  flex-shrink: 0;
}
.net-sub {
  font-size: 12px;
  color: var(--text-muted);
}
.net-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.net-item {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px 12px;
  padding: 6px 8px;
  border-radius: 6px;
  background: var(--bg-secondary);
  font-size: 12px;
}
.net-name {
  min-width: 48px;
  color: var(--text-primary);
  font-weight: 600;
}
.net-mac {
  color: var(--text-secondary);
}
.net-address {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--text-secondary);
}
.net-empty {
  font-size: 12px;
  color: var(--text-muted);
}
.upload-dir-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.upload-dir-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.upload-dir-path {
  font-size: 12px;
  color: var(--text-secondary);
  word-break: break-all;
}
.upload-dir-source {
  font-size: 12px;
  color: var(--text-muted);
}
.upload-dir-actions {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}
</style>
