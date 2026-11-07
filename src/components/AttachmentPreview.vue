<script setup lang="ts">
/**
 * 附件应用内预览弹窗：图像走 `wst-img` 自定义协议按需解密渲染，不落临时文件。
 *
 * 上传页附件行和 ⌘P 快速搜索结果共用这一个组件，避免两处各写一份预览逻辑
 * （以前两处都是解密到系统临时目录再交给默认程序，临时文件会越积越多）。
 */
import { computed, ref, watch } from 'vue'
import { Modal } from 'ant-design-vue'
import { FileImageOutlined } from '@ant-design/icons-vue'
import type { AttachmentMeta } from '@/types'
import { noteImageUrl } from '@/utils/note-assets'

const props = defineProps<{
  open: boolean
  meta: AttachmentMeta | null
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
}>()

/** 能直接在应用内渲染的图片扩展名（`wst-img` 协议按扩展名给 Content-Type） */
const IMAGE_EXTS = new Set([
  'png',
  'jpg',
  'jpeg',
  'gif',
  'webp',
  'bmp',
  'svg',
  'ico',
  'avif',
])

const broken = ref(false)
const isImage = computed(
  () => !!props.meta && IMAGE_EXTS.has((props.meta.ext || '').toLowerCase()),
)
const title = computed(() => props.meta?.name ?? '预览附件')
const url = computed(() =>
  props.meta ? noteImageUrl(props.meta.id, props.meta.ext || '', props.meta.dir) : '',
)

// 每次打开 / 换附件都重新尝试加载，不能把上一次的加载失败带过来
watch(
  () => [props.open, props.meta?.id] as const,
  () => {
    broken.value = false
  },
)
</script>

<template>
  <Modal
    :open="open"
    :title="title"
    :footer="null"
    centered
    :width="720"
    :mask-closable="true"
    class="attachment-preview-modal"
    @update:open="emit('update:open', $event)"
  >
    <div class="attachment-preview-body">
      <img
        v-if="isImage && !broken"
        :src="url"
        :alt="title"
        class="attachment-preview-image"
        @error="broken = true"
      />
      <div v-else class="attachment-preview-placeholder">
        <FileImageOutlined class="attachment-preview-placeholder-icon" />
        <span>
          {{
            broken
              ? '资源暂时无法预览（密文文件可能已丢失）'
              : '该文件类型暂不支持在应用内预览，可下载后查看'
          }}
        </span>
      </div>
    </div>
  </Modal>
</template>

<style scoped>
.attachment-preview-body {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 220px;
  max-height: 68vh;
  overflow: auto;
}
.attachment-preview-image {
  display: block;
  max-width: 100%;
  max-height: 66vh;
  object-fit: contain;
}
.attachment-preview-placeholder {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 40px 24px;
  font-size: 13px;
  color: var(--text-secondary);
  text-align: center;
}
.attachment-preview-placeholder-icon {
  font-size: 34px;
  color: var(--text-muted);
}
</style>
