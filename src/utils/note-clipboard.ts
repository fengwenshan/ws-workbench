/**
 * 笔记编辑器的复制 / 剪切与「HTML 粘贴转义」修正。
 *
 * Vditor 的即时渲染（IR）模式自带 copy / cut 处理：它把选区按 Markdown 序列化写进
 * 剪贴板，隐藏的语法标记会变成 `\_`、`**`、反引号这些源码。笔记是所见即所得编辑器，
 * 复制出去的应该是屏幕上看到的内容，否则粘到别的应用里就会多出反斜杠
 * （例如公众号 / 密码管理器里粘出来的 `Risen\_typt@2022`）。所以这里接管 copy / cut：
 * 复用 Vditor 的 IR DOM，去掉标记层、还原代码块与链接，产出 text/plain 和 text/html。
 *
 * 另外，从带格式的来源（浏览器、Word、微信）粘贴时，Lute 的 HTML2VditorIRDOM 会把
 * 文本里的 `_` 转成反斜杠转义节点，正文里就会存下 `Risen\_typt@2022`。下划线夹在
 * 单词中间时这个转义是多余的（CommonMark 里词内 `_` 不能开 / 闭强调），
 * unescapeIntrawordUnderscores 把这类转义还原成普通字符；其它地方的转义保持不动。
 */

/** IR 里的语法标记（`**`、`#`、反引号、链接括号、URL 等）都挂在这个类上 */
const MARKER_SELECTOR = '.vditor-ir__marker'

/** 行内块容器：判断「词内」时不能越过段落 / 标题 / 单元格的边界 */
const BLOCK_SELECTOR = 'p, h1, h2, h3, h4, h5, h6, li, td, th, blockquote, pre'

/** 复制时保留的属性（粘贴到别的应用后仍有意义），其余编辑器内部属性一律去掉 */
const KEPT_ATTRS = new Set([
  'href',
  'src',
  'alt',
  'title',
  'colspan',
  'rowspan',
  'type',
  'checked',
  'start',
])

/** 复制出去时保留代码语言标记（粘回笔记还能恢复围栏语言） */
const LANGUAGE_CLASS = /^language-[\w+-]+$/

function createCodeBlock(code: string, languageClass?: string | null): HTMLElement {
  const pre = document.createElement('pre')
  const node = document.createElement('code')
  node.textContent = code
  if (languageClass) node.className = languageClass
  pre.appendChild(node)
  return pre
}

/**
 * 代码块的语言在 DOM 里是 `<code class="language-js">`，标记层里没有，
 * 复制 HTML 时带上，应用内复制 -> 粘贴才能保住围栏语言。
 */
function languageOf(code: HTMLElement | null): string | null {
  const found = Array.from(code?.classList ?? []).find((name) => LANGUAGE_CLASS.test(name))
  return found ?? null
}

/** 去掉 Vditor 的内部 class / data-* / id，避免把编辑器实现细节复制出去 */
function stripEditorAttrs(root: HTMLElement) {
  root.querySelectorAll<HTMLElement>('*').forEach((el) => {
    for (const attr of Array.from(el.attributes)) {
      if (KEPT_ATTRS.has(attr.name)) continue
      // 代码语言 class 是有语义的，保留
      if (attr.name === 'class' && el.tagName === 'CODE' && LANGUAGE_CLASS.test(attr.value)) continue
      el.removeAttribute(attr.name)
    }
  })
}

/** 用离屏节点让 innerText 参与布局，得到带换行 / 制表符的纯文本 */
function htmlToPlainText(html: string): string {
  const holder = document.createElement('div')
  holder.style.cssText = 'position: fixed; left: -10000px; top: 0; width: 900px;'
  holder.innerHTML = html
  document.body.appendChild(holder)
  try {
    return holder.innerText
  } finally {
    holder.remove()
  }
}

/**
 * 把 IR 选区转成剪贴板内容。
 * 选区为空或不在编辑器里时返回 null，调用方保持 Vditor 的默认行为。
 */
