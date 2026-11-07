import { reactive, ref } from 'vue'
import {
  listDefaultAppCandidates,
  listDefaultApps,
  openDefaultAppWith,
  revealDefaultAppsSettings,
  setDefaultApp,
} from '@/api/defaultapps'
import type { DefaultAppCandidate, DefaultAppEntry } from '@/types/defaultapps'
import { errorText } from '@/utils/error'

/**
 * 「默认应用」页签的状态与动作。
 *
 * 列表只在页签激活时拉一次（扩展名固定，不轮询）；
 * macOS 可以改，Windows 只能读 + 引导用户去系统设置里确认。
 */
export function useDefaultApps() {
  const entries = ref<DefaultAppEntry[]>([])
  const loading = ref(false)
  const saving = ref(false)
  /** 拉列表失败 / 改默认应用失败的提示 */
  const errorMessage = ref('')

  async function load(): Promise<void> {
    if (loading.value) return
    loading.value = true
    errorMessage.value = ''
    try {
      entries.value = await listDefaultApps()
    } catch (err) {
      errorMessage.value = errorText(err, '读取默认应用失败')
    } finally {
      loading.value = false
    }
  }

  /** 候选应用（macOS 才有；Windows 返回空表） */
  async function candidates(extension: string): Promise<DefaultAppCandidate[]> {
    try {
      return await listDefaultAppCandidates(extension)
    } catch (err) {
      errorMessage.value = errorText(err, '读取候选应用失败')
      return []
    }
  }

  /** 修改默认应用；成功返回 true 并就地更新列表 */
  async function apply(extension: string, appId: string): Promise<boolean> {
    if (saving.value) return false
    saving.value = true
    errorMessage.value = ''
    try {
      await setDefaultApp(extension, appId)
      await load()
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '修改默认应用失败')
      return false
    } finally {
      saving.value = false
    }
  }

  /** 打开系统「默认应用」设置页（Windows） */
  async function revealSettings(): Promise<boolean> {
    errorMessage.value = ''
    try {
      await revealDefaultAppsSettings()
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '打开系统设置失败')
      return false
    }
  }

  /** 调起系统「打开方式」对话框（Windows） */
  async function openWith(extension: string): Promise<boolean> {
    errorMessage.value = ''
    try {
      await openDefaultAppWith(extension)
      return true
    } catch (err) {
      errorMessage.value = errorText(err, '打开「打开方式」对话框失败')
      return false
    }
  }

  // 用 reactive 包一层：模板里直接写 defaults.entries / defaults.load()
  return reactive({
    entries,
    loading,
    saving,
    errorMessage,
    load,
    candidates,
    apply,
    revealSettings,
    openWith,
  })
}

/** 「默认应用」面板与外部共享的接口类型 */
export type DefaultAppsApi = ReturnType<typeof useDefaultApps>
