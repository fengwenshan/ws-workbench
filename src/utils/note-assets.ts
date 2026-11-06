/**
 * 笔记内联资源（加密图片 / 图标）解析工具。
 *
 * 笔记里的图片走加密上传链路：正文只保存指向密文 blob 的地址，
 * macOS / Linux 为 `wst-img://localhost/img?id=...&ext=...&dir=...`，
 * Windows 为 `http://wst-img.localhost/img?id=...`。
 * 地址里已经带齐 id / ext / dir，所以「笔记附件」栏可以直接从正文解析出
 * 资源清单，不需要额外保存一份元数据（老笔记同样生效）。
 *
 * 这里只放纯函数，方便单独测试；不发请求、不碰 Tauri 命令。
 */

import { isWindows } from '@/platform'

/** 一条笔记内联资源（同一条 url 在正文里出现多次时只返回一条） */
export interface NoteAsset {
  /** 在 markdown 中第一次出现的原始片段，如 `![名称](wst-img://…)` */
  raw: string
  /** 加密资源地址 */
  url: string
  /** 附件 id（uuid），也是密文文件名（不含 `.wst`） */
  id: string
  /** 原始扩展名（小写，不含点） */
  ext: string
  /** 上传时的绝对目录 */
  dir: string
  /** 图片 alt 文本，通常就是资源名；老数据可能为空 */
  alt: string
  /** 在正文中第一次出现的位置（用于定位/替换） */
  start: number
  end: number
  /** 在正文里出现的次数 */
  occurrences: number
}

/**
 * 严格 URL 编码：`encodeURIComponent` 不会转义 `!'()*`，而圆括号在
 * Markdown 链接目标里是保留字符（目录名里带「(备份)」会把链接截断），
 * 这里额外把它们转成 `%XX`。
 */
function encodeStrict(value: string): string {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (char) => `%${char.charCodeAt(0).toString(16).toUpperCase()}`,
  )
}

/** 加密图片地址的协议前缀，跟随平台走（Windows 只能用 http 形式） */
export function noteImageUrl(id: string, ext: string, dir: string): string {
  const prefix = isWindows() ? 'http://wst-img.localhost' : 'wst-img://localhost'
  const query = `id=${encodeStrict(id)}&ext=${encodeStrict(ext)}&dir=${encodeStrict(dir)}`
  return `${prefix}/img?${query}`
}

/** 从加密图片地址里解出 id / ext / dir；不是本应用的笔记资源则返回 null */
export function parseNoteImageUrl(
  url: string,
): { id: string; ext: string; dir: string } | null {
  const match = url.match(
    /^(?:wst-img:\/\/(?:localhost)?|https?:\/\/wst-img\.localhost)\/img\?(.+)$/i,
  )
  if (!match) return null
  const params = new URLSearchParams(match[1])
  const id = params.get('id') ?? ''
  const ext = (params.get('ext') ?? '').toLowerCase()
  const dir = params.get('dir') ?? ''
  if (!id || !dir) return null
  return { id, ext, dir }
}

export function isNoteAssetUrl(url: string): boolean {
  return parseNoteImageUrl(url) !== null
}

interface AssetMatch {
  raw: string
  url: string
  alt: string
  start: number
  end: number
}

/**
 * 找出 fenced code block（``` / ~~~）覆盖的字符区间。
 * 代码块里出现的 `![x](...)` 是示例文本，不能当成真正的附件。
 */
function fencedCodeRanges(markdown: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = []
  // 按 \n 切分、保留行内可能存在的 \r，偏移量才能和 markdown 原串对上
  const lines = markdown.split('\n')
  let offset = 0
  let fenceStart = -1
  let fenceChar = ''
  let fenceLength = 0

  for (const raw of lines) {
    const line = raw.endsWith('\r') ? raw.slice(0, -1) : raw
    const match = line.match(/^\s{0,3}(`{3,}|~{3,})/)
    if (match) {
      if (fenceStart < 0) {
        fenceStart = offset
        fenceChar = match[1][0]
        fenceLength = match[1].length
      } else if (match[1][0] === fenceChar && match[1].length >= fenceLength) {
        ranges.push([fenceStart, offset + raw.length])
        fenceStart = -1
      }
    }
    offset += raw.length + 1
  }
  if (fenceStart >= 0) ranges.push([fenceStart, markdown.length])
  return ranges
}

/**
 * 行内代码（`` `![x](url)` ``）覆盖的字符区间：代码片段里的图片同样只是示例，
 * 不参与附件统计与删除/重命名。按行配对，避免跨行吞掉正文。
 */
function inlineCodeRanges(markdown: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = []
  let offset = 0
  for (const raw of markdown.split('\n')) {
    const line = raw.endsWith('\r') ? raw.slice(0, -1) : raw
    let index = 0
    while (index < line.length) {
      if (line[index] !== '`' || line[index - 1] === '\\') {
        index += 1
        continue
      }
      let length = 1
      while (line[index + length] === '`') length += 1
      const marker = '`'.repeat(length)
      const close = line.indexOf(marker, index + length)
      if (close < 0) {
        index += length
        continue
      }
      ranges.push([offset + index, offset + close + length])
      index = close + length
    }
    offset += raw.length + 1
  }
  return ranges
}

