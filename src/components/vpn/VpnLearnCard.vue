<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { VpnApi } from '@/composables/useVpn'
import type { LearnedHost, LearnedSubnet } from '@/types/vpn'

const props = defineProps<{
  vpn: VpnApi
}>()

const vpn = props.vpn

/** 明细展开状态。报告是独立文件，几十 KB，默认不读，展开才拉 */
const expanded = ref(false)
/** 刚采纳、还没等到下一轮采样回填的网段，本地先标上，免得用户以为没点中 */
const adoptedNow = ref<string[]>([])

const summary = computed(() => vpn.status.learn)

const candidates = computed<LearnedSubnet[]>(() => vpn.learnReport?.candidates ?? [])

/** 走了隧道的排前面：它们才是最可能就是分流对象的那些地址 */
const hosts = computed<LearnedHost[]>(() => {
  const list = [...(vpn.learnReport?.hosts ?? [])]
  return list.sort((a, b) => {
    if (a.via_vpn !== b.via_vpn) return a.via_vpn ? -1 : 1
    return b.hits - a.hits
  })
})

/** 主机上限 2000 条，全渲染出来页面会卡，只显示命中靠前的 */
const HOST_LIMIT = 50
const visibleHosts = computed(() => hosts.value.slice(0, HOST_LIMIT))
const hiddenHosts = computed(() => Math.max(0, hosts.value.length - HOST_LIMIT))

const scanned = computed(() => (summary.value?.scans ?? 0) > 0)

const dotClass = computed(() => {
  if (summary.value?.error) return 'dot-error'
  if (!vpn.learnEnabled) return 'dot-idle'
  if (!scanned.value) return 'dot-busy'
  return 'dot-ok'
})

const toggleLabel = computed(() => (vpn.learnEnabled ? '关闭学习模式' : '开启学习模式'))

const statusText = computed(() => {
  if (!vpn.learnEnabled) {
    return '未开启。开启后再连接一次，隧道起来就会开始采样。'
  }
  const current = summary.value
  if (!current || current.scans === 0) {
    return vpn.isConnected ? '已开启，等待隧道就绪后开始采样…' : '已开启，连接后开始采样'
  }
  const parts = [
    `已采样 ${current.scans} 轮`,
    `看到 ${current.hosts} 台远端主机`,
    `反推出 ${current.candidates} 个候选网段`,
  ]
  if (current.uncovered > 0) parts.push(`其中 ${current.uncovered} 个还没进规则表`)
  return parts.join('，')
})

/** 采样时间一变就重读一次明细，用户不用手点刷新 */
watch(
  () => summary.value?.updated_at ?? '',
  (next) => {
    if (!next || !expanded.value) return
    vpn.loadLearnReport()
  },
)

async function toggleLearn() {
  await vpn.setLearn(!vpn.learnEnabled)
}

async function toggleDetail() {
  expanded.value = !expanded.value
  if (expanded.value) await vpn.loadLearnReport()
}

async function clearAll() {
  adoptedNow.value = []
  await vpn.clearLearnReport()
}

function isAdopted(candidate: LearnedSubnet) {
  return candidate.adopted || adoptedNow.value.includes(candidate.cidr)
}

/**
 * 采纳一条候选网段。
 *
 * 只往规则表里加一条，不会自动重连 —— 路由是连接时按规则表下发的，
 * 什么时候断开重连由用户自己定，免得正在用的隧道被悄悄改掉。
 */
async function adopt(candidate: LearnedSubnet) {
  await vpn.adoptLearnedCidr(candidate.cidr, `学习模式采集，${candidate.hosts} 台主机`)
  if (!adoptedNow.value.includes(candidate.cidr)) {
    adoptedNow.value = [...adoptedNow.value, candidate.cidr]
  }
}

function portText(host: LearnedHost) {
  return host.ports.length ? host.ports.join('/') : '—'
}

function processText(host: LearnedHost) {
  return host.processes.length ? host.processes.join(', ') : '—'
}
</script>

