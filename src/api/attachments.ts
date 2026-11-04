import { invoke } from '@tauri-apps/api/core'
import type { AttachmentMeta } from '@/types'

/** 附件加密存储相关命令 */

export function resolveUploadDir(
  entityDir: string | null,
  defaultDir: string | null,
): Promise<string> {
  return invoke<string>('resolve_upload_dir', {
    entityDir,
    defaultDir,
  })
}

export function encryptUploadFile(
  srcPath: string,
  uploadDir: string,
  name: string,
): Promise<AttachmentMeta> {
  return invoke<AttachmentMeta>('encrypt_upload_file', {
    srcPath,
    uploadDir,
    name,
  })
}

export function encryptUploadBytes(
  uploadDir: string,
  name: string,
  ext: string,
  dataB64: string,
): Promise<AttachmentMeta> {
  return invoke<AttachmentMeta>('encrypt_upload_bytes', {
    uploadDir,
    name,
    ext,
    dataB64,
  })
}

/** 解密到临时文件并返回路径，供系统默认程序打开 */
export function decryptToTemp(
  blobDir: string,
  id: string,
  name: string,
): Promise<string> {
  return invoke<string>('decrypt_to_temp', { blobDir, id, name })
}

export function decryptExportFile(
  blobDir: string,
  id: string,
  targetPath: string,
): Promise<void> {
  return invoke('decrypt_export_file', { blobDir, id, targetPath })
}

export function removeAttachmentFile(
  blobDir: string,
  id: string,
): Promise<void> {
  return invoke('remove_attachment_file', { blobDir, id })
}
