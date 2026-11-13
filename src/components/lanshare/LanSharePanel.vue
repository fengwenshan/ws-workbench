<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { message } from 'ant-design-vue'
import { type UnlistenFn } from '@tauri-apps/api/event'
import {
  clearLanShareReceived,
  getLanShareSnapshot,
  onLanShareSendProgress,
  onLanShareSnapshot,
  respondLanShareOffer,
  sendLanShareFiles,
  setLanShareAlias,
  setLanShareDownloadDir,
  setLanShareEnabled,
  type LanPeer,
  type LanShareSnapshot,
  type PendingOffer,
  type ReceivedFile,
  type SendProgress,
} from '@/api/lanshare'
import { openDir, openFile } from '@/api/system'
import { pickDirectory, pickFiles } from '@/utils/dialog'
import { formatBytes } from '@/components/cleaner/format'
import { formatDateTime } from '@/utils/time'

/**
 * 局域网分享工具页。
 *
 * 面板是 v-show 常驻挂载的：事件监听只在 onMounted 注册一次、onUnmounted 清理，
 * 保证在页面隐藏时也能收到「待接收 / 收到文件 / peer 变化」，切回来直接是最新状态；
 * props.active 由 false 变 true 时再补一次快照，防止事件漏收。
 */
const props = defineProps<{
  /** 当前工具页是否激活（App.vue 的 view === 'lanshare'） */
  active: boolean
}>()

const snapshot = ref<LanShareSnapshot | null>(null)
const loading = ref(false)
const loadError = ref('')

/** 顶栏总开关正在请求后端 */
const toggling = ref(false)
/** 设备名输入框里的草稿：只有点「保存」才写后端 */
const aliasDraft = ref('')
const aliasFocused = ref(false)
const savingAlias = ref(false)
/** 正在保存接收目录 */
const savingDir = ref(false)
/** 正在接受 / 拒绝的 offer id */
const offerBusy = ref<string | null>(null)
/** 正在选文件 / 发起发送的 peer fingerprint */
const sendingPeer = ref<string | null>(null)
const clearingReceived = ref(false)

/** 每个文件一行发送进度，key = transfer_id + file_index */
interface TransferRow extends SendProgress {
  key: string
}
const transfers = ref<TransferRow[]>([])
/** 1 秒一跳，给「待接收倒计时 / 设备最近在线」提供刷新 */
const now = ref(Date.now())

let unlisteners: UnlistenFn[] = []
let tickTimer: ReturnType<typeof setTimeout> | null = null
let disposed = false

const enabled = computed(() => snapshot.value?.enabled ?? false)
const running = computed(() => snapshot.value?.running ?? false)
const peers = computed(() =>
  [...(snapshot.value?.peers ?? [])].sort((a, b) => b.last_seen_ms - a.last_seen_ms),
)
const pendingOffers = computed(() => snapshot.value?.pending_offers ?? [])
const recentReceived = computed(() => snapshot.value?.recent_received ?? [])
const aliasDirty = computed(
  () => aliasDraft.value.trim() !== (snapshot.value?.alias ?? ''),
)
const statusText = computed(() => {
  if (!snapshot.value) return '读取中…'
  if (snapshot.value.last_error) return '启动失败'
  if (running.value) return '运行中'
  if (enabled.value) return '启动中…'
  return '未开启'
})
const statusClass = computed(() => {
  if (!snapshot.value || snapshot.value.last_error) return 'is-error'
  if (running.value) return 'is-on'
  if (enabled.value) return 'is-busy'
  return 'is-off'
})
const shortFingerprint = computed(() => {
  const fingerprint = snapshot.value?.fingerprint ?? ''
  return fingerprint.length > 12 ? `${fingerprint.slice(0, 8)}…${fingerprint.slice(-4)}` : fingerprint
})
const localAddresses = computed(() => (snapshot.value?.local_addresses ?? []).join(' · '))

function errorText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

function applySnapshot(next: LanShareSnapshot) {
  snapshot.value = next
  if (!aliasFocused.value) aliasDraft.value = next.alias ?? ''
}