<template>
  <div class="learn-card">
    <div class="learn-head">
      <div>
        <h4>学习模式</h4>
        <p class="learn-hint">
          连接期间每 15 秒采样一次本机已建立的连接，把实际走通的远端地址聚合成候选网段。
          只观察、只建议，不会自己往规则表里塞东西。
        </p>
      </div>
      <button
        class="btn btn-xs"
        :class="vpn.learnEnabled ? 'btn-danger' : 'btn-primary'"
        @click="toggleLearn"
      >
        {{ toggleLabel }}
      </button>
    </div>

    <div class="learn-summary">
      <span class="dot" :class="dotClass"></span>
      <span class="learn-status">{{ statusText }}</span>
      <span v-if="summary?.updated_at" class="learn-time mono">{{ summary.updated_at }}</span>
    </div>

    <p v-if="summary?.error" class="learn-error">采样失败：{{ summary.error }}</p>

    <div class="learn-actions">
      <button class="btn btn-secondary btn-xs" :disabled="vpn.learnLoading" @click="toggleDetail">
        {{ expanded ? '收起明细' : '查看采集明细' }}
      </button>
      <button class="btn btn-secondary btn-xs" :disabled="vpn.learnLoading" @click="clearAll">
        清空采集结果
      </button>
    </div>

    <div v-if="expanded" class="learn-detail">
      <div class="learn-block">
        <div class="learn-block-head">
          <span>候选网段</span>
          <span class="learn-count">{{ candidates.length }}</span>
        </div>

        <div v-if="candidates.length === 0" class="learn-empty">
          还没有候选。采到的地址太少，或者它们本来就落在已有规则里。
        </div>

        <div v-else class="learn-table">
          <div class="learn-row learn-row-head">
            <span class="col-cidr">网段</span>
            <span class="col-num">主机</span>
            <span class="col-state">状态</span>
            <span class="col-act"></span>
          </div>
          <div v-for="item in candidates" :key="item.cidr" class="learn-row">
            <span class="col-cidr">
              <span class="mono">{{ item.cidr }}</span>
              <span v-if="item.samples.length" class="learn-samples mono">
                {{ item.samples.join(' ') }}
              </span>
            </span>
            <span class="col-num">{{ item.hosts }}</span>
            <span class="col-state">
              <em v-if="isAdopted(item)" class="tag tag-ok">已在规则表</em>
              <em v-else-if="item.broad" class="tag tag-warn">大网段</em>
              <em v-else-if="item.covered" class="tag tag-muted">已被覆盖</em>
              <em v-else class="tag tag-new">未覆盖</em>
            </span>
            <span class="col-act">
              <button
                class="btn btn-secondary btn-xs"
                :disabled="isAdopted(item)"
                @click="adopt(item)"
              >
                采纳
              </button>
            </span>
          </div>
        </div>

        <p class="learn-note">
          采纳只是往规则表里加一条，重新连接后才会真正下发路由。
          <template v-if="candidates.some((item) => item.broad)">
            标着「大网段」的是从少量样本向上汇聚出来的，采纳前先确认一下范围。
          </template>
        </p>
      </div>

      <div class="learn-block">
        <div class="learn-block-head">
          <span>采到的远端主机</span>
          <span class="learn-count">{{ hosts.length }}</span>
        </div>

        <div v-if="hosts.length === 0" class="learn-empty">还没有采到任何远端地址。</div>

        <div v-else class="learn-table">
          <div class="learn-row learn-row-head">
            <span class="col-ip">地址</span>
            <span class="col-port">端口</span>
            <span class="col-proc">进程</span>
            <span class="col-num">命中</span>
            <span class="col-via">出口</span>
          </div>
          <div v-for="host in visibleHosts" :key="host.ip" class="learn-row">
            <span class="col-ip mono">{{ host.ip }}</span>
            <span class="col-port mono">{{ portText(host) }}</span>
            <span class="col-proc" :title="processText(host)">{{ processText(host) }}</span>
            <span class="col-num">{{ host.hits }}</span>
            <span class="col-via">
              <em class="tag" :class="host.via_vpn ? 'tag-ok' : 'tag-muted'">
                {{ host.via_vpn ? '走 VPN' : '走本地' }}
              </em>
            </span>
          </div>
        </div>

        <p v-if="hiddenHosts > 0" class="learn-note">
          还有 {{ hiddenHosts }} 台没显示，先看命中多的。
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.learn-card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  background: var(--bg-primary);
}
.learn-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 10px;
}
.learn-head h4 {
  font-size: 14px;
  color: var(--text-primary);
  margin-bottom: 4px;
}
.learn-hint {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
  max-width: 640px;
}
.btn-xs {
  padding: 2px 8px;
  font-size: 12px;
}
.learn-summary {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--text-secondary);
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-secondary);
}
.learn-status {
  flex: 1;
}
.learn-time {
  font-size: 11px;
  color: var(--text-muted);
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.dot-ok {
  background: var(--success);
}
.dot-idle {
  background: var(--text-muted);
}
.dot-busy {
  background: var(--warning);
}
.dot-error {
  background: var(--danger);
}
.learn-error {
  margin-top: 8px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--danger);
  word-break: break-all;
}
.learn-actions {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}
.learn-detail {
  margin-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.learn-block-head {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  margin-bottom: 6px;
}
.learn-count {
  font-size: 11px;
  color: var(--text-muted);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 0 6px;
}
.learn-table {
  border: 1px solid var(--border-color);
  border-radius: 6px;
  overflow: hidden;
}
.learn-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  font-size: 12px;
  color: var(--text-primary);
}
.learn-row + .learn-row {
  border-top: 1px solid var(--border-color);
}
.learn-row-head {
  background: var(--bg-secondary);
  color: var(--text-muted);
}
.learn-empty {
  font-size: 12px;
  color: var(--text-muted);
  padding: 10px;
  text-align: center;
  border: 1px dashed var(--border-color);
  border-radius: 6px;
}
.learn-note {
  margin-top: 6px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
.col-cidr {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.col-num {
  width: 48px;
  flex-shrink: 0;
  text-align: right;
  color: var(--text-secondary);
}
.col-state {
  width: 92px;
  flex-shrink: 0;
}
.col-act {
  width: 58px;
  flex-shrink: 0;
  text-align: right;
}
.col-ip {
  width: 132px;
  flex-shrink: 0;
}
.col-port {
  width: 88px;
  flex-shrink: 0;
  color: var(--text-secondary);
}
.col-proc {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-secondary);
}
.col-via {
  width: 66px;
  flex-shrink: 0;
}
.learn-samples {
  font-size: 11px;
  color: var(--text-muted);
  word-break: break-all;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.tag {
  font-style: normal;
  font-size: 11px;
  border-radius: 8px;
  padding: 1px 7px;
  white-space: nowrap;
}
.tag-ok {
  color: var(--success);
  background: rgba(16, 185, 129, 0.1);
}
.tag-warn {
  color: var(--warning);
  background: rgba(245, 158, 11, 0.1);
}
.tag-new {
  color: var(--primary);
  background: rgba(59, 130, 246, 0.1);
}
.tag-muted {
  color: var(--text-muted);
  background: var(--bg-tag);
}
</style>
