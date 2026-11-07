# 笔记编辑器升级：md-editor-v3 + 图片加密内联 设计文档

- 日期：2026-09-30
- 分支：feat/vpn
- 状态：待评审
- 关联：`2026-09-30-group-project-notes-and-encrypted-file-upload-design.md`（加密笔记与加密附件上传，本特性复用其存储与加解密链路）、`2026-09-29-local-storage-encryption-design.md`（本地存储加密）

## 1. 背景与目标

现有笔记编辑器 `src/components/NoteEditor.vue` 只有 6 个手写按钮（粗体/斜体/标题/列表/引用/链接）+ ant-design-vue `Textarea` + `marked` 渲染的左右分栏预览，"太捞了"。用户希望参考掘金创作页的形态，或采用市面上成熟的 Markdown 编辑器方案。

已完成的加密附件链路（`WST1` 流式加密、`AttachmentMeta`、上传/导出/预览/删除命令、上传页签）为本特性提供了直接可复用的存储与加解密基础。

### 目标（In Scope）

1. 用成熟开源库 `md-editor-v3` 替换现有编辑器：Markdown 源码 + 实时预览、专业工具栏。
2. 目录（TOC）大纲导航。
3. GFM 扩展语法：表格、任务列表。
4. 代码块语法高亮。
5. 图片支持**粘贴 / 拖拽 / 选择文件**上传：密文落盘（`.wst`），预览内联显示。
6. 图片复用现有附件体系（`AttachmentMeta` + `.wst`），上传页签可见、可下载、可删除。

### 非目标（Out of Scope）

- 不做暗色模式（应用当前仅浅色）。
- 不做所见即所得（WYSIWYG）模式，保持"源码 + 预览"。
- 不改 `note` 数据类型（仍为 `string`），不改配置加密信封边界。
- 不做导出 / 打包可移植 Markdown（图片 URL 为机器本地路径，换机失效）。
- 不重做附件加解密算法，不删除现有上传页签功能。
- 不改动 `ProjectConfig.vue` 业务逻辑。

## 2. 现状与约束

- 技术栈：Tauri 2.x + Vue 3.5 + Vite 6 + TypeScript + ant-design-vue 4.2.6。
- `note`：`ProjectGroup.note` / `ProjectConfig.note`，随整份 `AppConfig` JSON 加密落盘。
- 附件：`AttachmentMeta { id, name, size, ext, created_at, dir }`；密文 blob 路径 = `<dir>/<id>.wst`。
- 现有命令：`resolve_upload_dir`、`encrypt_upload_file`（入参为磁盘 `src_path`）、`decrypt_export_file`、`remove_attachment_file`（`decrypt_to_temp` 已下线，预览改走 `wst-img` 协议）。
- `tauri.conf.json`：`csp: null`（自定义协议图片不会被 CSP 拦截）；`dragDropEnabled: false`（HTML5 拖拽/粘贴事件直达 webview，正是 md-editor-v3 所依赖）。
- 目前**未**注册任何自定义 URI scheme 协议。
- 项目规则：凡写入本地磁盘的数据必须密文（Rust 侧 `crypto`/`filecrypto`，前端侧 `safeStorage`）；每次改完需重启应用验证。

## 3. 选型与依赖

- 新增依赖：`md-editor-v3`（`^7.1.0`，MIT，Vue3 原生，基于 CodeMirror 6）。
- 组件：`MdEditor`（源码 + 预览一体）、内置 `catalog` 目录、`onUploadImg` 上传钩子。
- 配置：`language="zh-CN"`、`theme="light"`、`previewTheme="github"`、`codeTheme="github"`、`showCodeRowNumber`、`noMermaid` / `noKatex` 关闭（本需求不需要，减小体积）。
- 渲染由 md-editor-v3 内部 markdown-it 负责；现有 `marked` + `dompurify` 若确认仅 `NoteEditor.vue` 引用，则移除依赖与引用（实现时用 grep 确认）。

## 4. 后端改造

### 4.1 `src-tauri/src/filecrypto.rs`：抽出核心 + 新增内存加解密