async function refresh() {
  loading.value = true
  try {
    applySnapshot(await getLanShareSnapshot())
    loadError.value = ''
  } catch (err) {
    loadError.value = `读取局域网分享状态失败：${errorText(err)}`
  } finally {
    loading.value = false
  }
}

async function toggleEnabled() {
  if (!snapshot.value || toggling.value) return
  toggling.value = true
  const next = !enabled.value
  try {
    applySnapshot(await setLanShareEnabled(next))
    message.success(next ? '局域网分享已开启' : '局域网分享已关闭')
  } catch (err) {
    message.error(`切换失败：${errorText(err)}`)
    await refresh()
  } finally {
    toggling.value = false
  }
}

async function saveAlias() {
  if (!snapshot.value || savingAlias.value) return
  const alias = aliasDraft.value.trim()
  if (!alias || alias === snapshot.value.alias) {
    aliasDraft.value = snapshot.value.alias
    return
  }
  savingAlias.value = true
  try {
    applySnapshot(await setLanShareAlias(alias))
    message.success('设备名已保存')
  } catch (err) {
    message.error(`保存设备名失败：${errorText(err)}`)
    aliasDraft.value = snapshot.value.alias
  } finally {
    savingAlias.value = false
  }
}

async function chooseDownloadDir() {
  if (!snapshot.value || savingDir.value) return
  let dir: string | null = null
  try {
    dir = await pickDirectory({
      defaultPath: snapshot.value.download_dir || undefined,
      title: '选择接收文件的保存目录',
    })
  } catch (err) {
    message.error(`打开目录选择框失败：${errorText(err)}`)
    return
  }
  if (!dir) return
  savingDir.value = true
  try {
    applySnapshot(await setLanShareDownloadDir(dir))
    message.success('接收目录已更新')
  } catch (err) {
    message.error(`设置接收目录失败：${errorText(err)}`)
  } finally {
    savingDir.value = false
  }
}

async function resetDownloadDir() {
  if (!snapshot.value || savingDir.value) return
  savingDir.value = true
  try {
    applySnapshot(await setLanShareDownloadDir(null))
    message.success('已恢复默认接收目录')
  } catch (err) {
    message.error(`恢复默认目录失败：${errorText(err)}`)
  } finally {
    savingDir.value = false
  }
}

async function openDownloadDir() {
  if (!snapshot.value?.download_dir) return
  try {
    await openDir(snapshot.value.download_dir)
  } catch (err) {
    message.error(`打开目录失败：${errorText(err)}`)
  }
}

async function sendToPeer(peer: LanPeer) {
  if (sendingPeer.value) return
  sendingPeer.value = peer.fingerprint
  try {
    const paths = await pickFiles({ title: `发送文件给 ${peer.alias}` })
    if (!paths.length) return
    await sendLanShareFiles(peer.fingerprint, paths)
    message.success(`已开始向「${peer.alias}」发送 ${paths.length} 个文件`)
  } catch (err) {
    message.error(`发送失败：${errorText(err)}`)
  } finally {
    sendingPeer.value = null
  }
}

async function respondOffer(offer: PendingOffer, accept: boolean) {
  if (offerBusy.value) return
  offerBusy.value = offer.id
  try {
    await respondLanShareOffer(offer.id, accept)
    message.success(accept ? `已接受「${offer.sender_alias || '对方设备'}」的分享` : '已拒绝这次分享')
    await refresh()
  } catch (err) {
    message.error(`${accept ? '接受' : '拒绝'}失败：${errorText(err)}`)
    await refresh()
  } finally {
    offerBusy.value = null
  }
}

function handleProgress(progress: SendProgress) {
  const key = `${progress.transfer_id}:${progress.file_index}`
  const row: TransferRow = { ...progress, key }
  const rest = transfers.value.filter((item) => item.key !== key)
  transfers.value = [row, ...rest].slice(0, 80)
}

async function clearReceivedList() {
  if (clearingReceived.value) return
  clearingReceived.value = true
  try {
    await clearLanShareReceived()
    await refresh()
    message.success('最近接收记录已清空（文件仍保留在磁盘上）')
  } catch (err) {
    message.error(`清空记录失败：${errorText(err)}`)
  } finally {
    clearingReceived.value = false
  }
}

