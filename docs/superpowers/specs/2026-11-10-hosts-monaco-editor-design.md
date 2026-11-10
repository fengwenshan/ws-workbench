# Hosts 编辑器换成 Monaco 设计

- 日期：2026-11-10
- 状态：已定稿（方案 A · Monaco）
- 关联：`src/components/hosts/HostsEditor.vue`、`src/utils/hostsHighlight.ts`、`package.json`、README「系统 → Hosts」段

## 1. 背景与目标

Hosts 页签的编辑器原来是手写的「透明 `<textarea>` + `<pre>` 叠层高亮 + 自绘行号栏」：
textarea 负责输入 / 光标 / 选区，`pre` 负责着色，两边靠 `scrollTop` / `scrollLeft` 手动同步，
样式必须逐项对齐（字体、字号、行高、`tab-size`、内边距、换行规则）。

用户反馈：「有些地方光标过不去，鼠标点某处，光标不落在那儿」。

实测复现（真实 `HostsEditor` 组件、mock IPC、逐字点击）：

- 滚到最底部时，最后 20px 的滚动区间里 **63/63 次点击全部落错一行**（点视觉第 15 行，光标到第 16 行）；
- 滚动到 0 / 40% / 80% 时 0 次出错；
- 同一时刻行号栏也比实际行号大 1（用户截图里 `/etc/hosts` 的 Docker 段真实行号 18/19/20，界面显示 19/20/21）。

**根因**：hosts 文件以换行结尾，`textarea` 会为末尾的空行渲染一个行盒，而 `<pre>` 不会。
于是 `textarea.scrollHeight` 比 `pre.scrollHeight` 恰好大一行（20.4px）；滚到底时 `pre` 先被
clamp（`scrollTop` 305 → 实际 284.5），可见文字层比 textarea 的光标坐标系整体低一行，
行号栏又跟着 textarea 的 `scrollTop` 走、不被 clamp，所以「点的位置」和「光标落的位置」错开一行。

这类「两层排版必须逐像素一致」的叠层方案还有别的脆弱点（经典滚动条占位、字体加载抖动、
IME 组字、大文件关高亮的降级分支），补一个洞不难，但每次都要靠肉眼比对。用户希望改用
`07_scope-ftp` 里用过的在线编辑器（Monaco，VS Code 内核）。

### 目标（In Scope）

1. Hosts 页签的编辑区换成 Monaco，删掉叠层 textarea / `pre` / 自绘行号 / 滚动同步 / IME 降级。
2. 修复上面复现的点击错行、行号错位（换成 Monaco 后该类问题整体消失）。
3. 保留现有页面骨架与链路：工具栏（刷新 / 备份 / 打开目录 / 保存）、未保存标记、`⌘/Ctrl+S`、
   保存前自动备份 + 系统授权、备份还原、保存中只读、错误提示、行号、hosts 语法着色。
4. Monaco 走本地打包（离线可用、不走 CDN），并且**懒加载**：不打开 Hosts 页签不加载。

### 非目标（Out of Scope）

- 不改 hosts 的读写链路（`src-tauri/src/commands/hosts.rs`、`file.rs`、`useHosts.ts` 不动）。
- 不做多文件 / 多标签编辑、不做主题切换（应用只有浅色主题）。
- 不做 hosts 条目的语义校验（比如「同一个域名出现两次」），保持现在的「非法地址标红」。
- 不引入 CodeMirror / Monaco 的 React 封装（本仓库是 Vue）。

## 2. 方案

### 2.1 依赖

`monaco-editor@0.52.2`（与 `07_scope-ftp` 当年同一版本，精确 pin）。

- 不用 `@monaco-editor/react`（React 专用）；直接调 `monaco.editor.create`。
- 不用任何 vite/webpack 的 monaco 插件：worker 用 Vite 的 `?worker` 后缀本地打包，
  `vite.config.ts` 零改动（sibling 项目已验证过这条路）。
- 引 `monaco-editor/esm/vs/editor/edcore.main`：编辑器内核 + 全部编辑功能（查找替换、右键菜单、
  多光标、撤销重做…），但**不含**语言包。不要用 `editor.api`——它只有内核，连 ⌘F 的查找框、
  右键菜单都没有（实测 `getSupportedActions()` 返回 0 个 action）；也不要引全量 `monaco-editor`，
  那会带上 basic-languages 与 json/css/html/ts 语言服务（sibling 项目实测主 chunk 因此 +3.6MB）。
- worker 只有 `editor.worker` 一个，用 Vite 的 `?worker` 本地打包，CSP 当前是 `null`，不用改。

### 2.2 模块划分

```text
src/editor/monacoHosts.ts       懒加载 Monaco、worker 环境、注册 hosts 语言 + 主题、create 封装
src/utils/hostsTokens.ts        由 hostsHighlight.ts 收窄：只留 tokenizeHostsLine + isIPv4/isIPv6/isHostname
（删）src/utils/hostsHighlight.ts
src/components/hosts/HostsEditor.vue
                                只留页面骨架与状态：工具栏 / 备份 / 错误条 / 未保存标记 / 保存链
package.json                    + monaco-editor@0.52.2
```

`src/editor/monacoHosts.ts` 对外只暴露：

```ts
export interface HostsEditorHandle {
  getValue(): string
  setValue(value: string): void
  setReadOnly(readOnly: boolean): void
  layout(): void
  dispose(): void
}

export async function createHostsEditor(
  container: HTMLElement,
  options: { value: string; onChange: (value: string) => void; onSave: () => void },
): Promise<HostsEditorHandle>
```

组件里只按需 `await createHostsEditor(...)`，Monaco 的类型用 `import type` 引入（编译后擦除），
保证 monaco 的代码只在动态 `import()` 的 chunk 里。

