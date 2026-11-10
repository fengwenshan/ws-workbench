//! 应用自身的元信息（名字随构建变化：发布版 ws-workbench，开发版 ws-workbench-dev）

import { invoke } from '@tauri-apps/api/core'

/** 当前构建的应用显示名；调用失败时回退到发布版名字 */
export async function getAppName(): Promise<string> {
  try {
    return await invoke<string>('app_name')
  } catch {
    return 'ws-workbench'
  }
}

/** 当前是不是开发构建；调用失败时按 Vite 的 DEV 兜底 */
export async function isDevBuild(): Promise<boolean> {
  try {
    return await invoke<boolean>('dev_build')
  } catch {
    return import.meta.env.DEV
  }
}

/** 把网页的 document.title 同步成当前构建的应用名（开发版带 -dev） */
export async function applyDocumentTitle(): Promise<void> {
  document.title = await getAppName()
}