- 把 `encrypt_file_stream` / `decrypt_file_stream` 的读写循环抽成对 `Read` / `Write` 泛型的核心函数，行为与帧格式（`WST1`，SM4-CBC + HMAC-SM3）保持不变；原函数改为薄封装。
- 新增：

```rust
pub fn encrypt_bytes(app: &AppHandle, data: &[u8]) -> Result<Vec<u8>, String>;
pub fn decrypt_bytes(app: &AppHandle, blob: &[u8]) -> Result<Vec<u8>, String>;
```

- 全程内存操作，**避免明文临时文件落盘**，满足"本地磁盘一律密文"规则。
- `decrypt_bytes` 仍校验末尾 HMAC-SM3，失败即报错。

### 4.2 `src-tauri/src/commands.rs`：新增命令 `encrypt_upload_bytes`

```rust
#[tauri::command]
pub fn encrypt_upload_bytes(
    app: AppHandle,
    upload_dir: String,
    name: String,
    ext: String,
    data_b64: String,
) -> Result<AttachmentMeta, String>;
```

流程：

1. `create_dir_all(upload_dir)`，失败返回"路径 + 权限"错误，不留临时文件。
2. base64 解码 `data_b64`。
3. `filecrypto::encrypt_bytes` 得到密文字节。
4. 写 `<upload_dir>/<uuid>.wst.tmp` → `rename` 为 `<uuid>.wst`（原子写）。
5. 返回 `AttachmentMeta { id, name, size: 原始字节数, ext: 小写去点, created_at, dir: upload_dir }`。

源为内存，不读磁盘。

### 4.3 `src-tauri/src/lib.rs`：注册自定义 URI 协议 `wst-img`

- `.register_asynchronous_uri_scheme_protocol("wst-img", handler)`。
- 请求形如：`<prefix>/img?id=<uuid>&ext=<png>&dir=<urlencoded 绝对目录>`。
  - 校验 `id` 为 uuid 形态（防路径穿越）。
  - 读 `<dir>/<id>.wst` → `filecrypto::decrypt_bytes` → 按 `ext` 映射 `Content-Type`，返回字节。
  - blob 缺失 / HMAC 失败 → 返回 404/500，前端显示裂图并在编辑器 `log` 提示。
- **跨平台 URL 前缀差异**：macOS/Linux 为 `wst-img://localhost/img?...`；Windows 为 `http://wst-img.localhost/img?...`。前端用现有 `get_platform_info` 命令判断平台后拼接。
- 安全边界：仅服务 `.wst`（本应用密文）；`id` 白名单校验；单用户本地桌面应用，风险可接受。

## 5. 前端改造

### 5.1 `src/components/NoteEditor.vue` 重写

- 用 `<MdEditor v-model="text" ...>` 替换原 `Textarea` + 手写 toolbar + 手写预览。
- 引入 `md-editor-v3/lib/style.css`；用现有设计令牌（`--border-color` / `--bg-primary` / `--bg-input` / `--text-secondary` / `--primary` / `--bg-tag` 等）覆盖编辑器 CSS 变量，`height: 100%` 撑满页签。
- 工具栏（掘金风格裁剪）：
  `bold italic strikeThrough title | quote unorderedList orderedList task | codeRow code link image table | revoke next | prettier catalog preview`
- 目录：使用内置 `catalog` 工具栏入口（弹出/侧栏大纲）。
- **保留**原有"防抖 500ms `emit('update')` + 卸载前补交"逻辑（切换实体不丢内容）。
- props 扩展：

```ts
const props = defineProps<{
  note: string
  uploadDir: string | null        // 实体各自的上传目录（可空）
  defaultUploadDir: string | null // 全局兜底目录（可空）
}>()
```

- emits 扩展：

```ts
const emit = defineEmits<{
  update: [note: string]
  addAttachment: [meta: AttachmentMeta]
  log: [level: LogLevel, message: string]
}>()
```

- `onUploadImg(files, callback)` 流程：
  1. `resolve_upload_dir`（复用现有命令：实体目录 → 全局默认 → App 私有）。
  2. 每个 `File`：`FileReader` 读成 base64 → `encrypt_upload_bytes` → 得 `meta`。
  3. `emit('addAttachment', meta)` 入附件列表；`emit('log','success', ...)`。
  4. 按 §6 拼 URL，`callback(urls)` 由编辑器插入 `![alt](url)`。

