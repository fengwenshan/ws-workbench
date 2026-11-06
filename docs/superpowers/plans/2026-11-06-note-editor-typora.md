# 笔记编辑器仿 Typora + 笔记附件栏 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把项目 / 分组笔记的编辑器换成 Vditor 即时渲染（仿 Typora），并在笔记右侧加「附件 / 大纲」栏，自动展示正文里引用的加密图标 / 图片资源。

**Architecture:** 笔记仍是 Markdown 字符串。`NoteWorkspace.vue` 组合 `NoteEditor.vue`（Vditor IR + 加密图片上传）与 `NoteSidebar.vue`（正文资源解析 / 大纲）；资源清单从正文的 `wst-img` URL 派生，不新增配置字段。Vditor 运行时资源由 Vite 插件本地化，后端只加一条只读 WST1 头的 `stat_attachment` 命令。

**Tech Stack:** Tauri 2、Vue 3.5、Vite 6、TypeScript、ant-design-vue 4、vditor 4.0.1、Rust。

**设计文档：** `docs/superpowers/specs/2026-11-06-note-editor-typora-design.md`

---

## 文件结构

| 文件 | 职责 |
| --- | --- |
| `src/components/NoteEditor.vue`（重写） | Vditor 挂载、工具栏、图片上传、状态栏、对外方法 |
| `src/components/NoteSidebar.vue`（新建） | 附件 / 大纲侧栏、资源操作 |
| `src/components/NoteWorkspace.vue`（新建） | 编辑器 + 侧栏组合、折叠状态、正文同步 |
| `src/components/GroupDetail.vue`、`ProjectDetail.vue` | 换用 NoteWorkspace |
| `src/utils/note-assets.ts`（新建） | 正文资源解析 / 删除 / 重命名纯函数 |
| `src/utils/note-text.ts`（新建） | 标题解析、字数统计 |
| `src/api/attachments.ts` | `statAttachment` |
| `src/types/index.ts` | `AttachmentStat` |
| `vite.config.ts` | Vditor 离线资源插件 |
| `src-tauri/src/commands/attachments.rs`、`lib.rs` | `stat_attachment` 命令与注册 |
| `README.md` | 笔记与附件章节更新 |

---

## Task 1：基础设施（Lead）

**Files:**
- Modify: `package.json`、`pnpm-lock.yaml`（`pnpm add vditor@4.0.1`、`pnpm remove md-editor-v3`）
- Modify: `vite.config.ts`
- Create: `src/utils/note-assets.ts`、`src/utils/note-text.ts`
- Modify: `src/types/index.ts`、`src/api/attachments.ts`
- Modify: `src-tauri/src/commands/attachments.rs`、`src-tauri/src/lib.rs`

- [x] **Step 1: 依赖** — `pnpm add vditor@4.0.1`、`pnpm remove md-editor-v3`（本机 pnpm store 在仓库内 `.pnpm-store`，需 `--store-dir` 或先 `pnpm install` 对齐）。
- [x] **Step 2: Vite 离线资源插件** — `vditorOfflineAssets()`：dev 中间件直出 `/vditor/dist/*`，build 拷贝 `js/lute`、`js/highlight.js`、`js/i18n`、`js/icons`、`css/content-theme` 到 `dist/vditor/dist/*`。
- [x] **Step 3: 纯函数工具** — `note-assets.ts` 提供 `noteImageUrl / parseNoteImageUrl / parseNoteAssets / noteAssetDisplayName / noteAssetFileName / noteAssetMarkdown / removeNoteAssetFromMarkdown / renameNoteAssetInMarkdown`；`note-text.ts` 提供 `parseNoteHeadings / countNoteWords / noteStats`。
- [x] **Step 4: 后端 stat** — `AttachmentStat { size, modified_at }` + `stat_attachment(blob_dir, id)`（校验 uuid，读 WST1 明文头），注册进 `generate_handler!`；前端加 `statAttachment` 与类型。
- [x] **Step 5: 验证** — `cargo check --manifest-path src-tauri/Cargo.toml --no-default-features` 通过。

## Task 2：NoteEditor.vue 换 Vditor（editor-vditor）

**Files:**
- Modify: `src/components/NoteEditor.vue`

- [x] **Step 1: 初始化** — `import Vditor from 'vditor'`、`import 'vditor/dist/index.css'`；`new Vditor(el, { mode:'ir', height:'100%', minHeight:200, cdn:'/vditor', theme:'classic', lang:'zh_CN', icon:'material', cache:{enable:false}, outline:{enable:false}, counter:{enable:false}, toolbar:[...], toolbarConfig:{pin:true}, upload:{...}, input, after })`。
- [x] **Step 2: 图片上传** — `upload: { accept:'image/*', multiple:true, handler: async (files) => { const dir = await resolveUploadDir(props.defaultUploadDir); const urls = []; for (const file of files) { const meta = await encryptUploadBytes(dir, name, ext, b64); urls.push(noteImageUrl(meta.id, meta.ext, meta.dir)) } vditor.insertValue(urls.map(u => `![](${u})`).join('\n')) } }`；失败 emit log。
- [x] **Step 3: 保存** — 500ms 防抖 emit update；`watch(() => props.note)` 值不同时 `setValue(next, true)`；卸载前补交；`destroy()`。
- [x] **Step 4: 对外方法** — `insertMarkdown`、`setMarkdown`（静默）、`focus`、`scrollToHeading(index)`（`.vditor-ir` 下 h1-h6 按序 scrollIntoView）。
- [x] **Step 5: 状态栏 + 主题样式** — 字数 / 保存状态；CSS 变量覆盖 Vditor。
- [x] **Step 6: 验证** — `pnpm exec vue-tsc --noEmit`、`pnpm build`、浏览器 harness 验证 IR / 源码切换 / URL 不被改写。