### 2.3 语言与主题

- 语言 id `hosts`：`monaco.languages.register({ id: 'hosts' })` +
  `setLanguageConfiguration`（行注释 `#`，不自动闭合括号）。
- tokenizer 复用现有的 `tokenizeHostsLine`（纯函数、已有 hosts 边界处理：BOM、行尾注释、
  第一个非空白 token 必须是地址、非法地址标红），通过 `monaco.languages.setTokensProvider`
  输出 token → scope：
  `comment.hosts` / `ipv4.hosts` / `ipv6.hosts` / `hostname.hosts` / `invalid.hosts`，
  空白与普通文本不设 scope。
- 主题 `hosts-light`（`base: 'vs'`）沿用现在的配色：

| token | 颜色 | 样式 |
| --- | --- | --- |
| 注释 | `#9ca3af` | 斜体 |
| IPv4 | `#2563eb` | 粗体 |
| IPv6 | `#7c3aed` | 粗体 |
| 主机名 | `#047857` | — |
| 非法 | `#dc2626` | — |

编辑器 chrome 的颜色对齐 `src/assets/styles/main.css`：背景 `#ffffff`、正文 `#111827`、
行号 `#9ca3af`（当前行 `#4b5563`）、行号槽背景与正文一致（`#ffffff`，不加分隔线）、选区 `#dbeafe`、
光标 `#111827`、滚动条滑块 `#cbd5e1`（hover `#94a3b8`）、控件边框 `#e5e7eb`。

非法地址不再画波浪下划线（Monaco 主题只支持实线下划线，波浪线要另加 markers）；
保留「标红」，README 相应改成「非法地址标红」。

### 2.4 编辑器选项

```ts
{
  language: 'hosts',
  theme: 'hosts-light',
  fontSize: 12,
  lineHeight: 20,
  fontFamily: "'SF Mono', Menlo, Consolas, monospace",
  fontLigatures: false,
  lineNumbers: 'on',
  lineNumbersMinChars: 3,
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
  automaticLayout: true,
  padding: { top: 10, bottom: 10 },
  scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8, useShadows: false },
  unicodeHighlight: { ambiguousCharacters: false, invisibleCharacters: false },
  stickyScroll: { enabled: false },
  // 保存中只读由 setReadOnly 控制
}
```

`automaticLayout: true` 负责页签切换 / 窗口缩放后的重新布局；另外在 `active` 变 true 时补一次
`layout()`，因为 antd Tabs 的未激活页签是 `display: none`（尺寸为 0）。

### 2.5 数据流与生命周期

- Monaco 的 model 是编辑态真源；`onDidChangeModelContent` → 立即 `hosts.draft = value`
  （hosts 一般只有几 KB，字符串同步没有性能问题）→ 现有 `dirty` / 保存逻辑不变。
- 反向同步：`watch(() => hosts.draft, ...)`，只有 `draft !== handle.getValue()` 时才 `setValue`
  （刷新 / 还原 / 首次加载会走到这里），避免回环。
- `⌘/Ctrl+S`：保留现有 window `keydown`（页签激活时生效），再加一条 Monaco
  `addCommand(KeyMod.CtrlCmd | KeyCode.KeyS)` 兜底；保存时从 `handle.getValue()` 取内容。
- 保存中只读：`watch(() => hosts.saving, v => handle.setReadOnly(v))`。
- 编辑器创建时机：`status.readable` 为真 → 容器渲染 → `nextTick` 后 `await createHostsEditor`
  （加载 chunk 期间显示「正在加载编辑器…」）；创建失败显示错误条 + 重试按钮。
- 卸载（`onUnmounted`）时 `dispose()`（editor + model）。
- 删掉「超过 32 KB 关闭高亮」分支：Monaco 本身没有这个需要。

## 3. 验证

1. `pnpm build`（`vue-tsc` + vite）通过；`dist/assets` 里 Monaco 是独立 chunk，
   主 chunk 尺寸基本不变。
2. 用 mock IPC 探针（`.tmp/hosts-probe.html` 挂真实 `HostsEditor`）在浏览器里回归：
   - 逐字点击：滚动 0 / 40% / 80% / 100% 各点 60+ 个字符位置，`editor.getPosition()` 的 offset
     必须等于点中字符的 offset（改前底部署 63/63 错一行）；
   - 行号与内容逐行一致（截图核对，特别是文件末尾）；
   - `⌘S` 与「保存」按钮都触发 mock 的 `hosts_save`，参数是完整内容；
   - 输入后出现「有未保存改动」，刷新确认框流程正常；
   - 页面无外部网络请求（确认不走 CDN）；
   - 截图核对边框 / 圆角 / 高度 / 配色与改前一致。
3. 用户在 `npm run tauri dev` 里用 WKWebView 实际点一遍（菜单栏 / 中文注释 / IME 输入中文注释）。

## 4. 取舍与风险

- dist 体积变大：Monaco 的懒加载 chunk 实测 **3.3MB（gzip 850KB）** + `editor.worker` 232KB
  （gzip 约 70KB）+ 133KB 编辑器 CSS（gzip 21KB）；只打开 Hosts 页签时加载一次，之后走浏览器缓存。
  主 chunk 保持 1.12MB 不变。
- Monaco 自带滚动条 / 行号槽 / 光标样式与原来手绘的有细微差别，需要按截图微调一轮。
- Windows 上 Monaco 的高 DPI 表现、`tab-size` 与 `insertSpaces` 已在选项里固定，未实机验证。
- 备份 / 提权 / 换行风格等链路完全不动，回归面主要集中在编辑区。
