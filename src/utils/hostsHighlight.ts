/**
 * hosts 文件语法高亮：把整份内容转成带 `<span>` 的 HTML，供编辑器叠层渲染。
 *
 * hosts 的格式很简单：`IP 主机名 [别名...] [# 注释]`，所以不引第三方语法库，
 * 手写一个按行扫描的 tokenizer：
 * - 注释：`#` 到行尾（整行注释 / 行尾注释都算）；
 * - IPv4 / IPv6：第一个非空白 token 必须能解析成地址，否则标红；
 * - 主机名：地址之后的非空白 token，校验字符与长度，非法标红；
 * - 空白原样保留（叠层渲染必须保证文本与 textarea 完全一致，否则高亮会错位）。
 *
 * 文件很大时前端会关闭高亮（见 `HOSTS_HIGHLIGHT_LIMIT`），避免每次按键都重排整份内容。
 */

/** 超过这个字节数就不做高亮：hosts 正常只有几 KB，大文件交给纯文本编辑 */
export const HOSTS_HIGHLIGHT_LIMIT = 32 * 1024

export type HostsTokenKind = 'plain' | 'comment' | 'ip' | 'ip6' | 'host' | 'invalid'

export interface HostsToken {
  kind: HostsTokenKind
  /** 原文片段（不含 HTML 转义） */
  text: string
}


/** 主机名 / 域名：字母数字 `_` 开头，标签内允许 `-`，整体 ≤253；允许末尾的点（绝对域名） */
const HOST_LABEL_RE = /^[A-Za-z0-9_](?:[A-Za-z0-9_-]{0,61}[A-Za-z0-9_])?$/

export function isIPv4(token: string): boolean {
  const parts = token.split('.')
  if (parts.length !== 4) return false
  return parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
}

export function isIPv6(token: string): boolean {
  if (!token.includes(':') || !/^[0-9a-fA-F:.]+$/.test(token)) return false
  // 用 URL 解析器做完整校验（覆盖 `::`、内嵌 IPv4 等写法）；WebView 里一定有 URL
  try {
    new URL(`http://[${token}]/`)
    return true
  } catch {
    return false
  }
}

export function isHostname(token: string): boolean {
  if (token.length === 0 || token.length > 253) return false
  const body = token.endsWith('.') ? token.slice(0, -1) : token
  if (!body) return false
  return body.split('.').every((label) => HOST_LABEL_RE.test(label))
}

/**
 * 单行 token 化：返回的 token 按顺序拼接后必须与入参完全一致（含空白与注释）。
 */
export function tokenizeHostsLine(line: string): HostsToken[] {
  if (line === '') return []

  // Windows 的 hosts 可能带 BOM：单独当空白处理，别把第一行地址标成非法
  const bom = line.startsWith('\uFEFF') ? '\uFEFF' : ''
  line = line.slice(bom.length)

  const commentIndex = line.indexOf('#')
  const code = commentIndex === -1 ? line : line.slice(0, commentIndex)
  const comment = commentIndex === -1 ? '' : line.slice(commentIndex)

  const tokens: HostsToken[] = []
  let seenAddress = false
  const re = /(\s+)|(\S+)/g
  let match: RegExpExecArray | null
  while ((match = re.exec(code)) !== null) {
    if (match[1]) {
      tokens.push({ kind: 'plain', text: match[1] })
      continue
    }
    const token = match[2]
    if (!seenAddress) {
      seenAddress = true
      if (isIPv4(token)) tokens.push({ kind: 'ip', text: token })
      else if (isIPv6(token)) tokens.push({ kind: 'ip6', text: token })
      else tokens.push({ kind: 'invalid', text: token })
    } else {
      tokens.push({ kind: isHostname(token) ? 'host' : 'invalid', text: token })
    }
  }
  if (comment) tokens.push({ kind: 'comment', text: comment })
  if (bom) tokens.unshift({ kind: 'plain', text: bom })
  return tokens
}

const TOKEN_CLASS: Record<HostsTokenKind, string> = {
  plain: '',
  comment: 'tok-comment',
  ip: 'tok-ip',
  ip6: 'tok-ip6',
  host: 'tok-host',
  invalid: 'tok-invalid',
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/** 单行高亮（带缓存：打字时只有改动的那几行会重新 token 化） */
const lineCache = new Map<string, string>()

function highlightLine(line: string): string {
  const cached = lineCache.get(line)
  if (cached !== undefined) return cached
  const html = tokenizeHostsLine(line)
    .map((token) => {
      const className = TOKEN_CLASS[token.kind]
      const escaped = escapeHtml(token.text)
      return className ? `<span class="${className}">${escaped}</span>` : escaped
    })
    .join('')
  if (lineCache.size > 4000) lineCache.clear()
  lineCache.set(line, html)
  return html
}

/**
 * 整份内容 → 高亮 HTML。
 *
 * 输出文本内容（去掉 `<span>` 后）必须与原内容逐字符一致，否则叠层会错位；
 * 换行用 `\n` 原样保留，在 `<pre>` 里渲染成换行。
 */
export function highlightHosts(content: string): string {
  return content.split('\n').map(highlightLine).join('\n')
}