async function openReceivedFile(item: ReceivedFile) {
  try {
    await openFile(item.path)
  } catch (err) {
    message.error(`打开文件失败：${errorText(err)}`)
  }
}

/** 打开「所在目录」：优先用文件自己所在的目录，取不到时退回接收目录 */
async function openReceivedDir(item: ReceivedFile) {
  const dir = parentDir(item.path) || snapshot.value?.download_dir || ''
  if (!dir) return
  try {
    await openDir(dir)
  } catch (err) {
    message.error(`打开目录失败：${errorText(err)}`)
  }
}

function parentDir(path: string): string {
  const index = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return index > 0 ? path.slice(0, index) : ''
}

/** 待接收倒计时：剩余秒数，过点显示「即将超时」 */
function remainingText(offer: PendingOffer): string {
  if (!offer.expires_in_ms || !offer.created_at_ms) return ''
  const remainMs = offer.created_at_ms + offer.expires_in_ms - now.value
  if (remainMs <= 0) return '即将超时'
  const seconds = Math.ceil(remainMs / 1000)
  return `${seconds} 秒后自动拒绝`
}

/** 设备最近在线：按 last_seen_ms 显示相对时间 */
function lastSeenText(peer: LanPeer): string {
  const delta = now.value - peer.last_seen_ms
  if (!Number.isFinite(delta) || delta < 0) return '在线'
  if (delta < 2000) return '在线'
  if (delta < 12000) return `${Math.round(delta / 1000)} 秒前在线`
  return '即将离线'
}

function sendPercent(row: SendProgress): number {
  if (row.total_bytes <= 0) return row.done && !row.error ? 100 : 0
  return Math.min(100, Math.max(0, Math.floor((row.sent_bytes / row.total_bytes) * 100)))
}

/** 清掉页面上的发送进度记录（不影响已经完成的文件） */
function clearTransfers() {
  transfers.value = []
}

function peerLabel(row: SendProgress): string {
  return row.peer_alias || '对方设备'
}

onMounted(async () => {
  await refresh()

  unlisteners.push(
    await onLanShareSnapshot((next) => {
      applySnapshot(next)
      loadError.value = ''
    }),
  )
  unlisteners.push(await onLanShareSendProgress(handleProgress))

  if (disposed) {
    unlisteners.forEach((unlisten) => unlisten())
    unlisteners = []
    return
  }
  tickTimer = setInterval(() => {
    now.value = Date.now()
  }, 1000)
})

onUnmounted(() => {
  disposed = true
  unlisteners.forEach((unlisten) => unlisten())
  unlisteners = []
  if (tickTimer) {
    clearInterval(tickTimer)
    tickTimer = null
  }
})

watch(
  () => props.active,
  (active) => {
    if (active) void refresh()
  },
)
</script>

