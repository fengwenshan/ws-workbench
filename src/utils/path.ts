/** 路径相关的小工具，兼容 Windows 反斜杠与 POSIX 斜杠 */

/** 去掉末尾分隔符后取最后一段目录/文件名 */
export function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const index = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  return index >= 0 ? trimmed.slice(index + 1) : trimmed
}

/** 拼接子路径，沿用 dir 自身的分隔符风格 */
export function joinPath(dir: string, name: string): string {
  const trimmed = dir.replace(/[\\/]+$/, '')
  const separator = trimmed.includes('\\') ? '\\' : '/'
  return `${trimmed}${separator}${name}`
}
