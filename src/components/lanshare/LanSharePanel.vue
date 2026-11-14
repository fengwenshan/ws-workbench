<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { message } from 'ant-design-vue'
import { type UnlistenFn } from '@tauri-apps/api/event'
import {
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
  type SendProgress,
} from '@/api/lanshare'
import { openDir } from '@/api/system'
import { pickDirectory, pickFiles } from '@/utils/dialog'
import { formatBytes } from '@/components/cleaner/format'

/**
 * 局域网分享工具页。
 *
 * 面板是 v-show 常驻挂载的：事件监听只在 onMounted 注册一次、onUnmounted 清理，
 * 保证在页面隐藏时也能收到「待接收 / peer 变化」，切回来直接是最新状态；
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

/** 设备名就地编辑：aliasDraft 是编辑态草稿，只有 ✓ / Enter 才写后端（blur 不保存） */
const aliasEditing = ref(false)
const aliasSaving = ref(false)
const aliasDraft = ref('')
const aliasInput = ref<HTMLInputElement | null>(null)

/** 正在保存接收目录 */
const savingDir = ref(false)
/** 正在接受 / 拒绝的 offer id */
const offerBusy = ref<string | null>(null)
/** 正在选文件 / 发起发送的 peer fingerprint */
const sendingPeer = ref<string | null>(null)

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
  // 正在编辑设备名时不要被后台快照覆盖草稿
  if (!aliasEditing.value) aliasDraft.value = next.alias ?? ''
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

/** 进入设备名编辑态：聚焦并全选当前名字 */
function startAliasEdit() {
  if (!snapshot.value) return
  aliasDraft.value = snapshot.value.alias ?? ''
  aliasEditing.value = true
  void nextTick(() => {
    aliasInput.value?.focus()
    aliasInput.value?.select()
  })
}

/** 取消编辑：恢复原值，不写后端 */
function cancelAliasEdit() {
  aliasEditing.value = false
  aliasDraft.value = snapshot.value?.alias ?? ''
}

