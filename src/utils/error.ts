/** 统一把 catch 到的未知错误转成可展示文本 */
export function errorText(error: unknown, fallback = '未知错误'): string {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message || fallback
  if (error == null) return fallback
  return String(error)
}