### 5.2 父组件连线

- `src/components/ProjectDetail.vue`（笔记页签内 `<NoteEditor>` 处）：补 `:upload-dir="project.upload_dir"`、`:default-upload-dir="defaultUploadDir"`，接 `@add-attachment` / `@log` 并转发到既有 `emit('addAttachment')` / `emit('log')`。
- `src/components/GroupDetail.vue`：同上，针对分组实体。
- 上层（`App.vue` / 视图容器）已具备 `addAttachment` / `log` 事件处理，无需改动。

## 6. 图片引用格式

Markdown 内图片一律写成：

```
![alt](wst-img://localhost/img?id=<uuid>&ext=png&dir=%2Fabs%2Fpath)
```

- 只存 `uuid` + 目录 + 扩展名；原文件名与图片内容均不出现在笔记明文里（笔记本身还整体加密）。
- 兼容性取舍：URL 含机器本地绝对路径，笔记导出 / 换机后图片失效（本地单机笔记可接受，本文档标注）。
- 若用户在上传页签删除该附件，笔记中对应图片变为裂图（可接受，文档标注）。

## 7. 与现有附件体系的关系

- 笔记图片与上传页签附件是**同一批** `AttachmentMeta` + `.wst`，统一管理（查看 / 下载 / 删除）。
- 新增图片不新开存储位置，直接 push 到 `entity.attachments` 并 `save_config`。
- 删除附件仅删 blob 与 meta，不改动笔记文本（裂图可接受）。

## 8. 异常与边界

- `onUploadImg` 单张失败：该图跳过，`log` 报错，其余图片照常插入。
- 上传目录不可写：报"路径 + 权限"，清理 `.tmp`。
- base64 解码失败 / 空文件：跳过并报错。
- blob 缺失 / HMAC 失败：协议返回错误，前端裂图，`log` 提示"图片数据缺失或损坏"。
- 大文件：base64 传输 + 内存加密，截图量级无压力；超大文件仍建议走上传页签（文档标注）。
- 原子写：`.tmp` → `rename`，失败清理临时文件（沿用现有约定）。

## 9. 改动文件清单

| 文件 | 动作 |
| --- | --- |
| `src-tauri/src/filecrypto.rs` | 改：抽 `Read`/`Write` 核心，加 `encrypt_bytes` / `decrypt_bytes` |
| `src-tauri/src/commands.rs` | 改：新增 `encrypt_upload_bytes` |
| `src-tauri/src/lib.rs` | 改：注册 `wst-img` 协议、挂载新命令 |
| `src/components/NoteEditor.vue` | 重写：md-editor-v3 包装 + 上传钩子 |
| `src/components/ProjectDetail.vue` | 改：连线 props / events |
| `src/components/GroupDetail.vue` | 改：连线 props / events |
| `package.json` | 改：加 `md-editor-v3`；若确认无用则移除 `marked` / `dompurify` |

## 10. 验收标准

1. 笔记页签呈现"工具栏 + 源码 + 右侧实时预览"，含目录大纲入口、代码高亮、表格 / 任务列表可用。
2. 粘贴 / 拖拽 / 选择一张图片 → 磁盘出现新 `<uuid>.wst`（文本工具查看为密文），预览内联显示该图，上传页签同步出现该附件。
3. 重启应用后：笔记内容与内联图片仍在、可正常显示。
4. 上传页签下载该附件，字节与原始图片一致，文件名可还原。
5. 中文界面、浅色主题与现有设计令牌一致，编辑器撑满页签。
6. `pnpm exec vue-tsc --noEmit` 通过；`pnpm exec tauri dev` 正常启动（改完重启验证）。

## 11. 待实现时确认项

- md-editor-v3 具体 props / 事件名（`onUploadImg`、`catalog`、`codeTheme`、`previewTheme`、`noMermaid` / `noKatex`）以安装后的类型定义为准。
- `marked` / `dompurify` 是否还有其他引用（grep 确认后再决定是否移除）。
- `get_platform_info` 返回结构，用于拼接协议 URL 前缀。