<template>
  <div class="lanshare">
    <div class="lan-head">
      <div class="lan-title">
        <h3>局域网分享</h3>
        <p>同一局域网里都打开本应用的设备会互相发现，选中设备就能把本机文件发过去。</p>
      </div>
      <div class="lan-head-actions">
        <span class="lan-status" :class="statusClass">
          <span class="lan-status-dot"></span>
          {{ statusText }}
          <template v-if="running && snapshot?.port">（端口 {{ snapshot.port }}）</template>
        </span>
        <button
          class="btn"
          :class="enabled ? 'btn-danger-ghost' : 'btn-primary'"
          :disabled="!snapshot || toggling"
          @click="toggleEnabled"
        >
          {{ toggling ? '处理中…' : enabled ? '关闭功能' : '开启功能' }}
        </button>
      </div>
    </div>

    <div class="lan-body">
      <p v-if="loadError" class="lan-alert">{{ loadError }}</p>
      <p v-if="!snapshot && loading" class="lan-empty">正在读取局域网分享状态…</p>

      <template v-if="snapshot">
        <p v-if="snapshot.last_error" class="lan-alert">
          启动失败：{{ snapshot.last_error }}
          <span class="lan-alert-hint">请检查端口 / 防火墙设置后重新开启。</span>
        </p>

        <!-- 运行状态 -->
        <section class="card status-card">
          <div class="status-grid">
            <div class="status-item">
              <span class="label">本机设备</span>
              <span class="value">{{ snapshot.alias || '—' }}</span>
            </div>
            <div class="status-item">
              <span class="label">监听端口</span>
              <span class="value mono">{{ snapshot.port ?? '—' }}</span>
            </div>
            <div class="status-item">
              <span class="label">本机地址</span>
              <span class="value mono">{{ localAddresses || '未获取到可用网卡地址' }}</span>
            </div>
            <div class="status-item">
              <span class="label">设备指纹</span>
              <span class="value mono" :title="snapshot.fingerprint">{{ shortFingerprint || '—' }}</span>
            </div>
          </div>
          <p v-if="!enabled" class="muted">
            功能已关闭：本机不会广播，也不会接受别人发来的文件。开启后会自动搜索局域网内的设备。
          </p>
        </section>

        <!-- 本机设置 -->
        <section class="card">
          <h4>本机设置</h4>
          <div class="setting-row">
            <span class="label">设备名</span>
            <input
              v-model="aliasDraft"
              class="text-input"
              :disabled="savingAlias"
              maxlength="64"
              placeholder="默认使用主机名"
              @focus="aliasFocused = true"
              @blur="aliasFocused = false"
              @keydown.enter.prevent="saveAlias"
            />
            <button
              class="btn btn-secondary btn-xs"
              :disabled="savingAlias || !aliasDirty"
              @click="saveAlias"
            >
              {{ savingAlias ? '保存中…' : '保存' }}
            </button>
          </div>
          <div class="setting-row">
            <span class="label">接收目录</span>
            <span class="path-text mono" :title="snapshot.download_dir || ''">
              {{ snapshot.download_dir || '—' }}
            </span>
            <button class="btn btn-secondary btn-xs" :disabled="savingDir" @click="chooseDownloadDir">
              选择目录
            </button>
            <button class="btn btn-secondary btn-xs" @click="openDownloadDir">打开目录</button>
            <button
              v-if="snapshot.download_dir_custom"
              class="btn btn-secondary btn-xs"
              :disabled="savingDir"
              @click="resetDownloadDir"
            >
              恢复默认
            </button>
          </div>
          <p class="muted">
            别人分享过来的文件会保存到这里（重名自动加序号，不会覆盖已有文件）。
            {{ snapshot.download_dir_custom ? '当前是自定义目录。' : '当前是默认目录。' }}
          </p>
        </section>

        <!-- 待接收：别人发来的分享请求 -->
        <section v-if="pendingOffers.length" class="card">
          <div class="card-head">
            <h4>待接收</h4>
            <span class="count">{{ pendingOffers.length }}</span>
          </div>
          <div v-for="offer in pendingOffers" :key="offer.id" class="offer">
            <div class="offer-head">
              <span class="offer-from">
                {{ offer.sender_alias || '未知设备' }}
                <span class="mono offer-ip">{{ offer.sender_ip }}</span>
              </span>
              <span class="offer-expire">{{ remainingText(offer) }}</span>
            </div>
            <ul class="file-list">
              <li v-for="file in offer.files" :key="file.id" class="file-item">
                <span class="file-name" :title="file.file_name">{{ file.file_name }}</span>
                <span class="file-size">{{ formatBytes(file.size) }}</span>
              </li>
            </ul>
            <div class="offer-actions">
              <span class="total">
                共 {{ offer.files.length }} 个文件 · {{ formatBytes(offer.total_size) }}
              </span>
              <button
                class="btn btn-secondary btn-xs"
                :disabled="offerBusy === offer.id"
                @click="respondOffer(offer, false)"
              >
                拒绝
              </button>
              <button
                class="btn btn-primary btn-xs"
                :disabled="offerBusy === offer.id"
                @click="respondOffer(offer, true)"
              >
                {{ offerBusy === offer.id ? '处理中…' : '接受' }}
              </button>
            </div>
          </div>
        </section>

        <!-- 附近设备 -->
        <section class="card">
          <div class="card-head">
            <h4>附近设备</h4>
            <span class="muted">{{ peers.length }} 台在线</span>
          </div>
          <p v-if="!enabled" class="lan-empty">功能未开启，开启后才会搜索局域网内的设备。</p>
          <p v-else-if="!peers.length" class="lan-empty">
            同一局域网内打开本应用的设备会出现在这里。
          </p>
          <ul v-else class="peer-list">
            <li v-for="peer in peers" :key="peer.fingerprint" class="peer">
              <div class="peer-main">
                <span class="peer-name">{{ peer.alias || '未知设备' }}</span>
                <span class="peer-meta mono">{{ peer.ip }}:{{ peer.port }}</span>
                <span class="peer-seen">{{ lastSeenText(peer) }}</span>
              </div>
              <button
                class="btn btn-secondary btn-xs"
                :disabled="sendingPeer === peer.fingerprint"
                @click="sendToPeer(peer)"
              >
                {{ sendingPeer === peer.fingerprint ? '选择文件…' : '发送文件' }}
              </button>
            </li>
          </ul>
        </section>

        <!-- 发送进度 -->
        <section v-if="transfers.length" class="card">
          <div class="card-head">
            <h4>发送进度</h4>
            <button class="btn btn-secondary btn-xs" @click="clearTransfers">清除记录</button>
          </div>
          <div
            v-for="row in transfers"
            :key="row.key"
            class="transfer"
            :class="{ 'is-failed': !!row.error, 'is-done': row.done && !row.error }"
          >
            <div class="transfer-head">
              <span class="file-name" :title="row.file_name">{{ row.file_name }}</span>
              <span class="transfer-meta">
                {{ row.file_index + 1 }}/{{ row.file_total }} · {{ peerLabel(row) }}
              </span>
            </div>
            <div class="progress">
              <div class="progress-fill" :style="{ width: sendPercent(row) + '%' }"></div>
            </div>
            <div class="transfer-foot">
              <span class="transfer-state">
                <template v-if="row.error">{{ row.error }}</template>
                <template v-else-if="row.done">已完成</template>
                <template v-else>
                  {{ formatBytes(row.sent_bytes) }} / {{ formatBytes(row.total_bytes) }}
                </template>
              </span>
              <span class="transfer-percent">{{ sendPercent(row) }}%</span>
            </div>
          </div>
        </section>

        <!-- 最近接收 -->
        <section class="card">
          <div class="card-head">
            <h4>最近接收</h4>
            <button
              v-if="recentReceived.length"
              class="btn btn-secondary btn-xs"
              :disabled="clearingReceived"
              @click="clearReceivedList"
            >
              清空记录
            </button>
          </div>
          <p v-if="!recentReceived.length" class="lan-empty">
            还没有收到文件。别人分享时接受后会出现在这里。
          </p>
          <ul v-else class="received-list">
            <li v-for="(item, index) in recentReceived" :key="item.path + index" class="received">
              <div class="received-main">
                <span class="file-name" :title="item.path">{{ item.file_name }}</span>
                <span class="received-meta">
                  {{ item.from_alias || '未知设备' }} · {{ formatBytes(item.size) }} ·
                  {{ formatDateTime(item.received_at_ms) }}
                </span>
              </div>
              <div class="received-actions">
                <button class="btn btn-secondary btn-xs" @click="openReceivedFile(item)">打开文件</button>
                <button class="btn btn-secondary btn-xs" @click="openReceivedDir(item)">打开所在目录</button>
              </div>
            </li>
          </ul>
        </section>
      </template>
    </div>
  </div>