## Task 3：NoteSidebar.vue（note-sidebar）

**Files:**
- Create: `src/components/NoteSidebar.vue`

- [x] **Step 1: 骨架** — props `{ markdown, defaultUploadDir }`，emits `insert / updateMarkdown / scrollToHeading / collapse / log`；tabs 附件 / 大纲 + 收起按钮。
- [x] **Step 2: 附件解析与卡片** — `parseNoteAssets`；缩略图 + 名称 + `statAttachment` 懒加载缓存（失败「文件缺失」）。
- [x] **Step 3: 资源操作** — 插入、预览 Modal、复制 Markdown、打开（decryptToTemp + openFile）、导出（pickSavePath + decryptExportFile）、重命名（renameNoteAssetInMarkdown）、删除（confirm → removeAttachmentFile + removeNoteAssetFromMarkdown）。
- [x] **Step 4: 上传** — 「上传资源」pickFiles + 拖拽，`encryptUploadFile` / `encryptUploadBytes` → emit insert。
- [x] **Step 5: 大纲** — `parseNoteHeadings`，按 level 缩进，点击 emit scrollToHeading。
- [x] **Step 6: 验证** — vue-tsc + 浏览器 harness（无 Tauri 时降级不崩）。

## Task 4：工作区接线 + 文档（Lead）

**Files:**
- Create: `src/components/NoteWorkspace.vue`
- Modify: `src/components/GroupDetail.vue`、`src/components/ProjectDetail.vue`、`README.md`

- [x] **Step 1: NoteWorkspace** — 本地 markdown 与 note prop 同步；`update` 上抛；侧栏 `updateMarkdown` → `setMarkdown` + 立即上抛；`insert` → `insertMarkdown`；`scrollToHeading` 透传；宽度 < 760 自动收起；收起后悬浮「附件 N」入口。
- [x] **Step 2: 详情页替换** — `NoteEditor` → `NoteWorkspace`，props/emits 不变。
- [x] **Step 3: README** — 更新「笔记与附件」章节（Vditor、离线资源、附件栏、stat_attachment）。
- [x] **Step 4: 验证 + 提交** — `pnpm build`、`cargo check`、浏览器/真实应用截图；按仓库风格提交（`feat(note): ...`、`docs(note): ...`）。

## Task 5：独立验证（verifier）

- [x] 代码审查（契约一致性、URL 不被改写、防抖保存、侧栏同步、无 Tauri 降级）。
- [x] `pnpm build` + `cargo check`。
- [x] `pnpm dev` + 浏览器 harness 功能验证（附件解析 / 大纲 / tabs / 重命名删除纯函数 / 窄宽度布局）。
- [x] 尝试 `pnpm tauri dev` 真实应用截图（本机 screencapture 被 TCC 拒绝，改为启动日志检查，无 panic）。
- [x] 输出验证报告与修复建议（不改业务代码）。

## Task 6：验证问题修复（Lead）

独立验证报告见 `.tmp/verify/REPORT.md`（未纳入版本库）。修复了 6 个发现：

- [x] **F1 目录含括号导致 URL 截断** — `noteImageUrl` 对 `!'()*` 严格编码；`parseNoteAssets` 改为括号配对扫描，兼容老正文里未转义的平衡括号，无法配对的残缺 URL 整条跳过（不再截断后删除/改写正文）。
- [x] **F2 代码块内图片被当成附件** — `note-assets.ts` 增加 fenced code range 过滤；解析与重命名/删除都不会碰代码块里的示例图。
- [x] **F3 老 `<img>` 正文在 IR 里不渲染** — 已知限制，写入设计文档（正文不丢、附件栏仍可用）；新内容统一用 Markdown 图片语法。
- [x] **F4 `<img>` 重命名丢属性** — 只替换/补 `alt`，保留 width/height/style 等属性。
- [x] **F5 blockquote 标题导致大纲索引错位** — `parseNoteHeadings` 识别 `> ## 标题`，与 IR DOM 的 h 标签顺序一致。
- [x] **F6 防抖窗口内改名/删除用旧正文** — `NoteEditor` 新增 `change` 事件即时回传最新正文，`NoteWorkspace` 用它同步侧栏解析数据（落盘仍走 500ms 防抖）。

复测：`src/utils` 边界用例（`.tmp/lead-check/check.ts`）、`pnpm build`、verifier 的 harness 复测。

第二轮复测又发现 R1（带 title 的图片 `![a](url "t")` 不解析）及 CRLF/行内代码两个小问题，已修：

- [x] **R1 title 形式的图片** — URL 扫描增加「顶层空白正常收尾」标记，`"…"` / `'…'` / `(…)` 三种 title 与 `<url> "t"` 均可解析，删除/重命名正确。
- [x] **CRLF 代码块偏移** — fence 区间按 `\n` 切分保留 `\r`，偏移与原文一致。
- [x] **行内代码里的示例图片** — 新增 inline code 区间过滤，不计数、不被删除/重命名改写。

最终验证：verifier 三轮报告均为通过（`.tmp/verify/REPORT.md`、`REPORT-round2.md`、`REPORT-round3.md`）；真实 Tauri 窗口截图受本机 TCC 限制未覆盖，图片上传/自定义协议渲染建议在桌面端点一次确认。
