import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { PackResult, PackProgress, ProjectConfig } from '@/types'
import { useLog } from './useLog'

export function usePack() {
  const packing = ref(false)
  const progress = ref<PackProgress | null>(null)
  const result = ref<PackResult | null>(null)
  const { logs, addLog, clearLogs } = useLog()
  let unlisten: UnlistenFn | null = null

  async function pack(project: ProjectConfig, defaultExclude: string[], asZip: boolean = false) {
    if (packing.value) return
    packing.value = true
    progress.value = null
    result.value = null

    if (unlisten) { unlisten(); unlisten = null }
    unlisten = await listen<PackProgress>('pack-progress', (event) => {
      progress.value = event.payload
      if (event.payload.phase === 'copying' && event.payload.current % 50 === 0) {
        addLog('info', `${event.payload.phase}: ${event.payload.current}/${event.payload.total} - ${event.payload.current_file}`)
      } else if (event.payload.phase === 'zipping' && event.payload.current % 50 === 0) {
        addLog('info', `压缩中: ${event.payload.current}/${event.payload.total} - ${event.payload.current_file}`)
      }
    })

    const modeLabel = asZip ? 'ZIP打包' : '打包'
    addLog('info', `开始${modeLabel}项目: ${project.name}`)
    addLog('info', `源目录: ${project.source_dir}`)
    addLog('info', `输出: ${asZip ? project.output_dir + '.zip' : project.output_dir}`)

    try {
      const command = asZip ? 'pack_to_zip' : 'pack_project'
      const res = await invoke<PackResult>(command, {
        project,
        defaultExclude,
      })
      result.value = res
      if (res.success) {
        addLog('success', `${modeLabel}完成! ${res.copied_files}/${res.total_files} 文件, 耗时 ${res.elapsed_ms}ms`)
      } else {
        addLog('warn', `${modeLabel}完成(有错误). ${res.copied_files}/${res.total_files}, 跳过 ${res.skipped_files}`)
        res.errors.forEach(e => addLog('error', e))
      }
    } catch (err: any) {
      addLog('error', `${modeLabel}失败: ${err}`)
    } finally {
      packing.value = false
      if (unlisten) { unlisten(); unlisten = null }
    }
  }

  return { packing, progress, result, logs, addLog, clearLogs, pack }
}
