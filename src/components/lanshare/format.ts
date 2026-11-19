/**
 * 局域网分享的时间格式化：左栏摘要、聊天日期分隔、消息时间共用。
 *
 * 都基于「1 秒一跳的 now」计算，好处是页面挂着的时候「今天 / 昨天 / HH:mm」
 * 会自己跨过零点，不需要额外事件触发重渲染。
 */

export function pad2(value: number): string {
  return value < 10 ? `0${value}` : String(value)
}

/** 当天 00:00 的时间戳（本地时区），用于比较是不是同一天 */
export function startOfDay(ms: number): number {
  const date = new Date(ms)
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

/** 左栏列表时间：今天显示 HH:mm，昨天「昨天」，更早显示 MM-DD */
export function formatListTime(ms: number, nowMs: number): string {
  if (!ms) return ''
  const date = new Date(ms)
  const current = new Date(nowMs)
  const today = startOfDay(nowMs)
  const day = startOfDay(ms)
  if (day === today) return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`
  const yesterday = new Date(
    current.getFullYear(),
    current.getMonth(),
    current.getDate() - 1,
  ).getTime()
  if (day === yesterday) return '昨天'
  return `${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`
}

/** 聊天日期分隔：今天 / 昨天 / MM-DD，跨年时补年份 */
export function formatDayLabel(ms: number, nowMs: number): string {
  const current = new Date(nowMs)
  const today = startOfDay(nowMs)
  const day = startOfDay(ms)
  if (day === today) return '今天'
  const yesterday = new Date(
    current.getFullYear(),
    current.getMonth(),
    current.getDate() - 1,
  ).getTime()
  if (day === yesterday) return '昨天'
  const date = new Date(ms)
  if (date.getFullYear() !== current.getFullYear()) {
    return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`
  }
  return `${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`
}

/** 消息行上的 HH:mm */
export function formatClock(ms: number): string {
  const date = new Date(ms)
  return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`
}
