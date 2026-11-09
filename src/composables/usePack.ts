import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { PackResult, PackProgress, ProjectConfig } from '@/types'

/** 打包：进度与结果都在「项目」模块内展示，不再往全局日志里写 */
export function usePack() {
  const packing = ref(false)
  const progress = ref<PackProgress | null>(null)
  const result = ref<PackResult | null>(null)
  /** invoke 直接失败（后端未返回结果）时的错误信息 */
  const error = ref('')
  let unlisten: UnlistenFn | null = null

  async function pack(project: ProjectConfig, defaultInclude: string[], asZip: boolean = false) {
    if (packing.value) return
    packing.value = true
    progress.value = null
    result.value = null
    error.value = ''

    if (unlisten) { unlisten(); unlisten = null }
    unlisten = await listen<PackProgress>('pack-progress', (event) => {
      progress.value = event.payload
    })

    try {
      const command = asZip ? 'pack_to_zip' : 'pack_project'
      result.value = await invoke<PackResult>(command, {
        project,
        defaultInclude,
      })
    } catch (err: any) {
      error.value = `${asZip ? 'ZIP打包' : '打包'}失败: ${err}`
    } finally {
      packing.value = false
      if (unlisten) { unlisten(); unlisten = null }
    }
  }

  return { packing, progress, result, error, pack }
}
