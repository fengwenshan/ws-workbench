# Hosts 编辑器换成 Monaco Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把「系统 → Hosts」的编辑区从手写叠层编辑器换成 Monaco，修掉滚到最底部时点击错行 / 行号错位，并保留现有工具栏、保存链路与备份还原。

**Architecture:** 新增 `src/editor/monacoHosts.ts` 负责懒加载 Monaco（`edcore.main` + `editor.worker`）、注册 `hosts` 语言（复用 `tokenizeHostsLine`）与浅色主题，并暴露 `createHostsEditor()`；`HostsEditor.vue` 只保留页面骨架与状态，通过 handle 读写 Monaco；`hostsHighlight.ts` 收窄成纯 tokenizer 的 `hostsTokens.ts`。

**Tech Stack:** Vue 3.5 + TS + Vite 6、ant-design-vue、monaco-editor 0.52.2（本地打包，无 CDN）。

**Spec:** `docs/superpowers/specs/2026-11-10-hosts-monaco-editor-design.md`

**通用命令（在仓库根执行）**

```bash
# 类型检查 + 构建（看 Monaco 是否独立 chunk、主 chunk 是否不变）
pnpm build

# 组件探针：mock Tauri IPC 后挂真实 HostsEditor（浏览器里点）
#   http://localhost:8790/.tmp/hosts-probe.html
```

---

## Phase A：依赖与编辑器模块

### Task 1: 装依赖

**Files:** `package.json`、`pnpm-lock.yaml`

- [x] `pnpm add monaco-editor@0.52.2`（与 sibling 项目 `07_scope-ftp` 同版本）。
- [x] 不加 `@monaco-editor/react`（React 专用）、不加任何 monaco vite 插件。

### Task 2: 收窄 tokenizer

**Files:** 新增 `src/utils/hostsTokens.ts`，删除 `src/utils/hostsHighlight.ts`

- [x] 只保留 `HostsTokenKind` / `HostsToken` / `isIPv4` / `isIPv6` / `isHostname` / `tokenizeHostsLine`。
- [x] 删掉 HTML 高亮（`highlightHosts` / `highlightLine` / 行缓存 / `HOSTS_HIGHLIGHT_LIMIT`）。

### Task 3: Monaco 集成模块

**Files:** 新增 `src/editor/monacoHosts.ts`

- [x] `loadHostsMonaco()`：动态 `import()` `monaco-editor/esm/vs/editor/edcore.main.js` 与
      `monaco-editor/esm/vs/editor/editor.worker?worker`，挂 `MonacoEnvironment`（在任何编辑器创建之前），
      注册 host 语言 + 主题；失败时清缓存以便重试。
- [x] 语言 `hosts`：`monaco.languages.register` + `setLanguageConfiguration({ comments: { lineComment: '#' } })`
      + `setTokensProvider`，把 `tokenizeHostsLine` 的每种 token 映射成 scope
      （`comment.hosts` / `ipv4.hosts` / `ipv6.hosts` / `hostname.hosts` / `invalid.hosts`）。
- [x] 主题 `hosts-light`：`base: 'vs'`，token 配色对齐旧 CSS（注释 `#9ca3af` 斜体、IPv4 `#2563eb` 粗、
      IPv6 `#7c3aed` 粗、主机名 `#047857`、非法 `#dc2626`），chrome 颜色对齐 `main.css` 变量。
- [x] `createHostsEditor(container, { value, onChange, onSave })`：`createModel` + `editor.create`，
      `onDidChangeContent` → `onChange(model.getValue())`，`addCommand(CtrlCmd|KeyS)` → `onSave`，
      返回 `{ getValue, setValue, setReadOnly, layout, dispose }`。
- [x] 选项：`lineNumbers on` / `minimap 关` / `wordWrap off` / `fontSize 12` / `lineHeight 20` /
      `tabSize 2` + `insertSpaces` + `detectIndentation false` / `automaticLayout` / `padding 10` /
      `scrollBeyondLastLine false` / `unicodeHighlight 关` / `stickyScroll 关`。

## Phase B：页面改造

### Task 4: 重写 `HostsEditor.vue` 的编辑区

**Files:** `src/components/hosts/HostsEditor.vue`

- [x] 删掉 textarea / `pre` / gutter / `syncScroll` / Tab 处理 / IME 兼容 / `showHighlight` / 大文件降级。
- [x] 模板：`<div class="hosts-code">` 内放 `<div ref="editorMount" class="hosts-monaco">`，
      加载中显示「正在加载编辑器…」，失败显示错误 + 重试。
- [x] 创建时机：`status.readable` 且页签 `active` → `nextTick` → `mountEditor()`（容器必须有非零尺寸）。
- [x] 数据流：Monaco 模型为真源，编辑即同步 `hosts.draft`；外部刷新 / 还原走 `editor.setValue()`。
- [x] 保存：优先 `editor.getValue()`；保存中 `setReadOnly`；`⌘/Ctrl+S` 同时保留 window 监听与编辑器命令。
- [x] 页签切回时 `editor.layout()`（antd Tabs 未激活页签是 `display:none`）。
- [x] 卸载时 `dispose()`。
- [x] 样式：保留边框 / 圆角（不做聚焦光晕），行号槽无底色、无分隔线，只靠右对齐的数字区分；
      行号列 5 字符宽 + `lineDecorationsWidth: 20`（数字与正文留出 19px）；删掉 `.tok-*`、`.hosts-textarea` 等旧样式。

### Task 5: 文档

- [x] 新增设计文档 + 本实施计划。
- [x] README「系统 → Hosts」段：改成 Monaco 描述、去掉「超过 32 KB 关闭高亮」。
- [x] README 目录树：补 `src/editor/`、`hostsTokens`。

## Phase C：验证

### Task 6: 构建

- [x] `pnpm build`（`vue-tsc --noEmit && vite build`）通过。
- [x] chunk 检查：主 chunk 1.12MB（与改前 1.12MB 持平）；Monaco 独立懒加载 chunk
      `edcore.main-*.js` 3.30MB（gzip 851KB）；`editor.worker-*.js` 232KB；Monaco 的 CSS 单独一个 133KB 的 chunk（gzip 21KB）。

### Task 7: 浏览器回归（mock IPC 探针）

- [x] 逐字点击：滚动 0 / 40% / 80% / 100% 各 45 次，按 DOM 字符矩形点击，光标行列与点中字符**全部一致**
      （4×45=180 次，0 失败；改前手写编辑器滚到底部是 63/63 全错一行）。
- [x] 行号槽与文本行对齐：4 个滚动位置各查 30 个行号，**0 个错位**（改前底部差 20.4px）。
- [x] 滚到底部点最后一行空行 → 光标 `43:1`。
- [x] 页签隐藏（`display:none`）再切回 → 编辑器尺寸恢复，点击仍准。
- [x] 打字 → 「有未保存改动」；点「保存」→ mock `hosts_save` 收到完整内容、脏标记消失。
- [x] 编辑后点「刷新」→ 确认框「刷新会丢弃未保存的改动」→「丢弃并刷新」→ 内容回到磁盘版本。
- [x] `⌘F` 打开 Monaco 查找框；`⌘Z` / `⌘⇧Z` 撤销重做；Tab 插入 2 空格；中文注释正常显示。
- [x] 无外部网络请求（Monaco 全部本地打包，不走 CDN）；控制台 0 报错。

### Task 8: 实机验收（用户）

- [ ] `npm run tauri dev` 里点一遍：WKWebView 下的光标定位、IME 输入中文注释、备份还原。
