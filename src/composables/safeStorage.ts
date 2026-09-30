import { invoke } from '@tauri-apps/api/core'

/** 后端密文信封前缀，与 Rust crypto.rs 的 PREFIX 保持一致 */
const ENCRYPTED_PREFIX = 'v1.'

/**
 * 读取本地键值：密文经后端解密后返回明文字符串；不存在返回 null。
 * 历史明文值会被直接采用，并异步回写为密文。
 */
export async function safeGet(key: string): Promise<string | null> {
  if (typeof localStorage === 'undefined') return null
  let raw: string | null
  try {
    raw = localStorage.getItem(key)
  } catch {
    // 隐私模式下 localStorage 可能抛 SecurityError，按「无值」处理
    return null
  }
  if (raw === null) return null

  if (!raw.trimStart().startsWith(ENCRYPTED_PREFIX)) {
    // 历史明文：直接采用，并异步升级为密文
    void safeSet(key, raw)
    return raw
  }

  try {
    return await invoke<string>('vault_decrypt', { cipher: raw })
  } catch (err) {
    console.error(`[safeStorage] 解密 ${key} 失败`, err)
    return null
  }
}

/**
 * 写入本地键值：经后端加密后以密文落盘。
 * 加密不可用时降级为明文写入并打日志，不阻断功能。
 */
export async function safeSet(key: string, value: string): Promise<void> {
  if (typeof localStorage === 'undefined') return
  try {
    const cipher = await invoke<string>('vault_encrypt', { plain: value })
    localStorage.setItem(key, cipher)
  } catch (err) {
    console.error(`[safeStorage] 加密 ${key} 失败，降级为明文`, err)
    try {
      localStorage.setItem(key, value)
    } catch {
      // localStorage 不可用时忽略，仅内存生效
    }
  }
}
