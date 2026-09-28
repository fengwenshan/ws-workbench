<script setup lang="ts">
import { computed } from 'vue'
import type { VpnStatus } from '@/types/vpn'
import { MODE_LABELS } from '@/types/vpn'

const props = defineProps<{
  status: VpnStatus
}>()

const tunnelRows = computed(() => {
  const status = props.status
  const rows: { label: string; value: string }[] = []
  if (status.vpn_ip) {
    rows.push({
      label: '隧道地址',
      value: status.vpn_ip + (status.mtu ? `  MTU ${status.mtu}` : ''),
    })
  }
  if (status.tun_dev || status.tun_index) {
    rows.push({
      label: '隧道网卡',
      value: status.tun_dev + (status.tun_index ? ` (if#${status.tun_index})` : ''),
    })
  }
  if (status.vpn_gateway) {
    rows.push({ label: '隧道下一跳', value: status.vpn_gateway })
  }
  if (status.dns.length > 0) {
    rows.push({ label: '服务端 DNS', value: status.dns.join('  ') })
  }
  const snapshot = status.snapshot
  if (snapshot?.default_gateway) {
    rows.push({
      label: '本地出口',
      value: `${snapshot.default_gateway} (${snapshot.default_if_alias || 'if#' + snapshot.default_if_index})`,
    })
  }
  if (snapshot?.local_subnets?.length) {
    rows.push({ label: '本地直连网段', value: snapshot.local_subnets.join('  ') })
  }
  return rows
})

const hasRoutes = computed(() => props.status.routes.length > 0)
</script>

<template>
  <div class="preview">
    <div class="preview-head">
      <h4>当前生效的分流</h4>
      <span class="mode-tag">{{ MODE_LABELS[status.mode] }}</span>
    </div>

    <div v-if="tunnelRows.length === 0" class="preview-empty">
      连接成功后，这里会显示隧道参数与实际下发的路由。
    </div>

    <template v-else>
      <div class="info-grid">
        <div v-for="row in tunnelRows" :key="row.label" class="info-row">
          <span class="info-label">{{ row.label }}</span>
          <span class="info-value mono">{{ row.value }}</span>
        </div>
      </div>

      <div class="routes">
        <div class="routes-head">
          <span>目标网段</span>
          <span>出口</span>
          <span>下一跳 / 网卡</span>
        </div>
        <div v-for="route in status.routes" :key="route.cidr + route.via" class="route-row">
          <span class="mono">{{ route.cidr }}</span>
          <span>
            <em class="via-tag" :class="route.via === 'vpn' ? 'via-vpn' : 'via-local'">
              {{ route.via === 'vpn' ? '走 VPN' : '走本地' }}
            </em>
          </span>
          <span class="mono route-hop">
            {{ route.next_hop || 'on-link' }}
            <template v-if="route.iface"> · {{ route.iface }}</template>
          </span>
        </div>
        <div v-if="!hasRoutes" class="preview-empty">
          本次没有下发任何分流路由
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.preview {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  background: var(--bg-primary);
}
.preview-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
}
.preview-head h4 {
  font-size: 14px;
  color: var(--text-primary);
}
.mode-tag {
  font-size: 12px;
  color: var(--primary);
  border: 1px solid rgba(59, 130, 246, 0.4);
  background: rgba(59, 130, 246, 0.08);
  border-radius: 10px;
  padding: 1px 8px;
}
.preview-empty {
  font-size: 12px;
  color: var(--text-muted);
  padding: 10px 0;
  text-align: center;
}
.info-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px 16px;
  margin-bottom: 12px;
}
.info-row {
  display: flex;
  gap: 8px;
  font-size: 12px;
  padding: 2px 0;
}
.info-label {
  color: var(--text-muted);
  width: 84px;
  flex-shrink: 0;
}
.info-value {
  color: var(--text-secondary);
  word-break: break-all;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.routes {
  border: 1px solid var(--border-color);
  border-radius: 6px;
  overflow: hidden;
}
.routes-head,
.route-row {
  display: flex;
  gap: 8px;
  padding: 5px 10px;
  font-size: 12px;
  align-items: center;
}
.routes-head {
  background: var(--bg-secondary);
  color: var(--text-muted);
}
.routes-head span:first-child,
.route-row span:first-child {
  width: 170px;
  flex-shrink: 0;
}
.routes-head span:nth-child(2),
.route-row span:nth-child(2) {
  width: 72px;
  flex-shrink: 0;
}
.route-row {
  border-top: 1px solid var(--border-color);
  color: var(--text-primary);
}
.route-hop {
  color: var(--text-muted);
  word-break: break-all;
}
.via-tag {
  font-style: normal;
  font-size: 11px;
  border-radius: 8px;
  padding: 1px 7px;
}
.via-vpn {
  color: var(--primary);
  background: rgba(59, 130, 246, 0.1);
}
.via-local {
  color: var(--success);
  background: rgba(16, 185, 129, 0.1);
}
</style>
