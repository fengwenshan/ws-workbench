//! 更新检查：按通道（stable / beta）走后端的 `update_check` 命令。
//!
//! 选端点、两个清单取版本更高者都在 Rust 侧做（见 src-tauri/src/updates.rs）；
//! 这里只负责把后端返回的元数据还原成插件里的 `Update` 对象，后续下载 / 安装
//! 继续用 `update_download_resumed` / `update_install_resumed`（靠 rid 取回）。

import { invoke } from '@tauri-apps/api/core'
import { Update } from '@tauri-apps/plugin-updater'

export type UpdateChannel = 'stable' | 'beta'

/** 后端 `update_check` 的返回（字段名与 Rust 侧约定一致，勿改） */
export interface UpdateCheckResult {
  rid: number
  currentVersion: string
  version: string
  date?: string | null
  body?: string | null
  rawJson: Record<string, unknown>
  /** 这次结果来自哪个通道 */
  channel: UpdateChannel
}

/**
 * 检查更新。
 *
 * @param channel 生效通道：stable 只看稳定清单；beta 会同时看 beta 与稳定清单
 * @param timeout 单个端点的超时（毫秒），后端默认 10s
 */
export async function checkUpdateForChannel(
  channel: UpdateChannel,
  timeout: number,
): Promise<Update | null> {
  const result = await invoke<UpdateCheckResult | null>('update_check', { channel, timeout })
  if (!result) return null
  // 用插件导出的 Update 类包一层：它自带 close()（释放后端 resource table 里的 update），
  // 也和 `pending.download()` 等原有类型兼容。
  return new Update({
    rid: result.rid,
    currentVersion: result.currentVersion,
    version: result.version,
    date: result.date ?? undefined,
    body: result.body ?? undefined,
    rawJson: result.rawJson,
  })
}
