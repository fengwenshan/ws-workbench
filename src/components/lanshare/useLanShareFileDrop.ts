import { ref } from 'vue'
import { message as antMessage } from 'ant-design-vue'
import { importLanShareClipboardFiles, type DroppedFileInput } from '@/api/lanshare'

/**
 * 聊天区的「粘贴发送」与「拖拽发送」（设计 §5.3）。
 *
 * 从 LanChatPane 拆出来，组件只负责把事件接到模板上：
 * - 粘贴：有文本且没有文件时不拦截，走原生文本粘贴；有文件 / 纯图片时交给后端
 *   导入暂存目录，成功后抛 `onSendPaths`（走 send_files）。
 * - 拖拽：进入时高亮遮罩；放下时跳过目录、跳过 >100MB 的文件并提示，
 *   其余 File → base64 抛 `onSendDroppedFiles`（走 send_dropped_files）。
 *
 * 设备离线 / 正在发送时统一提示或忽略，避免把暂存文件落了一堆却发不出去。
 */

/** 拖拽 / 粘贴发送限制：与后端 outbox 一致，单文件 ≤ 100MB */
const MAX_DROP_FILE_BYTES = 100 * 1024 * 1024

export interface FileDropContext {
  /** 当前选中设备；null 表示还没选设备 */
  peer: () => { online: boolean } | null
  /** 正在选文件 / 发文件 */
  sendingFiles: () => boolean
  /** 粘贴导入的剪贴板文件路径（父组件走 send_files） */
  onSendPaths: (paths: string[]) => void
  /** 拖拽进来的文件内容（父组件走 send_dropped_files） */
  onSendDroppedFiles: (files: DroppedFileInput[]) => void
}

function errText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

/** File → base64（去掉 DataURL 前缀）；后端按 100MB 上限再校验一次 */
function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = typeof reader.result === 'string' ? reader.result : ''
      const comma = result.indexOf(',')
      resolve(comma >= 0 ? result.slice(comma + 1) : result)
    }
    reader.onerror = () => reject(reader.error ?? new Error('读取文件失败'))
    reader.readAsDataURL(file)
  })
}

export function useLanShareFileDrop(ctx: FileDropContext) {
  /** 拖拽高亮遮罩：用深度计数，避免在子元素间移动时闪烁 */
  const dragging = ref(false)
  let dragDepth = 0

  function hasFilePayload(event: DragEvent): boolean {
    const data = event.dataTransfer
    if (!data) return false
    if (data.files.length > 0) return true
    return Array.from(data.types).includes('Files')
  }

  function onDragEnter(event: DragEvent) {
    if (!hasFilePayload(event)) return
    dragDepth += 1
    dragging.value = true
  }

  function onDragOver(event: DragEvent) {
    if (!hasFilePayload(event)) return
    event.preventDefault()
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy'
  }

  function onDragLeave(event: DragEvent) {
    if (!hasFilePayload(event) && !dragging.value) return
    dragDepth = Math.max(0, dragDepth - 1)
    if (dragDepth === 0) dragging.value = false
  }

  function resetDrag() {
    dragDepth = 0
    dragging.value = false
  }

  async function onDrop(event: DragEvent) {
    resetDrag()
    const data = event.dataTransfer
    if (!data) return
    const target = ctx.peer()
    if (!target) {
      antMessage.warning('请先选择一台设备')
      return
    }
    if (!target.online) {
      antMessage.warning('设备已离线，无法发送文件')
      return
    }
    if (ctx.sendingFiles()) return

    // 目录判断优先用 webkitGetAsEntry；拿不到时按普通文件处理
    const picked: File[] = []
    let skippedDirs = 0
    const items = Array.from(data.items ?? []).filter((item) => item.kind === 'file')
    if (items.length > 0) {
      for (const item of items) {
        const file = item.getAsFile()
        if (!file) continue
        const entry = item.webkitGetAsEntry?.() ?? null
        if (entry?.isDirectory) {
          skippedDirs += 1
          continue
        }
        picked.push(file)
      }
    } else {
      picked.push(...Array.from(data.files))
    }

    if (skippedDirs > 0) {
      antMessage.warning(`暂不支持发送文件夹，已跳过 ${skippedDirs} 个文件夹`)
    }
    const tooLarge = picked.filter((file) => file.size > MAX_DROP_FILE_BYTES)
    const files = picked.filter((file) => file.size <= MAX_DROP_FILE_BYTES)
    if (tooLarge.length > 0) {
      antMessage.warning(`文件超过 100MB，请用「发送文件」选择（已跳过 ${tooLarge.length} 个）`)
    }
    if (!files.length) return
    try {
      const payload: DroppedFileInput[] = await Promise.all(
        files.map(async (file) => ({ name: file.name, dataBase64: await fileToBase64(file) })),
      )
      ctx.onSendDroppedFiles(payload)
    } catch (err) {
      antMessage.error(`读取文件失败：${errText(err)}`)
    }
  }

  /**
   * 粘贴：剪贴板里有文件（Finder 复制文件）或没有 text/plain（截图/图片）时，
   * 交给后端导入暂存目录再发送；有文本且没有文件时不拦截，走原生文本粘贴。
   */
  async function onPaste(event: ClipboardEvent) {
    const data = event.clipboardData
    if (!data) return
    const hasFiles = data.files.length > 0 || Array.from(data.types).includes('Files')
    const hasText = Array.from(data.types).includes('text/plain')
    if (!hasFiles && hasText) return
    event.preventDefault()
    const target = ctx.peer()
    if (!target) {
      antMessage.warning('请先选择一台设备')
      return
    }
    if (!target.online) {
      antMessage.warning('设备已离线，无法发送文件')
      return
    }
    if (ctx.sendingFiles()) {
      antMessage.warning('正在发送文件，请稍后再粘贴')
      return
    }
    try {
      const paths = await importLanShareClipboardFiles()
      if (!paths.length) return
      ctx.onSendPaths(paths)
    } catch (err) {
      antMessage.warning(errText(err))
    }
  }

  return { dragging, onPaste, onDragEnter, onDragOver, onDragLeave, onDrop }
}