function inRanges(ranges: Array<[number, number]>, position: number): boolean {
  return ranges.some(([start, end]) => position >= start && position < end)
}

/**
 * 扫描 Markdown 图片语法（`![alt](url "title")` / `![alt](<url>)`）。
 *
 * 不用单个正则，是为了兼容目录名里的圆括号：老笔记的 URL 里可能有未转义的
 * `(` / `)`，按括号配对扫描才不会把链接截断；实在无法配对（例如目录名里
 * 只有半个括号）时直接放弃这条，避免删除/重命名时改坏正文。
 */
function findMarkdownImages(markdown: string): AssetMatch[] {
  const matches: AssetMatch[] = []
  let search = 0

  while (search < markdown.length) {
    const bang = markdown.indexOf('![', search)
    if (bang < 0) break
    search = bang + 2

    // alt 结束位置（跳过转义字符）
    let i = bang + 2
    while (i < markdown.length && markdown[i] !== ']') {
      i += markdown[i] === '\\' ? 2 : 1
    }
    if (i >= markdown.length || markdown[i + 1] !== '(') continue
    const alt = markdown.slice(bang + 2, i)

    let j = i + 2
    while (j < markdown.length && /\s/.test(markdown[j])) j += 1

    let url: string
    if (markdown[j] === '<') {
      const close = markdown.indexOf('>', j + 1)
      if (close < 0) continue
      url = markdown.slice(j + 1, close).trim()
      j = close + 1
    } else {
      // 目标 URL：圆括号按层数配对；空白在顶层表示 URL 结束（后面是 title）。
      // 注意不能用 depth !== 0 判断失败：空白结束也是 depth 仍为 1 的正常收尾。
      let depth = 1
      let k = j
      let out = ''
      let terminated = false
      while (k < markdown.length) {
        const char = markdown[k]
        if (char === '\\' && k + 1 < markdown.length) {
          out += markdown[k + 1]
          k += 2
          continue
        }
        if (char === '(') {
          depth += 1
          out += char
          k += 1
          continue
        }
        if (char === ')') {
          depth -= 1
          if (depth === 0) {
            terminated = true
            break
          }
          out += char
          k += 1
          continue
        }
        if (depth === 1 && /\s/.test(char)) {
          terminated = true
          break
        }
        out += char
        k += 1
      }
      if (depth !== 0 && !terminated) continue
      url = out.trim()
      j = k
    }

    // 可选 title（"…" / '…' / (…)）与收尾右括号
    const tail = markdown.slice(j).match(/^\s*(?:"[^"]*"|'[^']*'|\([^)]*\))?\s*\)/)
    if (!tail) continue
    const end = j + tail[0].length
    // 旧正文里目录名可能带未转义的半个括号（如 `dir=...x)%202`）：上面的
    // 配对会提前收尾，紧跟 `%` 说明后面其实还是 URL，宁可整条放弃也不能截断
    if (markdown[end] === '%') continue
    search = end
    if (!isNoteAssetUrl(url)) continue
    matches.push({
      raw: markdown.slice(bang, end),
      url,
      alt: alt.replace(/\\(.)/g, '$1'),
      start: bang,
      end,
    })
  }

  return matches
}

/** HTML 图片标签；极老的笔记里可能以 <img> 形式保存 */
const HTML_IMAGE_RE = /<img\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>/gi

function findHtmlImages(markdown: string): AssetMatch[] {
  const matches: AssetMatch[] = []
  HTML_IMAGE_RE.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = HTML_IMAGE_RE.exec(markdown)) !== null) {
    const tag = match[0]
    const altMatch = tag.match(/\balt\s*=\s*["']([^"']*)["']/i)
    matches.push({
      raw: tag,
      url: match[1],
      alt: altMatch ? altMatch[1] : '',
      start: match.index,
      end: match.index + tag.length,
    })
  }
  return matches
}

