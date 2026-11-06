import { invoke } from '@tauri-apps/api/core'
import type { AttachmentMeta, AttachmentStat } from '@/types'

/** 附件加密存储相关命令 */

/** 解析附件的统一上传目录：设置里的地址 > App 私有 attachments 目录 */
export function resolveUploadDir(defaultDir: string | null): Promise<string> {
  return invoke<string>('resolve_upload_dir', { defaultDir })
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

/** 读取密文附件的原始大小与修改时间（只读 WST1 明文头，不解密内容） */
export function statAttachment(
  blobDir: string,
  id: string,
): Promise<AttachmentStat> {
  return invoke<AttachmentStat>('stat_attachment', { blobDir, id })
}

export function removeAttachmentFile(
  blobDir: string,
  id: string,
): Promise<void> {
  return invoke('remove_attachment_file', { blobDir, id })
}
