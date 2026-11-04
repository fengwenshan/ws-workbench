import { ref, computed, onMounted, reactive } from 'vue'
import {
  clearAppPath,
  importApp,
  listAppLocations,
  refreshAppSearch,
  removeManagedApp,
  setAppPath,
} from '@/api/apps'
import { openParentDir } from '@/api/system'
import { pickDirectory, pickFile } from '@/utils/dialog'
import type { AppId, AppLocation } from '@/types/apps'

/** 应用定位与路径设置的状态管理 */
export function useApps() {
  const locations = ref<AppLocation[]>([])
  const loading = ref(false)
  const lastMessage = ref('')

  const missing = computed(() => locations.value.filter((item) => !item.available))
  const allAvailable = computed(
    () => locations.value.length > 0 && missing.value.length === 0,
  )

  function find(id: AppId): AppLocation | undefined {
    return locations.value.find((item) => item.id === id)
  }

  async function refresh() {
    loading.value = true
    try {
      // 走 refresh 而不是 list，顺带清掉后端的注册表缓存 ——
      // 用户点了「重新检测」通常是刚装完软件
      locations.value = await refreshAppSearch()
    } finally {
      loading.value = false
    }
  }

  async function load() {
    loading.value = true
    try {
      locations.value = await listAppLocations()
    } finally {
      loading.value = false
    }
  }

  /** 让用户挑一个可执行文件并保存为该应用的指定路径 */
  async function pickExecutable(id: AppId): Promise<string | null> {
    const location = find(id)
    // 把期望的文件名放进对话框标题，避免用户挑错文件
    // （同一个软件家族里常有多个可执行文件）
    const expected = location?.expected_names?.join(' 或 ') ?? ''
    const title = expected
      ? `选择 ${location?.name ?? id} 的启动文件（${expected}）`
      : `选择 ${location?.name ?? id} 的启动文件`

    const path = await pickFile({ title })
    if (!path) return null
    locations.value = await setAppPath(id, path)
    lastMessage.value = `已指定 ${location?.name ?? id} 的启动文件`
    setTimeout(() => (lastMessage.value = ''), 3000)
    return path
  }

  /** 清除指定路径，回到自动查找 */
  async function clearPath(id: AppId) {
    locations.value = await clearAppPath(id)
    lastMessage.value = `已清除 ${find(id)?.name ?? id} 的指定路径，回到自动查找`
    setTimeout(() => (lastMessage.value = ''), 3000)
  }

  /** 把一个已有的应用目录复制到程序数据目录并托管 */
  async function importDirectory(id: AppId) {
    const picked = await pickDirectory({
      title: `选择 ${find(id)?.name ?? id} 所在的目录`,
    })
    if (!picked) return
    const message = await importApp(id, picked)
    await load()
    lastMessage.value = message
    setTimeout(() => (lastMessage.value = ''), 4000)
  }

  /** 移除程序托管的副本 */
  async function removeManaged(id: AppId) {
    const message = await removeManagedApp(id)
    await load()
    lastMessage.value = message
    setTimeout(() => (lastMessage.value = ''), 4000)
  }

  /** 在文件管理器里定位到该可执行文件 */
  async function revealPath(path: string) {
    await openParentDir(path)
  }

  onMounted(load)

  // 用 reactive 包一层：模板里写 apps.locations 就能直接拿到数组，
  // 不需要到处写 .value（ref 在 reactive 对象里会被自动解包）
  return reactive({
    locations,
    loading,
    lastMessage,
    missing,
    allAvailable,
    find,
    refresh,
    load,
    pickExecutable,
    clearPath,
    importDirectory,
    removeManaged,
    revealPath,
  })
}

export type AppsApi = ReturnType<typeof useApps>