/** 找出正文里所有加密图片（markdown + HTML 两种写法，跳过代码块与行内代码） */
function findAssetMatches(markdown: string): AssetMatch[] {
  const ranges = [...fencedCodeRanges(markdown), ...inlineCodeRanges(markdown)]
  const matches = [...findMarkdownImages(markdown), ...findHtmlImages(markdown)]
  return matches
    .filter((match) => !inRanges(ranges, match.start))
    .sort((a, b) => a.start - b.start)
}

/**
 * 解析正文里引用的加密资源。
 * 同一地址出现多次只返回一条（occurrences 记录次数），并保留第一次出现的位置。
 */
export function parseNoteAssets(markdown: string): NoteAsset[] {
  const assets = new Map<string, NoteAsset>()
  for (const match of findAssetMatches(markdown)) {
    const parsed = parseNoteImageUrl(match.url)
    if (!parsed) continue
    const current = assets.get(match.url)
    if (current) {
      current.occurrences += 1
      continue
    }
    assets.set(match.url, {
      raw: match.raw,
      url: match.url,
      alt: match.alt,
      id: parsed.id,
      ext: parsed.ext,
      dir: parsed.dir,
      start: match.start,
      end: match.end,
      occurrences: 1,
    })
  }
  return Array.from(assets.values())
}

/** 展示用名称：优先 alt，其次按 id 生成 `资源-<id8>.<ext>` */
export function noteAssetDisplayName(asset: NoteAsset): string {
  const alt = asset.alt.trim()
  if (alt) return alt
  return `资源-${asset.id.slice(0, 8)}${asset.ext ? `.${asset.ext}` : ''}`
}

/** 解密打开 / 导出时使用的文件名：确保带扩展名，去掉路径非法字符 */
export function noteAssetFileName(asset: NoteAsset): string {
  let name = noteAssetDisplayName(asset).replace(/[\\/:*?"<>|]/g, '_')
  if (!name.trim()) name = `资源-${asset.id.slice(0, 8)}`
  if (asset.ext && !name.toLowerCase().endsWith(`.${asset.ext}`)) {
    name = `${name}.${asset.ext}`
  }
  return name
}

/** 生成插入正文用的 markdown（alt 可覆盖） */
export function noteAssetMarkdown(asset: NoteAsset, alt?: string): string {
  const text = (alt ?? asset.alt ?? '').replace(/[\]\n]/g, ' ')
  return `![${text}](${asset.url})`
}

/**
 * 按 URL 替换正文里的资源片段：先按位置收集所有匹配再一次性重建字符串，
 * 避免正则替换把代码块 / 其它资源误伤。
 */
function replaceAssetOccurrences(
  markdown: string,
  url: string,
  replacer: (match: AssetMatch) => string,
): string {
  const matches = findAssetMatches(markdown).filter((match) => match.url === url)
  if (matches.length === 0) return markdown
  let result = ''
  let cursor = 0
  for (const match of matches) {
    result += markdown.slice(cursor, match.start) + replacer(match)
    cursor = match.end
  }
  return result + markdown.slice(cursor)
}

/** 从正文里删掉该资源的全部引用，顺带收敛多余空行 */
export function removeNoteAssetFromMarkdown(
  markdown: string,
  url: string,
): string {
  const next = replaceAssetOccurrences(markdown, url, () => '')
  if (next === markdown) return markdown
  // 资源独占一行时移除后可能留下连续空行，收敛成最多一个空行
  return next.replace(/\n{3,}/g, '\n\n').replace(/[ \t]+\n/g, '\n')
}

function escapeHtmlAttr(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}

/** 重命名资源（改 alt 文本）；正文里同一地址的多个引用一起改 */
export function renameNoteAssetInMarkdown(
  markdown: string,
  url: string,
  name: string,
): string {
  const alt = name.replace(/[\]\n\r]/g, ' ').trim()
  return replaceAssetOccurrences(markdown, url, (match) => {
    if (match.raw.startsWith('![')) {
      return `![${alt}](${match.url})`
    }
    // <img> 只改 alt，保留 width / height / style 等其它属性
    const tag = match.raw
    const escaped = escapeHtmlAttr(alt)
    if (/\balt\s*=\s*(["'])[^"']*\1/i.test(tag)) {
      return tag.replace(/\balt\s*=\s*(["'])[^"']*\1/i, `alt="${escaped}"`)
    }
    return tag.replace(/\s*(\/?>)$/, ` alt="${escaped}"$1`)
  })
}
