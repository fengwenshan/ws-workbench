/**
 * 笔记正文的轻量文本统计与标题解析。
 *
 * 只服务界面展示（大纲、字数、保存状态），不追求和 Typora 的字数口径完全一致：
 * - 标题：跳过代码块里的 `#` 行，支持 ATX（`## 标题`）与 Setext（`标题\n===`）
 * - 字数：中文按字、英文/数字按词计数，忽略 Markdown 标记、URL、HTML 标签
 */

export interface NoteHeading {
  /** 1-6 */
  level: number
  /** 去掉行内标记后的标题文字 */
  text: string
  /** 在 markdown 中的行号（0 起） */
  line: number
  /** 在标题列表中的序号，用于和编辑器 DOM 里的 h1-h6 顺序对齐 */
  index: number
}

function stripInlineMarkup(text: string): string {
  return text
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/`{1,3}([^`]*)`{1,3}/g, '$1')
    .replace(/(\*\*|__)(.*?)\1/g, '$2')
    .replace(/(\*|_)(.*?)\1/g, '$2')
    .replace(/~~(.*?)~~/g, '$1')
    .replace(/<[^>]+>/g, '')
    .replace(/\\([\\`*_{}\[\]()#+\-.!])/g, '$1')
    .trim()
}

/** 解析正文标题，供「大纲」页签使用 */
export function parseNoteHeadings(markdown: string): NoteHeading[] {
  const headings: NoteHeading[] = []
  const lines = markdown.split(/\r?\n/)
  let inFence = false
  let fenceToken = ''

  for (let line = 0; line < lines.length; line++) {
    const text = lines[line]

    const fence = text.match(/^\s{0,3}(`{3,}|~{3,})/)
    if (fence) {
      if (!inFence) {
        inFence = true
        fenceToken = fence[1][0]
      } else if (fence[1][0] === fenceToken) {
        inFence = false
      }
      continue
    }
    if (inFence) continue

    // 引用块里的标题（`> # 标题`）在 IR 里同样渲染成 h 标签，
    // 大纲必须一起识别，否则后续索引会和 DOM 里的标题错位。
    const body = text.replace(/^\s{0,3}(?:>\s?)*/, '')
    const atx = body.match(/^\s{0,3}(#{1,6})\s+(.+?)\s*#*\s*$/)
    if (atx) {
      const heading = stripInlineMarkup(atx[2])
      if (!heading) continue
      headings.push({
        level: atx[1].length,
        text: heading,
        line,
        index: headings.length,
      })
      continue
    }

    // Setext：下一行是 === 或 --- 时把当前行当标题
    const next = lines[line + 1] ?? ''
    if (text.trim() && /^\s{0,3}(=+|-+)\s*$/.test(next)) {
      const heading = stripInlineMarkup(text)
      if (heading) {
        headings.push({
          level: next.includes('=') ? 1 : 2,
          text: heading,
          line,
          index: headings.length,
        })
        line += 1
      }
    }
  }

  return headings
}

/** 粗略字数：中文按字、连续英文/数字按词 */
export function countNoteWords(markdown: string): number {
  const plain = markdown
    .replace(/```[\s\S]*?```/g, (block) => {
      // 代码块里的注释/字符串也算内容，但去掉围栏本身
      return block.replace(/^\s*```.*$/gm, '')
    })
    .replace(/~~~[\s\S]*?~~~/g, '')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/<[^>]+>/g, '')
    .replace(/https?:\/\/\S+/g, '')
    .replace(/wst-img:\/\/\S+|http:\/\/wst-img\.localhost\/\S+/g, '')

  let count = 0
  const cjk =
    /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\u3040-\u30ff\uac00-\ud7af]/g
  count += (plain.match(cjk) ?? []).length
  const latin = plain
    .replace(cjk, ' ')
    .match(/[A-Za-z0-9]+(?:['-][A-Za-z0-9]+)*/g)
  count += (latin ?? []).length
  return count
}

export function noteStats(markdown: string): { words: number; headings: number } {
  return {
    words: countNoteWords(markdown),
    headings: parseNoteHeadings(markdown).length,
  }
}
