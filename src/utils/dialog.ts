import { open, save } from '@tauri-apps/plugin-dialog'

interface PickDirectoryOptions {
  defaultPath?: string
  title?: string
}

interface PickFilesOptions {
  defaultPath?: string
  title?: string
}

/** 选择一个目录；用户取消返回 null */
export async function pickDirectory(
  options: PickDirectoryOptions = {},
): Promise<string | null> {
  const picked = await open({
    directory: true,
    multiple: false,
    defaultPath: options.defaultPath,
    title: options.title,
  })
  if (typeof picked !== 'string') return null
  return picked
}

/** 选择一个文件；用户取消返回 null */
export async function pickFile(options: PickFilesOptions = {}): Promise<string | null> {
  const picked = await open({
    directory: false,
    multiple: false,
    defaultPath: options.defaultPath,
    title: options.title,
  })
  if (typeof picked !== 'string') return null
  return picked
}

/** 选择一个或多个文件；用户取消返回空数组 */
export async function pickFiles(
  options: PickFilesOptions = {},
): Promise<string[]> {
  const picked = await open({
    directory: false,
    multiple: true,
    defaultPath: options.defaultPath,
    title: options.title,
  })
  if (!picked) return []
  return Array.isArray(picked) ? picked : [picked]
}

/** 选择保存路径；用户取消返回 null */
export async function pickSavePath(defaultPath?: string): Promise<string | null> {
  const picked = await save({ defaultPath })
  return typeof picked === 'string' ? picked : null
}
