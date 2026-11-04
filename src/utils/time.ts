/** 时间相关的小工具，统一前端时间格式，避免各处各写一套 */

/** 当前时间的 ISO 字符串（用于 created_at / updated_at 落盘） */
export function nowIso(): string {
  return new Date().toISOString()
}

/** 日志行使用的时间标签：只显示时分秒 */
export function timeLabel(date: Date = new Date()): string {
  return date.toLocaleTimeString('zh-CN')
}

/** 附件等场景展示的完整日期时间，非法值返回空串 */
export function formatDateTime(value: number | string | Date): string {
  const date = value instanceof Date ? value : new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return date.toLocaleString()
}
