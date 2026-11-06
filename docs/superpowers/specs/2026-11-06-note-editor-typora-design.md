# 笔记编辑器仿 Typora + 笔记附件栏 设计文档

- 日期：2026-11-06
- 分支：feat/uninstaller
- 状态：已确认（用户选定方案：Vditor 即时渲染 + 正文资源自动解析）

## 1. 背景与目标

当前分组 / 项目的笔记用 `md-editor-v3` 编辑：左右分栏（左编辑、右预览），工具栏偏重 Markdown 语法操作，实际使用中「所见即所得」体验差，用户反馈难用。同时笔记里插入的图标 / 图片资源（走加密 `wst-img` 协议）刻意不登记为附件，在界面上看不到、也管理不了。

目标（In Scope）：

1. 笔记编辑器换成 **Vditor 4 即时渲染（IR）**，单栏编辑、边写边排版，交互参考本机 `/Applications/Typora.app`；保留源码模式切换、大纲、字数统计。
2. 笔记页右侧新增 **「附件 / 大纲」栏**：
   - 附件：自动解析正文中引用的加密图片 / 图标，展示缩略图、名称、大小，可插入、预览、复制 Markdown、打开、导出、重命名、删除（删除同时移正文引用与密文 blob）。
   - 大纲：解析正文标题，点击滚动到对应位置。
3. 图片链路保持不变：仍是 `encryptUploadBytes` 加密落盘 + `wst-img://`（Windows 为 `http://wst-img.localhost`）协议渲染，不产生明文图片文件。
4. 桌面应用离线可用：Vditor 的 lute / highlight.js / i18n 等运行时资源从本地加载，不走 CDN。
5. 现有笔记正文、配置结构、附件加密格式完全兼容，不做数据迁移。

非目标（Out of Scope）：

- 数学公式 / mermaid / 图表（与现状一致，Vditor 不加载这些资源）。
- 独立的「笔记附件清单」（上传即登记、正文删除后仍保留）与孤儿加密 blob 清理。
- 专注模式 / 打字机模式。
- 其它页面（上传、配置、代理等）的改造。

## 2. 现状盘点

- 笔记：`GroupDetail.vue` / `ProjectDetail.vue` 的「笔记」tab 渲染 `NoteEditor.vue`（`md-editor-v3`）。
- 笔记内容是 `ProjectGroup.note` / `ProjectConfig.note` 的 Markdown 字符串，500ms 防抖后经 `updateNote` 写回配置。
- 笔记图片：工具栏选图 / 粘贴图片 → `encryptUploadBytes` → 正文保存 `![alt](wst-img://localhost/img?id=<uuid>&ext=<ext>&dir=<urlencoded dir>)`；Windows 前缀为 `http://wst-img.localhost`。密文 blob 为 `<dir>/<uuid>.wst`，运行时由 `wst-img` 异步协议按需解密返回。
- 「上传」tab 的附件列表（`AttachmentPanel.vue`）与笔记图片互不登记。
- WST1 容器头为明文：magic(4) + version(1) + flags(1) + orig_size(8)，因此不摸密钥也能拿到原始大小。

## 3. 方案总览

```
┌──────────────────────── NoteWorkspace.vue ────────────────────────┐
│ ┌───────────────────────────┐  ┌──────────── NoteSidebar.vue ────┐ │
│ │ NoteEditor.vue (Vditor IR)│  │ tabs: 附件 N / 大纲 N           │ │
│ │  - toolbar / IR-SV 切换   │  │ 附件卡片：缩略图 + 操作          │ │
│ │  - 图片加密上传           │  │ 大纲：标题树，点击滚动           │ │
│ │  - 状态栏：字数 / 已保存  │  │ 上传资源 / 拖拽上传              │ │
│ └───────────────────────────┘  └─────────────────────────────────┘ │
└────────────────────────────────────────────────────────────────────┘
            │ update(note)                  │ insert / updateMarkdown
            ▼                               ▼
     详情页（Group/ProjectDetail） ──► useConfig.updateNote ──► config.json
```

### 3.1 编辑器（NoteEditor.vue）