</template>

<style scoped>
.lanshare {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* 页头：标题 + 开关，和「系统」「设置」页同一套排版 */
.lan-head {
  flex-shrink: 0;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 20px 12px;
  border-bottom: 1px solid var(--border-color);
}
.lan-title h3 {
  font-size: 16px;
  color: var(--text-primary);
  margin-bottom: 4px;
}
.lan-title p {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}
.lan-head-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-shrink: 0;
}
.lan-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
}
.lan-status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-muted);
  flex-shrink: 0;
}
.lan-status.is-on .lan-status-dot {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.16);
}
.lan-status.is-on {
  color: var(--success);
}
.lan-status.is-busy .lan-status-dot {
  background: var(--warning);
}
.lan-status.is-busy {
  color: var(--warning);
}
.lan-status.is-error .lan-status-dot {
  background: var(--danger);
}
.lan-status.is-error {
  color: var(--danger);
}

.lan-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 14px 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.lan-empty {
  font-size: 13px;
  color: var(--text-muted);
  line-height: 1.6;
  padding: 10px 0;
}
.lan-alert {
  margin: 0;
  padding: 8px 12px;
  border-radius: 6px;
  border: 1px solid rgba(239, 68, 68, 0.3);
  background: rgba(239, 68, 68, 0.06);
  color: var(--danger);
  font-size: 12px;
  line-height: 1.6;
}
.lan-alert-hint {
  color: var(--text-muted);
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
  gap: 10px;
  margin-bottom: 10px;
}
.card-head h4 {
  margin-bottom: 0;
}
.count {
  font-size: 12px;
  color: var(--primary);
  background: var(--bg-active);
  border-radius: 10px;
  padding: 1px 8px;
}

