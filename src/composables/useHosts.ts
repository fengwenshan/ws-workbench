import { computed, reactive, ref } from 'vue'
import { hostsBackups, hostsOpenFileDir, hostsRestore, hostsSave, hostsStatus } from '@/api/hosts'
import { errorText } from '@/utils/error'
import type { HostsBackup, HostsSaveResult, HostsStatus } from '@/types/hosts'

/** 编辑器内部统一用 LF；保存时按原文件的换行风格还原 */
type LineEnding = '\n' | '\r\n'

function normalizeNewlines(content: string): string {
  return content.replace(/\r\n/g, '\n').replace(/\r/g, '\n')
}

function detectLineEnding(content: string): LineEnding {
  return content.includes('\r\n') ? '\r\n' : '\n'
}

/** 字节数 → 人类可读大小（状态栏 / 备份列表用） */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`
}

/**
 * 「Hosts 在线编辑」页面的状态与动作。
 *
 * 数据流：`hosts_status` 一次返回路径 + 完整内容 + 备份信息；
 * 编辑器里的 `draft` 是本地草稿，`savedContent` 是最近一次落盘 / 读取的内容，
 * 两者不一致即「未保存」。保存走 `hosts_save(content)`（整文件写入）。
 */
export function useHosts() {
  /** 系统 hosts 状态（路径、可读性、备份、提示） */
  const status = ref<HostsStatus | null>(null)
  /** 编辑器草稿（完整 hosts 内容） */
  const draft = ref('')
  /** 最近一次从磁盘读到 / 成功保存的内容，用于判断脏状态（统一 LF） */
  const savedContent = ref('')
  /** 原文件的换行风格：textarea 会把 CRLF 归一成 LF，保存时再还原，避免无谓改写 */
  const lineEnding = ref<LineEnding>('\n')
  /** 备份列表（新→旧） */
  const backups = ref<HostsBackup[]>([])

  const loaded = ref(false)
  const loading = ref(false)
  const backupsLoading = ref(false)
  const saving = ref(false)
  const restoringId = ref('')
  const error = ref('')

  const dirty = computed(() => draft.value !== savedContent.value)
  /** 编辑器行数（tail 空行也算一行，和编辑器显示一致） */
  const lineCount = computed(() => (draft.value === '' ? 1 : draft.value.split('\n').length))
  /** 草稿字节数（UTF-8；和后端上限同一口径） */
  const size = computed(() => new TextEncoder().encode(draft.value).length)
  const backupCount = computed(() => status.value?.backup_count ?? backups.value.length)
  const lastBackupAt = computed(
    () => status.value?.last_backup_at || backups.value[0]?.created_at || '',
  )
  const busy = computed(() => loading.value || saving.value || restoringId.value !== '')

  function fail(err: unknown, fallback: string): void {
    error.value = errorText(err, fallback)
  }

  function clearError(): void {
    error.value = ''
  }

  /** 把后端返回的内容同步到状态 + 草稿（会覆盖未保存改动，调用方负责确认）。 */
  function applyStatus(next: HostsStatus): void {
    status.value = next
    lineEnding.value = detectLineEnding(next.content)
    const normalized = normalizeNewlines(next.content)
    draft.value = normalized
    savedContent.value = normalized
  }

  /** 保存前把编辑器的 LF 还原成原文件的换行风格 */
  function applyLineEnding(content: string): string {
    return lineEnding.value === '\r\n' ? content.replace(/\n/g, '\r\n') : content
  }

  /** 拉取状态 + 备份；首次进入 / 手动刷新 / 保存后调用。 */
  async function load(): Promise<boolean> {
    if (loading.value) return false
    loading.value = true
    error.value = ''
    try {
      const [nextStatus, nextBackups] = await Promise.all([hostsStatus(), hostsBackups()])
      applyStatus(nextStatus)
      backups.value = nextBackups
      loaded.value = true
      return true
    } catch (err) {
      fail(err, '读取 hosts 失败')
      return false
    } finally {
      loading.value = false
    }
  }

  /** 只刷新系统状态与备份，不动本地草稿（保存成功 / 切回页签时用）。 */
  async function refreshStatus(): Promise<void> {
    try {
      const [nextStatus, nextBackups] = await Promise.all([hostsStatus(), hostsBackups()])
      status.value = nextStatus
      backups.value = nextBackups
    } catch (err) {
      fail(err, '刷新 hosts 状态失败')
    }
  }

  /** 备份列表（展开备份面板时刷新）。 */
  async function loadBackups(): Promise<void> {
    if (backupsLoading.value) return
    backupsLoading.value = true
    try {
      backups.value = await hostsBackups()
    } catch (err) {
      fail(err, '读取备份列表失败')
    } finally {
      backupsLoading.value = false
    }
  }

  /** 保存整份内容；成功返回结果，失败返回 null（错误进 error）。 */
  async function save(): Promise<HostsSaveResult | null> {
    if (saving.value) return null
    saving.value = true
    error.value = ''
    const snapshot = draft.value
    try {
      const result = await hostsSave(applyLineEnding(snapshot))
      // 写成功后磁盘内容就是这一份草稿；保存期间用户继续打字也不会被覆盖
      savedContent.value = result.saved ? snapshot : savedContent.value
      await refreshStatus()
      return result
    } catch (err) {
      fail(err, '保存 hosts 失败')
      return null
    } finally {
      saving.value = false
    }
  }

  /** 从备份还原；成功后重新读取内容（会覆盖草稿）。 */
  async function restore(backupId: string): Promise<HostsSaveResult | null> {
    if (restoringId.value !== '') return null
    restoringId.value = backupId
    error.value = ''
    try {
      const result = await hostsRestore(backupId)
      await refreshStatus()
      if (result.saved) await load()
      return result
    } catch (err) {
      fail(err, '从备份还原失败')
      return null
    } finally {
      restoringId.value = ''
    }
  }

  async function openDir(): Promise<boolean> {
    try {
      await hostsOpenFileDir()
      return true
    } catch (err) {
      fail(err, '打开 hosts 目录失败')
      return false
    }
  }

  // 用 reactive 包一层：模板里直接写 hosts.status / hosts.save()，不用到处 .value
  return reactive({
    status,
    draft,
    savedContent,
    lineEnding,
    backups,
    loaded,
    loading,
    backupsLoading,
    saving,
    restoringId,
    error,
    dirty,
    lineCount,
    size,
    backupCount,
    lastBackupAt,
    busy,
    load,
    refreshStatus,
    loadBackups,
    save,
    restore,
    openDir,
    clearError,
  })
}
