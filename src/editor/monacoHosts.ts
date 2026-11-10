/**
 * Hosts 编辑器用的 Monaco 集成。
 *
 * 和 `07_scope-ftp` 的做法一致：本地打包、不走 CDN，worker 用 Vite 的 `?worker` 生成。
 * 不同的地方是：
 * - 本仓库是 Vue，直接调 `monaco.editor.create`，不用 `@monaco-editor/react`；
 * - 只引 `monaco-editor/esm/vs/editor/edcore.main`（编辑器内核 + 全部编辑功能，但没有语言包），
 *   不引全量包 `monaco-editor`，否则会带上 basic-languages 与 json/css/html/ts 语言服务；
 * - hosts 语言用现成的 `tokenizeHostsLine` 接成 Monaco 的 tokens provider，配色沿用旧编辑器。
 *
 * 整个模块只通过动态 `import()` 使用（见 `HostsEditor.vue`），保证不打开 Hosts 页签就不加载 Monaco。
 */
import type * as Monaco from 'monaco-editor/esm/vs/editor/editor.api'
import { type HostsTokenKind, tokenizeHostsLine } from '@/utils/hostsTokens'

type MonacoModule = typeof import('monaco-editor/esm/vs/editor/editor.api')

/** Monaco 语言 id（自定义；Monaco 没有内置 hosts） */
const HOSTS_LANGUAGE_ID = 'hosts'
/** Monaco 主题 id（配色对齐 src/assets/styles/main.css 的浅色变量） */
const HOSTS_THEME_ID = 'hosts-light'

/** 直接把 Monaco 的编辑器包成组件需要的那几个动作，组件里不出现 Monaco 类型 */
export interface HostsEditorHandle {
  getValue(): string
  setValue(value: string): void
  setReadOnly(readOnly: boolean): void
  layout(): void
  dispose(): void
}

export interface CreateHostsEditorOptions {
  /** 初始内容（来自 useHosts 的 draft） */
  value: string
  /** 内容变化（每次编辑都会回调，组件用它同步 draft） */
  onChange: (value: string) => void
  /** 编辑器内按下 ⌘/Ctrl+S */
  onSave: () => void
}

let loading: Promise<MonacoModule> | null = null

/** 懒加载 Monaco 并完成一次性注册（worker 环境 / hosts 语言 / 主题），失败后允许重试 */
export function loadHostsMonaco(): Promise<MonacoModule> {
  if (!loading) {
    loading = initHostsMonaco().catch((error: unknown) => {
      // 失败就把缓存清掉，下次进页签还能重试
      loading = null
      throw error
    })
  }
  return loading
}

async function initHostsMonaco(): Promise<MonacoModule> {
  const [monacoModule, editorWorker] = await Promise.all([
    // edcore.main = 编辑器内核 + 全部编辑功能（查找替换 / 右键菜单 / 多光标…），但不含语言包；
    // editor.api 只有内核，连 ⌘F 查找框都没有，别用它。
    import('monaco-editor/esm/vs/editor/edcore.main.js'),
    import('monaco-editor/esm/vs/editor/editor.worker?worker'),
  ])
  // edcore.main 没有自己的 .d.ts（API 与 editor.api 完全一致）
  const monaco = monacoModule as unknown as MonacoModule

  // worker 必须在任何编辑器实例创建之前挂到 globalThis；hosts 只用编辑器内核的 worker
  const workerConstructor = editorWorker.default
  const monacoGlobal = globalThis as typeof globalThis & {
    MonacoEnvironment?: { getWorker: (workerId: string, label: string) => Worker }
  }
  monacoGlobal.MonacoEnvironment = {
    getWorker: () => new workerConstructor(),
  }

  registerHostsLanguage(monaco)
  monaco.editor.defineTheme(HOSTS_THEME_ID, HOSTS_THEME)
  return monaco
}

/** token 种类 → Monaco 主题里的 scope（theme 里按 scope 配颜色） */
const TOKEN_SCOPES: Record<HostsTokenKind, string> = {
  plain: '',
  comment: 'comment.hosts',
  ip: 'ipv4.hosts',
  ip6: 'ipv6.hosts',
  host: 'hostname.hosts',
  invalid: 'invalid.hosts',
}

function registerHostsLanguage(monaco: MonacoModule): void {
  if (monaco.languages.getLanguages().some((language) => language.id === HOSTS_LANGUAGE_ID)) return

  monaco.languages.register({
    id: HOSTS_LANGUAGE_ID,
    aliases: ['Hosts', 'hosts'],
    extensions: [],
  })
  monaco.languages.setLanguageConfiguration(HOSTS_LANGUAGE_ID, {
    comments: { lineComment: '#' },
  })

  // 无状态 tokenizer：hosts 每行独立，state 只是占位
  const state: Monaco.languages.IState = {
    clone: () => state,
    equals: (other) => other === state,
  }
  monaco.languages.setTokensProvider(HOSTS_LANGUAGE_ID, {
    getInitialState: () => state,
    tokenize: (line: string) => {
      const tokens: Monaco.languages.IToken[] = []
      let startIndex = 0
      for (const token of tokenizeHostsLine(line)) {
        tokens.push({ startIndex, scopes: TOKEN_SCOPES[token.kind] })
        startIndex += token.text.length
      }
      // 空行也要有一个起点 token，否则 Monaco 认为整行没有 token
      if (tokens.length === 0) tokens.push({ startIndex: 0, scopes: '' })
      return { tokens, endState: state }
    },
  })
}