export function buildNoteClipboard(range: Range): { text: string; html: string } | null {
  if (!range || range.collapsed) return null

  const box = document.createElement('div')
  box.appendChild(range.cloneContents())

  // 代码块：可编辑层（pre.vditor-ir__marker--pre）带着 marker 类，真正展示的代码在渲染层，
  // 两者内容重复，复制时只留一份干净的 <pre><code>
  box.querySelectorAll<HTMLElement>('[data-type="code-block"]').forEach((block) => {
    const source =
      block.querySelector<HTMLElement>('pre.vditor-ir__preview code') ??
      block.querySelector<HTMLElement>('pre.vditor-ir__marker--pre code')
    block.replaceWith(createCodeBlock(source?.textContent ?? '', languageOf(source)))
  })
  // 选区只落在代码块内部时，代码块外层容器可能没被整块克隆进来，这里单独兜底
  box.querySelectorAll<HTMLElement>('pre.vditor-ir__marker--pre').forEach((pre) => {
    pre.replaceWith(
      createCodeBlock(pre.textContent ?? '', languageOf(pre.querySelector<HTMLElement>('code'))),
    )
  })
  box.querySelectorAll('pre.vditor-ir__preview').forEach((pre) => pre.remove())

  // 链接：IR 里链接文字和 URL 是分开存的（URL 在隐藏的 marker 里），还原成 <a href>
  box.querySelectorAll<HTMLElement>('[data-type="a"]').forEach((node) => {
    const text = node.querySelector('.vditor-ir__link')?.textContent ?? ''
    const url = node.querySelector<HTMLElement>('.vditor-ir__marker--link')?.textContent ?? ''
    if (!url) return
    const link = document.createElement('a')
    link.setAttribute('href', url)
    link.textContent = text
    node.replaceWith(link)
  })
  // 标记层（`**`、`#`、反引号、`\_` 的反斜杠……）去掉后就是屏幕上的内容
  box.querySelectorAll(MARKER_SELECTOR).forEach((el) => el.remove())
  box
    .querySelectorAll('[data-type="code-block-open-marker"], [data-type="code-block-close-marker"]')
    .forEach((el) => el.remove())
  // Vditor 给行内元素套的外壳 span 没有语义，脱掉；空 span 顺带清掉
  box.querySelectorAll('span').forEach((span) => span.replaceWith(...Array.from(span.childNodes)))

  stripEditorAttrs(box)
  const html = box.innerHTML
  return { text: htmlToPlainText(html).trim(), html }
}

function isInsideMarker(node: Node, root: HTMLElement): boolean {
  const element = node.nodeType === Node.TEXT_NODE ? node.parentElement : (node as Element)
  if (!element || !root.contains(element)) return true
  return element.closest(MARKER_SELECTOR) !== null
}

/** 在同一个行内块里找 target 前面 / 后面的可见字符（跨过 marker，但不跨块） */
function adjacentVisibleChar(
  root: HTMLElement,
  target: Text,
  dir: 'prev' | 'next',
): string | null {
  const block = target.parentElement?.closest<HTMLElement>(BLOCK_SELECTOR) ?? root
  const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT)
  let found: string | null = null
  let before = true
  while (walker.nextNode()) {
    const text = walker.currentNode as Text
    if (text === target) {
      before = false
      continue
    }
    if (isInsideMarker(text, block)) continue
    const data = text.data
    if (!data) continue
    if (before) {
      if (dir === 'prev') found = data[data.length - 1]
      continue
    }
    if (dir === 'next') return data[0]
  }
  return dir === 'prev' ? found : null
}

/**
 * 还原「词内下划线」的多余转义：`Risen\_typt@2022` → `Risen_typt@2022`。
 * 返回实际处理的节点数（0 表示正文没有变化）。
 */
export function unescapeIntrawordUnderscores(root: HTMLElement): number {
  const nodes = Array.from(root.querySelectorAll<HTMLElement>('[data-type="backslash"]'))
  if (nodes.length === 0) return 0
  const wordChar = /[\p{L}\p{N}_]/u
  let count = 0

  for (const node of nodes) {
    const marker =
      node.querySelector<HTMLElement>(':scope > .vditor-ir__marker') ??
      node.querySelector<HTMLElement>('.vditor-ir__marker')
    if (!marker || (marker.textContent ?? '') !== '\\') continue

    // 去掉反斜杠标记后剩下的（就是这个转义字符本身）
    const visibleNodes = Array.from(node.childNodes).filter((child) => child !== marker)
    const visible = visibleNodes.map((child) => child.textContent ?? '').join('')
    if (visible !== '_') continue
    const target = visibleNodes.find((child): child is Text => child.nodeType === Node.TEXT_NODE)
    if (!target) continue

    const prev = adjacentVisibleChar(root, target, 'prev')
    const next = adjacentVisibleChar(root, target, 'next')
    if (!prev || !next || !wordChar.test(prev) || !wordChar.test(next)) continue

    // 拆掉转义外壳，保留里面的文本节点：光标落在里面时不会被挤丢
    marker.remove()
    node.replaceWith(...Array.from(node.childNodes))
    count += 1
  }

  return count
}