- 依赖 `vditor@4.0.1`，移除 `md-editor-v3`。
- 初始化：`mode: 'ir'`、`cdn: '/vditor'`、`theme: 'classic'`、`lang: 'zh_CN'`、`icon: 'material'`、`cache: { enable: false }`、`outline/counter` 关闭（大纲与字数由外层自己画）。
- 工具栏：`headings, bold, italic, strike, link, |, list, ordered-list, check, quote, line, code, inline-code, |, upload, table, |, undo, redo, |, edit-mode, preview`；`toolbarConfig.pin = true`。
- 图片上传：`upload.multiple = true`，`upload.handler(files)` 内逐个读 base64 → `resolveUploadDir` → `encryptUploadBytes` → `noteImageUrl`，全部成功后 `vditor.insertValue(...)` 插入 `![](url)`；配置了 handler 时 Vditor 不会自动插入，必须自己插。
- 保存：沿用现有策略（500ms 防抖 `emit('update')`；切换实体时 `watch(props.note)` 同步；卸载前补交；`destroy()` 清理）。
- 状态栏：字数（`countNoteWords`）+ 保存状态（编辑中… / 已保存）。
- 对外暴露：`insertMarkdown(md)`、`setMarkdown(md)`（静默，不触发 update）、`focus()`、`scrollToHeading(index)`。
- 视觉：复用 `main.css` 的 CSS 变量覆盖 Vditor 的边框 / 圆角 / 字体 / 背景 / 代码块配色，与现有界面一致。

### 3.2 离线资源管线（vite.config.ts）

Vditor 通过 `${cdn}/dist/js/...` 动态加载运行时资源，桌面端不能依赖 CDN。新增 `vditorOfflineAssets()` 插件：

- dev：`configureServer` 中间件把 `/vditor/dist/*` 从 `node_modules/vditor/dist` 直出；
- build：`closeBundle` 把 `js/lute`、`js/highlight.js`、`js/i18n`、`js/icons`、`css/content-theme` 拷到 `dist/vditor/dist/*`（约 5.7 MB），随前端产物进安装包；
- 编辑器侧只配置 `cdn: '/vditor'`，不新增依赖。

### 3.3 笔记附件解析（src/utils/note-assets.ts）

纯函数，从正文解析资源，不保存额外元数据：