/** 编辑器 chrome 的配色：尽量贴近旧版手绘编辑器的观感 */
const HOSTS_THEME: Monaco.editor.IStandaloneThemeData = {
  base: 'vs',
  inherit: true,
  rules: [
    { token: 'comment.hosts', foreground: '9ca3af', fontStyle: 'italic' },
    { token: 'ipv4.hosts', foreground: '2563eb', fontStyle: 'bold' },
    { token: 'ipv6.hosts', foreground: '7c3aed', fontStyle: 'bold' },
    { token: 'hostname.hosts', foreground: '047857' },
    { token: 'invalid.hosts', foreground: 'dc2626' },
  ],
  colors: {
    'editor.background': '#ffffff',
    'editor.foreground': '#111827',
    'editor.lineHighlightBackground': '#f5f5f5',
    'editor.lineHighlightBorder': '#00000000',
    'editorLineNumber.foreground': '#6e7681',
    'editorLineNumber.activeForeground': '#111827',
    // 行号槽不要底色，跟正文一样纯白（只靠右对齐的数字区分）
    'editorGutter.background': '#ffffff',
    'editorCursor.foreground': '#111827',
    'editor.selectionBackground': '#dbeafe',
    'editor.inactiveSelectionBackground': '#eff6ff',
    'editor.selectionHighlightBackground': '#eff6ff',
    'editorIndentGuide.background1': '#d3d3d3',
    'editorIndentGuide.activeBackground1': '#b0b0b0',
    'editorWidget.background': '#ffffff',
    'editorWidget.border': '#e5e7eb',
    'editorSuggestWidget.background': '#ffffff',
    'editorSuggestWidget.border': '#e5e7eb',
    'editorSuggestWidget.selectedBackground': '#eff6ff',
    'editorHoverWidget.background': '#ffffff',
    'editorHoverWidget.border': '#e5e7eb',
    'input.background': '#ffffff',
    'input.border': '#e5e7eb',
    focusBorder: '#93c5fd',
    'scrollbarSlider.background': '#cbd5e180',
    'scrollbarSlider.hoverBackground': '#94a3b870',
    'scrollbarSlider.activeBackground': '#64748b80',
    'scrollbar.shadow': '#00000000',
  },
}

/** 编辑器行为选项：等宽、行号、不折行、Tab = 2 空格，尽量贴近原来的手绘编辑器 */
const HOSTS_EDITOR_OPTIONS: Monaco.editor.IStandaloneEditorConstructionOptions = {
  language: HOSTS_LANGUAGE_ID,
  theme: HOSTS_THEME_ID,
  fontSize: 12,
  lineHeight: 18,
  fontFamily: "'SF Mono', Menlo, Consolas, 'Courier New', monospace",
  fontLigatures: false,
  lineNumbers: 'on',
  // 行号列留出 5 个字符宽，行号与内容之间留出 20px（默认 10px，数字几乎贴着正文）
  lineNumbersMinChars: 5,
  lineDecorationsWidth: 26,
  glyphMargin: false,
  folding: false,
  minimap: { enabled: false },
  overviewRulerLanes: 0,
  hideCursorInOverviewRuler: true,
  renderLineHighlight: 'line',
  scrollBeyondLastLine: false,
  wordWrap: 'off',
  tabSize: 2,
  insertSpaces: true,
  detectIndentation: false,
  renderWhitespace: 'selection',
  // hosts 里同名列很多，光标处的词一高亮满屏灰块，像被选中了一样；关掉这两种联想高亮
  occurrencesHighlight: 'off',
  selectionHighlight: false,
  automaticLayout: true,
  mouseWheelZoom: false,
  padding: { top: 10, bottom: 10 },
  scrollbar: {
    verticalScrollbarSize: 8,
    horizontalScrollbarSize: 8,
    useShadows: false,
  },
  // hosts 里常有中文注释；关掉 Monaco 对「易混淆字符 / 不可见字符」的底色提示
  unicodeHighlight: { ambiguousCharacters: false, invisibleCharacters: false },
  stickyScroll: { enabled: false },
  ariaLabel: 'Hosts 文件内容',
}

/** 在容器里创建编辑器；调用前容器必须有非零尺寸（页签已激活） */
export async function createHostsEditor(
  container: HTMLElement,
  options: CreateHostsEditorOptions,
): Promise<HostsEditorHandle> {
  const monaco = await loadHostsMonaco()
  const model = monaco.editor.createModel(options.value, HOSTS_LANGUAGE_ID)
  const editor = monaco.editor.create(container, { ...HOSTS_EDITOR_OPTIONS, model })

  const changeSubscription = model.onDidChangeContent(() => {
    options.onChange(model.getValue())
  })
  // 编辑器内 ⌘/Ctrl+S：window 级监听只在编辑器没有焦点时才需要，这里补一条更可靠
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
    options.onSave()
  })

  let disposed = false
  return {
    getValue: () => model.getValue(),
    setValue: (value: string) => {
      if (model.getValue() !== value) model.setValue(value)
    },
    setReadOnly: (readOnly: boolean) => editor.updateOptions({ readOnly }),
    layout: () => editor.layout(),
    dispose: () => {
      if (disposed) return
      disposed = true
      changeSubscription.dispose()
      editor.dispose()
      model.dispose()
    },
  }
}
