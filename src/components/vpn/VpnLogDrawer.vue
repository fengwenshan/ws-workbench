<script setup lang="ts">
import { computed } from 'vue'
import type { VpnApi } from '@/composables/useVpn'
import LogDrawer from '@/components/LogDrawer.vue'

const props = defineProps<{
  vpn: VpnApi
}>()

const emit = defineEmits<{
  close: []
}>()

const vpn = props.vpn

const statusText = computed(() => {
  if (vpn.isConnected) return '已连接，日志实时刷新'
  if (vpn.phaseBusy || vpn.connecting) return '连接中…'
  return '未连接'
})
</script>

<template>
  <LogDrawer
    title="实时日志"
    :status="statusText"
    :logs="vpn.logs"
    empty-text="暂无日志。连接 VPN 后，隧道建立、DNS、规则命中这些过程会实时显示在这里。"
    @clear="vpn.clearLogs()"
    @close="emit('close')"
  >
    <template #actions>
      <button class="btn btn-secondary btn-xs" @click="vpn.openSessionDir()">打开日志目录</button>
    </template>
  </LogDrawer>
</template>