.status-card {
  padding-bottom: 12px;
}
.status-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px 20px;
}
.status-item {
  display: flex;
  gap: 8px;
  font-size: 12px;
  padding: 3px 0;
  min-width: 0;
}
.label {
  color: var(--text-muted);
  width: 64px;
  flex-shrink: 0;
}
.value {
  color: var(--text-secondary);
  word-break: break-all;
}

.muted {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
  margin: 8px 0 0;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}

.setting-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.text-input {
  flex: 1;
  min-width: 0;
  height: 30px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-primary);
  font-size: 13px;
  outline: none;
}
.text-input:focus {
  border-color: var(--primary);
}
.text-input:disabled {
  background: var(--bg-readonly);
  color: var(--text-muted);
}
.path-text {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.btn.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
  flex-shrink: 0;
}

/* 待接收 */
.offer {
  border-top: 1px dashed var(--border-color);
  padding-top: 10px;
  margin-top: 10px;
}
.offer:first-of-type {
  border-top: none;
  padding-top: 0;
  margin-top: 0;
}
.offer-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 8px;
}
.offer-from {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}
.offer-ip {
  margin-left: 6px;
  font-size: 12px;
  font-weight: 400;
  color: var(--text-muted);
}
.offer-expire {
  font-size: 12px;
  color: var(--warning);
  flex-shrink: 0;
}
.file-list {
  list-style: none;
  max-height: 140px;
  overflow-y: auto;
  margin-bottom: 8px;
}
.file-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 4px 8px;
  border-radius: 4px;
  background: var(--bg-secondary);
  font-size: 12px;
  margin-bottom: 4px;
}
.file-name {
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.file-size {
  color: var(--text-muted);
  flex-shrink: 0;
}
.offer-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
}
.offer-actions .total {
  margin-right: auto;
  font-size: 12px;
  color: var(--text-secondary);
}

/* 附近设备 */
.peer-list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.peer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 10px;
  border-radius: 6px;
  background: var(--bg-secondary);
}
.peer-main {
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
  flex-wrap: wrap;
}
.peer-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}
.peer-meta {
  font-size: 12px;
  color: var(--text-secondary);
}
.peer-seen {
  font-size: 12px;
  color: var(--success);
}

/* 发送进度 */
.transfer {
  padding: 8px 0;
  border-top: 1px dashed var(--border-color);
}
.transfer:first-of-type {
  border-top: none;
  padding-top: 0;
}
.transfer-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 5px;
}
.transfer-meta {
  font-size: 12px;
  color: var(--text-muted);
  flex-shrink: 0;
}
.progress {
  height: 6px;
  border-radius: 3px;
  background: var(--bg-readonly);
  overflow: hidden;
}
.progress-fill {
  height: 100%;
  border-radius: 3px;
  background: var(--primary);
  transition: width 0.25s ease;
}
.transfer.is-failed .progress-fill {
  background: var(--danger);
}
.transfer.is-done .progress-fill {
  background: var(--success);
}
.transfer-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-secondary);
}
.transfer.is-failed .transfer-state {
  color: var(--danger);
}
.transfer.is-done .transfer-state {
  color: var(--success);
}
.transfer-percent {
  color: var(--text-muted);
}

/* 最近接收 */
.received-list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.received {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 10px;
  border-radius: 6px;
  background: var(--bg-secondary);
}
.received-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.received-meta {
  font-size: 12px;
  color: var(--text-muted);
}
.received-actions {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}
</style>