/** 保存设备名：只有 ✓ 按钮 / Enter 会走到这里，blur 不保存 */
async function saveAlias() {
  if (!snapshot.value || aliasSaving.value) return
  const alias = aliasDraft.value.trim()
  if (!alias) {
    message.warning('设备名不能为空')
    return
  }
  if (alias === snapshot.value.alias) {
    aliasEditing.value = false
    return
  }
  aliasSaving.value = true
  try {
    applySnapshot(await setLanShareAlias(alias))
    aliasEditing.value = false
    message.success('设备名已保存')
  } catch (err) {
    // 失败时保留编辑态和输入内容，方便直接重试
    message.error(`保存设备名失败：${errorText(err)}`)
  } finally {
    aliasSaving.value = false
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
    <div class="lan-body">
      <div class="lan-head">
        <h3>局域网分享</h3>
      </div>

      <p v-if="loadError" class="lan-alert">{{ loadError }}</p>
      <p v-if="!snapshot && loading" class="lan-empty">正在读取局域网分享状态…</p>

      <template v-if="snapshot">
        <p v-if="snapshot.last_error" class="lan-alert">
          启动失败：{{ snapshot.last_error }}
          <span class="lan-alert-hint">请检查端口 / 防火墙设置后重新开启。</span>
        </p>

        <!-- 顶部状态卡：设备名就地编辑 + 监听信息 + 接收目录 -->
        <section class="card local-card">
          <div class="local-grid">
            <div class="field field-alias">
              <span class="label">本机设备</span>
              <div v-if="!aliasEditing" class="alias-view">
                <span class="alias-name" :title="snapshot.alias">{{ snapshot.alias || '—' }}</span>
                <button
                  class="icon-btn"
                  type="button"
                  title="编辑设备名"
                  aria-label="编辑设备名"
                  @click="startAliasEdit"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M12 20h9" />
                    <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
                  </svg>
                </button>
              </div>
              <div v-else class="alias-edit">
                <input
                  ref="aliasInput"
                  v-model="aliasDraft"
                  class="alias-input"
                  type="text"
                  maxlength="64"
                  :disabled="aliasSaving"
                  placeholder="默认使用主机名"
                  @keydown.enter.prevent="saveAlias"
                  @keydown.esc.prevent="cancelAliasEdit"
                />
                <button
                  class="icon-btn is-ok"
                  type="button"
                  title="保存（Enter）"
                  aria-label="保存设备名"
                  :disabled="aliasSaving"
                  @click="saveAlias"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M20 6 9 17l-5-5" />
                  </svg>
                </button>
                <button
                  class="icon-btn"
                  type="button"
                  title="取消（Esc）"
                  aria-label="取消编辑"
                  :disabled="aliasSaving"
                  @click="cancelAliasEdit"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M18 6 6 18M6 6l12 12" />
                  </svg>
                </button>
              </div>

              <!-- 开关与状态放在卡片右上角 -->
              <div class="local-actions">
                <span class="lan-status" :class="statusClass">
                  <span class="lan-status-dot"></span>
                  {{ statusText }}
                  <template v-if="running && snapshot?.port">（端口 {{ snapshot.port }}）</template>
                </span>
                <button
                  class="btn btn-xs"
                  :class="enabled ? 'btn-danger-ghost' : 'btn-primary'"
                  type="button"
                  :disabled="!snapshot || toggling"
                  @click="toggleEnabled"
                >
                  {{ toggling ? '处理中…' : enabled ? '关闭功能' : '开启功能' }}
                </button>
              </div>
            </div>

            <div class="field">
              <span class="label">监听端口</span>
              <span class="value mono">{{ snapshot.port ?? '—' }}</span>
            </div>
            <div class="field">
              <span class="label">本机地址</span>
              <span class="value mono">{{ localAddresses || '未获取到可用网卡地址' }}</span>
            </div>
            <div class="field">
              <span class="label">设备指纹</span>
              <span class="value mono" :title="snapshot.fingerprint">{{ shortFingerprint || '—' }}</span>
            </div>

            <div class="field dir-field">
              <span class="label">接收目录</span>
              <span class="dir-path mono" :title="snapshot.download_dir || ''">
                {{ snapshot.download_dir || '—' }}
              </span>
              <div class="dir-actions">
                <button
                  class="text-btn"
                  type="button"
                  :disabled="savingDir"
                  @click="chooseDownloadDir"
                >
                  选择目录
                </button>
                <button class="text-btn" type="button" @click="openDownloadDir">打开目录</button>
                <button
                  v-if="snapshot.download_dir_custom"
                  class="text-btn"
                  type="button"
                  :disabled="savingDir"
                  @click="resetDownloadDir"
                >
                  恢复默认
                </button>
              </div>
            </div>
          </div>
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
                type="button"
                :disabled="offerBusy === offer.id"
                @click="respondOffer(offer, false)"
              >
                拒绝
              </button>
              <button
                class="btn btn-primary btn-xs"
                type="button"
                :disabled="offerBusy === offer.id"
                @click="respondOffer(offer, true)"
              >
                {{ offerBusy === offer.id ? '处理中…' : '接受' }}
              </button>
            </div>
          </div>
        </section>

        <!-- 附近设备：卡片网格 -->
        <section class="card">
          <div class="card-head">
            <h4>附近设备</h4>
            <span v-if="enabled" class="muted count-inline">{{ peers.length }} 台在线</span>
          </div>

          <div v-if="!peers.length" class="peer-empty">
            <svg
              class="peer-empty-icon"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <circle cx="6" cy="12" r="2.3" />
              <circle cx="18" cy="6" r="2.3" />
              <circle cx="18" cy="18" r="2.3" />
              <path d="M8.1 10.9l7.8-3.8M8.1 13.1l7.8 3.8" />
            </svg>
            <p class="peer-empty-title">
              {{ enabled ? '暂时没有发现其他设备' : '局域网分享未开启' }}
            </p>
            <p class="peer-empty-hint">
              {{
                enabled
                  ? '同一局域网内打开本应用的设备会出现在这里。'
                  : '开启功能后才会搜索局域网内的设备。'
              }}
            </p>
          </div>

          <ul v-else class="peer-grid">
            <li v-for="peer in peers" :key="peer.fingerprint" class="peer-card">
              <div class="peer-card-top">
                <span class="peer-icon">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <rect x="3" y="4" width="18" height="12" rx="2" />
                    <path d="M8 20h8M12 16v4" />
                  </svg>
                </span>
                <span class="peer-online" title="在线"></span>
              </div>
              <div class="peer-name" :title="peer.alias">{{ peer.alias || '未知设备' }}</div>
              <div class="peer-addr mono" :title="`${peer.ip}:${peer.port}`">{{ peer.ip }}:{{ peer.port }}</div>
              <div class="peer-seen">{{ lastSeenText(peer) }}</div>
              <div class="peer-card-foot">
                <button
                  class="btn btn-primary btn-xs peer-send"
                  type="button"
                  :disabled="sendingPeer === peer.fingerprint"
                  @click="sendToPeer(peer)"
                >
                  {{ sendingPeer === peer.fingerprint ? '选择文件…' : '发送文件' }}
                </button>
              </div>
            </li>
          </ul>
        </section>

        <!-- 发送进度 -->
        <section v-if="transfers.length" class="card">
          <div class="card-head">
            <h4>发送进度</h4>
            <button class="btn btn-secondary btn-xs" type="button" @click="clearTransfers">
              清除记录
            </button>
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

/* 页头：和设置页同一套排版——标题在滚动内容里，不加分隔线 */
.lan-head {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 16px;
}
.lan-head h3 {
  font-size: 16px;
  color: var(--text-primary);
  margin: 0;
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
  padding: 16px 20px;
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
.count-inline {
  margin: 0;
}
/* 与 .muted 同权重，需要多一层选择器压过它的 margin */
.card-head .muted.count-inline {
  margin: 0;
}

/* 顶部状态卡：设备名 / 监听信息 / 接收目录 */
.local-card {
  padding-bottom: 12px;
}
.local-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px 20px;
}
.field {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  min-width: 0;
  min-height: 28px;
}
.field-alias {
  grid-column: 1 / -1;
}
/* 开关与状态：卡片右上角 */
.local-actions {
  margin-left: auto;
  padding-left: 12px;
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
.field .label {
  color: var(--text-muted);
  width: 64px;
  flex-shrink: 0;
}
.field .value {
  color: var(--text-secondary);
  word-break: break-all;
}

/* 设备名：普通态与编辑态同高，切换不跳布局 */
.alias-view,
.alias-edit {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  min-height: 28px;
}
.alias-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.alias-edit {
  flex: 1;
  max-width: 420px;
}
.alias-input {
  flex: 1;
  min-width: 0;
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-primary);
  font-size: 13px;
  outline: none;
}
.alias-input:focus {
  border-color: var(--primary);
}
.alias-input:disabled {
  background: var(--bg-readonly);
  color: var(--text-muted);
}
.icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  flex-shrink: 0;
  transition: color 0.15s, background 0.15s, border-color 0.15s;
}
.icon-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--primary);
}
.icon-btn.is-ok {
  color: var(--success);
}
.icon-btn.is-ok:hover:not(:disabled) {
  background: rgba(16, 185, 129, 0.12);
  color: var(--success);
}
.icon-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.icon-btn svg {
  width: 15px;
  height: 15px;
}