- `noteImageUrl(id, ext, dir)`：按平台生成 `wst-img://localhost` / `http://wst-img.localhost` 地址（与现有编辑器一致）。`dir` 用严格编码：`encodeURIComponent` 不转义 `!'()*`，而圆括号在 Markdown 链接目标里是保留字符，会额外转成 `%28/%29` 等，避免上传目录名带「(备份)」时链接被截断。
- `parseNoteImageUrl(url)`：解出 `id / ext / dir`；非本应用地址返回 null。
- `parseNoteAssets(markdown)`：同时支持 Markdown 图片语法与 `<img>` 标签；同一 URL 去重并记录 `occurrences`；返回 `{ raw, url, id, ext, dir, alt, start, end, occurrences }`。
  - Markdown 图片按括号配对扫描（而不是单个正则）：兼容老正文 URL 里未转义的平衡括号与 `title`（`"…"` / `'…'` / `(…)`）；遇到无法配对（目录名里只有半个括号）或收尾后紧跟 `%` 的残缺 URL 时整条跳过，避免删除/重命名改写坏正文。
  - 跳过 fenced code block（``` / ~~~）与行内代码（`` `…` ``）里的示例图片，代码内容不会被当成附件或被改写。
- `noteAssetDisplayName / noteAssetFileName / noteAssetMarkdown`：展示名（alt 优先，缺省 `资源-<id8>.<ext>`）、解密用文件名（补扩展名）、插入用 Markdown。
- `removeNoteAssetFromMarkdown / renameNoteAssetInMarkdown`：按 URL 与位置精确替换（不再全文正则），删除后收敛多余空行；重命名 Markdown 只改 alt，老式 `<img>` 只替换/补 alt 属性并保留 width/height/style 等。

老笔记正文里图片本来就带 `dir`，因此不需要迁移，打开即生效。已知限制：老正文若把图片写成 `<img>` 标签，Vditor IR 会以 HTML 块形式展示源码而不渲染图片（正文不会丢失，附件栏仍能解析、预览、导出）；新插入的图片统一用 Markdown 语法。

### 3.4 笔记附件 / 大纲侧栏（NoteSidebar.vue）

- props：`markdown`、`defaultUploadDir`；emits：`insert`、`updateMarkdown`、`scrollToHeading`、`collapse`、`log`。
- 附件 tab：
  - 解析结果随 markdown 自动更新；
  - 卡片：缩略图（加载失败占位）、名称、大小（`statAttachment` 懒加载 + 缓存，失败显示「文件缺失」）；
  - 操作：插入 / 预览大图 / 复制 Markdown / 打开（解密到临时文件后系统程序打开）/ 导出（解密另存）/ 重命名（改 alt）/ 删除（确认后移除正文引用 + 删密文 blob，删除失败也移除引用）；
  - 上传资源：选择文件或拖入图片 → 加密上传 → 在光标处插入；
  - 空状态提示。
- 大纲 tab：`parseNoteHeadings` 结果按层级缩进，点击 `scrollToHeading(index)`。
- 浏览器（无 Tauri）环境所有 invoke 失败要降级，不白屏。

### 3.5 工作区与详情页接线

- 新建 `NoteWorkspace.vue`：组合编辑器与侧栏；本地 `markdown` ref 与 `note` prop 同步；编辑器每次输入额外 `emit('change')` 立即同步本地 markdown（不落盘），保证侧栏解析用的是最新正文，重命名/删除不会用 500ms 前的旧内容覆盖刚输入的文字；侧栏 `updateMarkdown` → `editor.setMarkdown` + 立即 `emit('update')`；`insert` → `editor.insertMarkdown`；容器宽度 < 760px 自动收起侧栏，收起后右下角悬浮「附件 N」按钮重新展开。
- `GroupDetail.vue` / `ProjectDetail.vue`：把 `NoteEditor` 替换为 `NoteWorkspace`；`updateNote` 事件改为携带实体 id（`[id, note]`），`App.vue` 的 `handleEntityNote(kind, id, note)` 按 id 落盘。这样切换实体时旧编辑器卸载补交的笔记不会写到新选中项上（原实现按当前 selection 落盘，存在竞态）。

### 3.6 后端（src-tauri）

- 新增 `stat_attachment(blob_dir, id)`：校验 uuid，只读 14 字节 WST1 明文头取原始大小 + 文件 mtime（毫秒），不解密、不落明文。注册进 `generate_handler!`。
- 前端 `src/api/attachments.ts` 增加 `statAttachment`，`src/types/index.ts` 增加 `AttachmentStat`。

## 4. 错误处理

- 图片上传失败：`emit('log','error', ...)` + 编辑器 tip，正文不插入坏链接。
- 资源文件缺失（blob 被删）：缩略图破图 + 「文件缺失」标记；仍允许从正文移除引用。
- 删除密文失败：提示错误，但正文引用照样移除，避免用户被卡住。
- 无 Tauri 环境（浏览器预览）：invoke 全部 catch 降级，只影响打开/导出/大小，不影响编辑与解析。

## 5. 兼容性与风险

- 笔记仍是 Markdown 字符串，配置结构不变；Vditor 在用户编辑保存后会按标准 Markdown 规范化（列表符号、空行等），可接受。
- 自定义协议图片在 IR 模式下的渲染与回写需实测；若 Vditor 改写 URL，退路是：图片地址保持 Markdown 原文（Vditor `getValue()` 以原文为准），必要时在 `renderImage` 钩子里恢复。
- Vditor 运行时资源必须随构建产物打包，否则离线打开编辑器加载不出 lute；已用构建插件兜住并在 build 后检查 `dist/vditor/dist/js/lute/lute.min.js`。
- 侧栏资源由正文派生：正文删掉资源后侧栏即消失；密文 blob 可能残留为孤儿文件（本次不清理，和上传页附件行为一致）。

## 6. 验收标准

1. `pnpm build`（vue-tsc + vite build）通过；`cargo check` 通过。
2. 笔记页为单栏即时渲染：输入 `# 标题`、列表、加粗、表格、代码块、引用即时变样式；可切源码模式。
3. 工具栏选图 / 粘贴图片 / 拖入图片 → 加密上传 → 正文出现图片、附件栏出现缩略图、名称、大小。
4. 附件栏操作可用：插入、预览、复制、打开、导出、重命名、删除；删除后正文引用与缩略图同步消失。
5. 大纲点击滚动到对应标题。
6. 老笔记（正文里已有 `wst-img` 图片）打开即显示图片与附件卡片，无需迁移。
7. 切换分组 / 项目、关闭 tab 不丢未落盘编辑。
8. 窗口变窄时侧栏自动收起、编辑器不塌；收起后有入口重新展开。
