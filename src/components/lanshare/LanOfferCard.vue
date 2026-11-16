<script setup lang="ts">
import type { PendingOffer } from '@/api/lanshare'
import { formatBytes } from '@/components/cleaner/format'

/**
 * 聊天里的待接收文件请求卡片：文件列表 + 倒计时 + 接受 / 拒绝。
 *
 * 倒计时用父组件 1 秒一跳的 now 计算，不用自己开定时器；
 * 接受成功后由后端生成 direction=in 的文件消息，卡片随快照消失。
 */
const props = defineProps<{
  offer: PendingOffer
  /** 正在处理中的 offer id（等于本地 offer.id 时禁用按钮） */
  busyId: string | null
  /** 1 秒一跳的时间基准 */
  now: number
}>()

const emit = defineEmits<{
  accept: [offer: PendingOffer]
  reject: [offer: PendingOffer]
}>()

/** 倒计时：过点显示「即将超时」 */
function remainingText(offer: PendingOffer): string {
  if (!offer.expires_in_ms || !offer.created_at_ms) return ''
  const remainMs = offer.created_at_ms + offer.expires_in_ms - props.now
  if (remainMs <= 0) return '即将超时'
  return `${Math.ceil(remainMs / 1000)} 秒后自动拒绝`
}
</script>

<template>
  <div class="offer-card">
    <div class="offer-head">
      <span class="offer-title">对方请求发送 {{ offer.files.length }} 个文件</span>
      <span class="offer-expire">{{ remainingText(offer) }}</span>
    </div>
    <ul class="offer-files">
      <li v-for="file in offer.files" :key="file.id" class="offer-file">
        <span class="offer-file-name" :title="file.file_name">{{ file.file_name }}</span>
        <span class="offer-file-size">{{ formatBytes(file.size) }}</span>
      </li>
    </ul>
    <div class="offer-foot">
      <span class="offer-total">共 {{ formatBytes(offer.total_size) }}</span>
      <button
        class="btn btn-secondary btn-xs"
        type="button"
        :disabled="busyId === offer.id"
        @click="emit('reject', offer)"
      >
        拒绝
      </button>
      <button
        class="btn btn-primary btn-xs"
        type="button"
        :disabled="busyId === offer.id"
        @click="emit('accept', offer)"
      >
        {{ busyId === offer.id ? '处理中…' : '接受' }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.offer-card {
  width: 340px;
  max-width: 78%;
  align-self: flex-start;
  padding: 12px;
  border: 1px solid rgba(245, 158, 11, 0.4);
  border-radius: 10px;
  background: rgba(245, 158, 11, 0.07);
}
.offer-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 8px;
}
.offer-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}
.offer-expire {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--warning);
}
.offer-files {
  list-style: none;
  max-height: 132px;
  overflow-y: auto;
  margin-bottom: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.offer-file {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 4px 8px;
  border-radius: 4px;
  background: var(--bg-primary);
  font-size: 12px;
}
.offer-file-name {
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.offer-file-size {
  flex-shrink: 0;
  color: var(--text-muted);
}
.offer-foot {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
}
.offer-total {
  margin-right: auto;
  font-size: 12px;
  color: var(--text-secondary);
}
.btn.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
}
</style>