/* 接收目录：路径后面紧跟文字按钮，路径太长时优先省略路径 */
.dir-field {
  grid-column: 1 / -1;
}
.dir-path {
  flex: 0 1 auto;
  min-width: 0;
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dir-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}
.text-btn {
  padding: 2px 6px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  line-height: 1.5;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.15s, color 0.15s;
}
.text-btn:hover:not(:disabled) {
  background: var(--bg-hover);
}
.text-btn:disabled {
  color: var(--text-muted);
  cursor: not-allowed;
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

/* 附近设备：卡片网格（220-320px 自适应） */
.peer-grid {
  list-style: none;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 12px;
}
.peer-card {
  display: flex;
  flex-direction: column;
  gap: 3px;
  max-width: 320px;
  padding: 12px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-primary);
  transition: border-color 0.15s, box-shadow 0.15s;
}
.peer-card:hover {
  border-color: var(--primary);
  box-shadow: 0 4px 14px rgba(59, 130, 246, 0.12);
}
.peer-card-top {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  margin-bottom: 6px;
}
.peer-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: 10px;
  background: var(--bg-active);
  color: var(--primary);
  flex-shrink: 0;
}
.peer-icon svg {
  width: 22px;
  height: 22px;
}
.peer-online {
  width: 8px;
  height: 8px;
  margin: 4px 2px 0 0;
  border-radius: 50%;
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.16);
  flex-shrink: 0;
}
.peer-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.peer-addr {
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.peer-seen {
  font-size: 12px;
  color: var(--text-muted);
}
.peer-card-foot {
  display: flex;
  justify-content: flex-end;
  margin-top: 8px;
}
.peer-send {
  width: 100%;
}

/* 附近设备空状态：居中图标 + 提示 */
.peer-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 34px 16px 38px;
  text-align: center;
}
.peer-empty-icon {
  width: 56px;
  height: 56px;
  color: var(--text-muted);
  opacity: 0.65;
}
.peer-empty-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-secondary);
}
.peer-empty-hint {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
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

/* 窄窗口：顶部状态卡收成一列，避免字段挤在一起 */
@media (max-width: 760px) {
  .local-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
