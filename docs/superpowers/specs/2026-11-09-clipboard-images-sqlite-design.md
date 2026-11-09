# 剪贴板历史 v2：图文 + SQLite 加密存储设计

- 日期：2026-11-09
- 分支：feat/uninstaller
- 状态：设计已确认，待实现
- 关联：取代 `2026-11-07-clipboard-and-hosts-design.md` 中的剪贴板部分（那份文档的 Hosts 部分继续有效）；复用现有 SQLite / `crypto` 信封 / `filecrypto` WST1 / `db.rs` 迁移机制，不改项目、打包、笔记、VPN 逻辑

## 1. 背景与目标

### 1.1 现状

- 剪贴板历史目前是**纯内存**：`src-tauri/src/clipboard.rs` 后台线程每 600ms 用官方插件 `read_text` 读文本，最多 20 条，进程退出即清空
- 只支持纯文本，图片 / HTML 富文本一律丢失
- 面板是独立窗口 `clipboard-panel`（380 × 480，无边框透明，贴近鼠标位置），`⌘⇧V` 切换显示/隐藏
- 本地已有可复用的加密与存储设施：
  - `db.rs`：SQLite（WAL + busy_timeout），业务实体 payload 用密文信封存储，`SCHEMA_VERSION = 1`
  - `crypto.rs`：SM4-CBC + HMAC-SM3 信封（`v1.` 前缀），主密钥由 keychain / DPAPI 托管
  - `filecrypto.rs`：`WST1` 流式加密容器，提供 `encrypt_bytes` / `decrypt_bytes`
  - `attachments`：笔记附件的加密上传目录约定，可参考但**不复用目录**

### 1.2 本次需求（用户确认）

1. 剪贴板支持图片，以及「文字中携带图片」的图文内容；回贴尽量还原图文
2. 历史落 **SQLite**，内容在数据库里加密
3. 加密图片附件放在单独目录，用 SQLite 索引记录，便于统一删除
4. 在 Hosts 页签后面新增一个「剪贴板」管理页签
5. 不设最大历史条数，面板/管理页**滚动懒加载**
6. 删除 / 清空历史时，级联删除本地加密图片附件
7. 快捷键 `⌘⇧V` 重复按（怕没复制上时连按）不能把面板关掉；重复复制同一内容不刷重复记录

### 1.3 目标（In Scope）

- macOS 采集：文本、HTML、图片；HTML 内嵌 `data:` 图片抽出加密为附件
- SQLite 持久化：正文 / HTML 加密，图片 WST1 加密，元数据明文
- 面板：分页懒加载、缩略图、后端加密搜索、图文条目展示、回贴（文本 / HTML / 图片）
- 管理页：历史列表（懒加载）+ 搜索 + 单条复制 / 删除 + 清空全部 + 图片预览 / 导出
- 删除级联：单条 / 清空都删除对应 `clipboard-attachments/*.wst`
- 快捷键：打开面板前强制补采一次剪贴板；300ms 去抖；已显示时再按不收起

### 1.4 非目标（Out of Scope）

- Finder / 桌面复制的图片文件（`file://` 文件列表）不收录
- HTML 里的远程 `http(s)` 图片不下载、不缓存，回贴时保留原 URL
- 富文本回贴只做 best-effort，跨应用不保证样式 / 图片完全还原
- Windows：不采集、不注册热键、不建面板；管理页显示「仅 macOS 启用」提示
- 不做最大条数设置、不做「暂停记录」开关、不建明文全文索引
- 不与笔记附件目录混用；不做 orphan 清理入口（只在删除时尽力清理）
- 不自动模拟 ⌘V（沿用现有选择，不申请辅助功能授权）

## 2. 架构总览

```
macOS 系统剪贴板
   │  NSPasteboard.changeCount（600ms 轮询 / 面板唤出时强制检查）
   ▼
采集线程（clipboard.rs）
   │  arboard 读 text / html / image
   ▼
解析与加密（clipboard/rich.rs）
   │  HTML <img> 抽取、data URI 解码、缩略图、内容哈希、限流
   ▼
存储（clipboard/store.rs）
   ├─ SQLite：clipboard_items（正文/HTML 为密文信封）
   └─ <app_data>/clipboard-attachments/<id>.wst / <id>.thumb.wst（WST1）
   │
   ├─ 面板 clipboard-panel（⌘⇧V，懒加载 + 搜索 + 回贴）
   ├─ wsclip:// 协议（缩略图 / 大图解密流，按附件 id 查库）
   └─ 管理页「剪贴板」（系统页第 5 个页签）
```

模块划分：

| 文件 | 职责 |
| --- | --- |
| `src-tauri/src/clipboard.rs` | 运行时状态、轮询线程、面板显隐 / 定位、事件、快捷键行为 |
| `src-tauri/src/clipboard/rich.rs` | HTML `<img>` 抽取与还原、data URI 解码、内容哈希、缩略图、上限常量 |
| `src-tauri/src/clipboard/store.rs` | SQLite 读写、加密/解密、附件目录、分页、搜索、删除级联 |
| `src-tauri/src/commands/clipboard.rs` | Tauri 命令薄层（参数校验 / 错误翻译 / `spawn_blocking`） |
| `src-tauri/src/db.rs` | `SCHEMA_VERSION` 1 → 2，新增两张表 + 迁移 |
| `src-tauri/src/lib.rs` | 注册 `wsclip` 协议、初始化采集线程、快捷键 handler 行为调整 |
| `src/clipboard/ClipboardPanel.vue` | 面板 UI 重写 |
| `src/components/clipboard/ClipboardManager.vue` | 管理页（新） |
| `src/composables/useClipboardHistory.ts` | 面板 / 管理页共享的分页、搜索、复制、删除状态 |
| `src/api/clipboard.ts`、`src/types/clipboard.ts` | 命令封装与类型 |
| `src/components/cleaner/CleanerPanel.vue` | 系统页新增「剪贴板」页签 |

## 3. 采集层

### 3.1 变更检测

- 轮询线程每 **600ms** 读一次 `NSPasteboard.changeCount`（`objc2-app-kit` 开 `NSPasteboard` feature；只在 macOS 编译）
- `changeCount` 未变 → 什么都不读，开销接近零
- 变了 → 读一次内容（`arboard`）：`get().text()`、`get().html()`、`get().image()`
- 自己写回剪贴板后，把「已见 changeCount」同步为新值，避免把自己写的内容再收一遍
- 面板唤出时（快捷键）额外触发一次立即检查（异步线程），解决「怕没复制上、打开看到旧内容」
- 面板/管理页收到 `clipboard-changed` 事件后刷新首屏

### 3.2 内容判定

一次剪贴板快照按以下优先级判定：

1. HTML 非空且含 `<img>` → `rich`（存文本 + HTML + 图片附件）
2. 否则文本非空 → `text`（沿用 `classify`：`url` / `code` / `text` 作为列表徽标）
3. 否则图片非空 → `image`（截图 / 复制图片，存 PNG）
4. 都为空 → 跳过

说明：

- 无图片的 HTML 富文本按纯文本记录（不保存格式），这是已确认的取舍
- 文本 + 图片同时存在但 HTML 为空（少见）→ 按 `image` 记录，文本作为空预览忽略

### 3.3 图文中的图片处理

- 扫描 HTML 中的 `<img ... src=...>`（单引号 / 双引号 / 无引号都认，容忍常见畸形写法）
- `src` 是 `data:image/...;base64,...`：
  - 解出 mime 与字节，超过单图上限则跳过该图（`src` 保持原样）
  - 字节走 WST1 加密落盘，得到 `attachment_id`
  - 在**存储的 HTML** 里把 `src` 替换为内部占位符 `wsclip-img://<attachment_id>`
- `src` 是 `http(s)` / `file://` / 相对路径：原样保留，不访问网络、不落盘
- 回贴时再把占位符还原成 `data:<mime>;base64,...`（见 5.3）

### 3.4 缩略图

- 每个图片附件生成一张最长边 **320px** 的 PNG 缩略图，同样 WST1 加密落盘
- 面板 / 管理页列表只请求缩略图；大图预览才解密原图
- 解码 / 缩放用 `image` crate（`png` / `jpeg` / `gif` / `webp`），解码失败时跳过该附件（不阻塞整条记录）

### 3.5 上限与去重

| 常量 | 值 | 说明 |
| --- | --- | --- |
| `MAX_TEXT_CHARS` | 200_000 | 超出不收录（沿用） |
| `MAX_HTML_BYTES` | 2 MiB | 超出则不存 HTML，按文本记录 |
| `MAX_IMAGE_BYTES` | 10 MiB | 单图上限，超出跳过该图 |
| `MAX_IMAGES_PER_ITEM` | 50 | 单条最多附件数 |
| `MAX_ITEM_BYTES` | 24 MiB | 单条（文本 + HTML + 原图）总量 |
| `THUMB_MAX_EDGE` | 320 | 缩略图最长边 |
| `PAGE_SIZE` | 20 | 分页大小 |
| `SEARCH_LIMIT` | 100 | 搜索结果上限 |
| `HOTKEY_DEBOUNCE_MS` | 300 | 快捷键去抖 |

- 去重：`content_hash = SM3("v1\n" + text + "\n" + html + "\n" + 按顺序拼接的每张图片 SHA/SM3 十六进制)`
- `content_hash` 明文列 + 索引；命中已有记录 → 只更新 `created_at`（置顶），不重复写附件
- 因此「怕没复制上按多次 ⌘C」不会产生重复历史

### 3.6 错误与隐私

- 加密 / 写盘 / DB 失败只记录错误分类（如 `capture: encrypt failed`），**绝不打印剪贴板内容**
- 采集写盘顺序：先加密写 blob（`.tmp` → 原子改名）→ 再写 DB；DB 失败回滚删除刚写的 blob
- 采集线程串行执行，面板唤出触发的「立即检查」用同一把锁，避免并发重复写入

## 4. 存储层

### 4.1 SQLite 表（`SCHEMA_VERSION = 2`）

```sql
CREATE TABLE IF NOT EXISTS clipboard_items (
  id           TEXT PRIMARY KEY NOT NULL,
  kind         TEXT NOT NULL,              -- text | url | code | image | rich
  created_at   INTEGER NOT NULL,           -- unix ms
  content_hash TEXT NOT NULL,              -- SM3 十六进制，用于去重
  text_enc     TEXT NOT NULL,              -- v1. 密文信封；无文本为 ''
  html_enc     TEXT NOT NULL,              -- v1. 密文信封；非图文为 ''
  image_count  INTEGER NOT NULL DEFAULT 0,
  byte_size    INTEGER NOT NULL DEFAULT 0  -- 文本 + HTML + 原图字节数
);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_created
  ON clipboard_items(created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_hash
  ON clipboard_items(content_hash);

CREATE TABLE IF NOT EXISTS clipboard_attachments (
  id         TEXT PRIMARY KEY NOT NULL,   -- 也是 blob 文件名
  item_id    TEXT NOT NULL,
  seq        INTEGER NOT NULL,            -- HTML 中出现顺序，从 0 开始
  mime       TEXT NOT NULL,               -- image/png 等
  width      INTEGER NOT NULL,
  height     INTEGER NOT NULL,
  orig_size  INTEGER NOT NULL,
  thumb_size INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_clipboard_attachments_item
  ON clipboard_attachments(item_id, seq);
```

- 明文列只有 id / kind / 时间 / 哈希 / 尺寸等元数据；**正文与 HTML 一律密文信封，图片一律 WST1**
- 迁移：`init_schema` 读到 `user_version = 1` 时执行上述 `CREATE TABLE` / `CREATE INDEX`，再把版本写为 2；新库直接建全量结构
- 升级前内存里的 20 条历史不落盘，不迁移

### 4.2 图片附件目录

- `<app_data_dir>/clipboard-attachments/`
  - `<attachment_id>.wst`：原图密文
  - `<attachment_id>.thumb.wst`：缩略图密文
- 目录按需创建；与笔记附件目录完全隔离（笔记附件目录可能被用户配置到外部路径，剪贴板始终用 App 私有目录）
- 文件名只用 uuid，路径永远由 DB 里的 id 推导，不接受前端传路径

### 4.3 分页与搜索

- 分页游标：`(created_at, id)` 倒序；请求带上一页最后一条的 `before_created_at` + `before_id`，返回 `items` + `has_more`
- 每页返回**摘要**：`id`、`kind`、`created_at`、`text_preview`（解密后截前 240 字符）、`image_count`、`byte_size`、`images[]`（附件元数据：id / mime / width / height / orig_size）
- 完整正文只在「复制」命令里读取，不下发前端
- `limit` 参数夹到 1–50，默认 20；`has_more` 由多取一条判断，避免额外 `COUNT`
- 搜索：`clipboard_search(query)` 按 `created_at DESC` 逐条解密，匹配文本 + HTML 去标签后的文字（大小写不敏感），最多返回 100 条；前端 250ms 防抖
- 统计：`clipboard_stats()` 返回总条数、附件数、占用字节数（管理页工具栏 / 面板计数）

### 4.4 命令清单

| 命令 | 说明 |
| --- | --- |
| `clipboard_page(before_created_at?, before_id?, limit?)` | 分页摘要，默认 20 条 |
| `clipboard_search(query, limit?)` | 解密搜索，默认上限 100 |
| `clipboard_stats()` | `{ items, images, bytes }` |
| `clipboard_copy(id)` | 按 kind 写回剪贴板（不再自动隐藏窗口，由面板前端调用隐藏） |
| `clipboard_copy_image(attachment_id)` | 复制单张图片到剪贴板（管理页图片弹层用） |
| `clipboard_delete(id)` | 单条删除 + 级联删附件，返回删除的图片数 |
| `clipboard_clear()` | 清空 + 级联删附件，返回条数与图片数 |
| `clipboard_export_image(attachment_id, target_path)` | 解密导出到用户选择的路径（`.tmp` + 改名，不留明文临时文件） |
| `clipboard_limits()` | 保留快捷键徽标与文本上限（`max_items` 移除，计数改用 stats） |
| `clipboard_panel_hide()` | 保留 |

- 命令走 `spawn_blocking`，每次操作独立打开连接（沿用 `db::open`：WAL + busy_timeout 2s）
- 参数校验：id 必须是数据库存在的行；`target_path` 非空且不以目录结尾；导出失败时清理 `.tmp`
- `clipboard_copy` 只写系统剪贴板、不隐藏窗口；面板复制后自己调 `clipboard_panel_hide`，管理页复制不动面板

### 4.5 删除级联

- 单条删除：
  1. 事务内删 `clipboard_attachments` + `clipboard_items`
  2. 提交后删对应 `<id>.wst`、`<id>.thumb.wst`；文件缺失忽略
- 清空全部：
  1. 事务内 `DELETE FROM clipboard_attachments` / `clipboard_items`
  2. 提交后清理 `clipboard-attachments/` 下所有 `*.wst`（含 `.thumb.wst`）
- 先 DB 后文件：崩溃最坏留下加密孤儿 blob（不泄密）；只删本功能命名模式的文件，不碰目录里其它内容

## 5. 回贴

### 5.1 纯文本

- 读 `text_enc` 解密 → 插件 `write_text`

### 5.2 图片

- 读附件原图 blob → `filecrypto::decrypt_bytes` → `image` 解码成 RGBA → 插件 `write_image`
- 解码失败返回可读错误（面板 toast / 管理页 message），不静默

### 5.3 图文（rich）

1. 解密 HTML
2. 把 `wsclip-img://<attachment_id>` 替换为 `data:<mime>;base64,<解密后的原图>`；找不到附件时移除该 `<img>` 标签
3. `write_html(html, alt=解密后的纯文本)`
4. HTML 写失败 → 降级 `write_text(纯文本)`；纯文本也为空时退化为复制第一张图片
5. 写成功后同步采集线程的 `changeCount`，并把记录 `created_at` 置顶

### 5.4 面板内交互

- `Enter` / 单击：复制当前选中项 → 收起面板
- `Esc`：有搜索词时先清搜索，否则收起
- 面板不提供删除（删除统一在管理页，避免误删）

## 6. 面板 UI（`clipboard-panel`）

### 6.1 快捷键行为（用户特别要求）

- `⌘⇧V` **只负责打开 / 聚焦**，不再切换隐藏：
  - 面板未显示 → 立即显示面板并聚焦搜索框，同时异步补采一次剪贴板；采集完成发 `clipboard-changed`，面板刷新首屏（不阻塞打开）
  - 面板已显示 → 拉到最前、聚焦搜索框、刷新首屏（不收起）
- 300ms 去抖：连按多次只处理第一次，避免闪烁
- 收起：`Esc`、点击外部失焦、选中复制后自动收起
- 文档里「再按一次 ⌘⇧V 收起」的旧描述一并改掉

### 6.2 列表与懒加载

- 首屏 20 条，滚动到底部哨兵触发 `clipboard_page` 下一页，到底显示「没有更多了」
- 行内容：类型徽标（文本 / 链接 / 代码 / 图片 / 图文）+ 两行文字预览 + 最多 3 张 56–64px 缩略图 + 相对时间；纯图片条目用首图作封面
- 缩略图用 `wsclip://localhost/img?id=<attachment_id>&variant=thumb`（Windows 为 `http://wsclip.localhost/img?...`，见第 7 节），`loading="lazy"`，加载失败显示占位图标
- 顶部显示总条数（`clipboard_stats`）；窗口尺寸保持 380 × 480

### 6.3 搜索与键盘

- 搜索框 250ms 防抖调用 `clipboard_search`；结果替换列表并显示「共 N 条结果」；清空搜索恢复分页列表
- `↑↓` 选择、`Enter` 复制、`Esc` 清搜索 / 收起
- 选中项到达已加载底部时自动加载下一页，保持键盘导航连续

## 7. `wsclip` 自定义协议

- 在 `lib.rs` 注册异步协议（同 `wst-img` 的写法），URL 形态（与笔记 `wst-img` 的 `isWindows()` 前缀约定一致）：
  - macOS：`wsclip://localhost/img?id=<attachment_id>&variant=thumb|full`
  - Windows：`http://wsclip.localhost/img?id=<attachment_id>&variant=...`
- 处理流程：解析 id → 查 `clipboard_attachments`（必须存在）→ 由 id 推导 blob 路径（`thumb` 用 `.thumb.wst`，`full` 用 `.wst`）→ `decrypt_bytes` → 带正确 `Content-Type` 返回，`Cache-Control: no-store`
- 安全：**不接受目录 / 文件名参数**，只认数据库里的附件 id，天然无目录穿越；解密失败返回 500，缺失返回 404

## 8. 管理页（系统页第 5 个页签「剪贴板」）

- `CleanerPanel.vue` 页签顺序：应用 / 弹窗 / 屏幕休眠 / Hosts / **剪贴板**；`TabKey` 增加 `clipboard`
- 组件 `src/components/clipboard/ClipboardManager.vue`，共用 `useClipboardHistory`
- 布局：
  - 工具栏：搜索框、`共 N 条 · M 张图片 · X MB`、右侧「清空历史」危险按钮（空时禁用）
  - 列表：同面板摘要信息，行操作 `复制` / `删除` /（有图时）`查看图片`，时间显示绝对时间
  - 滚动懒加载（IntersectionObserver 哨兵），空状态 / 加载骨架屏 / 搜索空结果
- 图片弹层：多图切换、缩略图条、`复制图片`、`导出`（`dialog:allow-save` 已有权限，选路径后调 `clipboard_export_image`，默认文件名 `clipboard-<yyyymmdd-hhmmss>-<seq>.<ext>`；目标已存在时由保存对话框确认覆盖）
- 删除：单条即时删除 + toast「已删除（含 N 张图片附件）」；清空全部弹确认框「将删除全部 N 条历史及 M 张图片附件，不可恢复」
- Windows：页签保留，内容替换为说明「剪贴板历史仅在 macOS 启用；Windows 请使用系统 Win+V」

## 9. 依赖、平台与常量

- 新增依赖（macOS target 下）：
  - `arboard = "3.7"`（默认特性含 `image-data`）：读 text / html / image
  - `image = { version = "0.25", default-features = false, features = ["png", "jpeg", "gif", "webp"] }`：缩略图、图片解码
  - `objc2-app-kit` features 增加 `NSPasteboard`：只用于 `changeCount`
- 写回继续用现有 `tauri-plugin-clipboard-manager`（`write_text` / `write_html` / `write_image`）
- 快捷键 accelerator 仍是 `Command+Shift+V`（`HOTKEY_ACCELERATOR`），不变
- 事件：沿用 `clipboard-changed`、`clipboard-panel-shown`，新增字段含义不变（前端刷新用）
- 移除：`MAX_ITEMS`、`ClipboardHistory` 内存队列、`push / snapshot / clear` 等内存实现

## 10. 测试计划

### 10.1 Rust 单测

- `rich.rs`
  - HTML `<img>` 提取：双引号 / 单引号 / 无引号 / data URI / 远程 URL / `file://` / 畸形标签
  - data URI 解析（base64、mime、缺失 payload）
  - 存储 HTML 占位符替换 & 回贴还原（含附件缺失时移除 `<img>`）
  - 去标签搜索文本、内容哈希稳定性、各项上限判定
- `store.rs`（临时目录 + 临时 DB）
  - v1 → v2 迁移：用户版本、两张表、索引存在
  - 插入 / 去重置顶 / 分页游标顺序 / 搜索命中
  - 单条删除 + 清空的级联文件删除（缺失文件不报错）
  - 附件查询、导出路径校验
- `db.rs` 既有测试保持通过
- 解码 / 缩略图：用内存生成的小 PNG 断言缩略图尺寸不超过 320

### 10.2 前端

- `pnpm build`（`vue-tsc --noEmit && vite build`）通过
- 新增类型 / API 与 Rust 命令签名一致

### 10.3 手动验收清单（真实剪贴板 / GUI 受 macOS 权限限制，无法自动化）

1. 启动 dev：复制文本 → 面板出现、计数 +1；再复制同样文本 → 仍是一条（去重置顶）
2. 截图到剪贴板（`⌃⇧⌘4`）→ 面板出现图片条目和缩略图；点选后可粘贴到其它应用
3. 浏览器复制带图文的选区 → 面板出现「图文」条目和缩略图；回贴到富文本编辑器尽量还原
4. 连按 `⌘⇧V` 3 次 → 面板保持打开、搜索框聚焦
5. 搜索关键词命中老记录；滚动加载超过 20 条
6. 管理页：单条删除后 `clipboard-attachments` 对应 `.wst` 消失；清空后目录为空、列表空状态
7. 重启应用 → 历史仍在（SQLite 持久化）
8. Windows：仅编译检查 + 页面提示（当前环境无法交叉编译验证运行）

## 11. 风险与取舍

- 富文本回贴跨应用保真度有限；远程图片离线不可用（已与用户确认）
- `⌘⇧V` 与 Chrome / 编辑器「粘贴为纯文本」冲突（应用运行期间本应用接管），保留现有提醒
- 不设上限 → 数据库 / 附件目录可能长期增长，靠手动「清空历史」控制；搜索是全量解密扫描，历史很大时首搜变慢（上限 100 条 + 防抖缓解）
- 崩溃可能留下加密孤儿 blob；不泄密，后续可加清理入口
- 图片解码 / 缩略图增加采集耗时；超限图片跳过，不阻塞文本入库
- `image` crate 增加编译时间；启用特性已收敛到 png / jpeg / gif / webp

## 12. 验收标准

- [ ] 文本 / 图片 / 图文三类内容都能采集、入库（SQLite）、在面板与管理页展示
- [ ] 数据库里的正文与 HTML 为密文信封，图片为 WST1 密文；磁盘无明文副本
- [ ] 面板 / 管理页滚动懒加载、搜索可用
- [ ] 回贴支持文本、图片、图文（best-effort）
- [ ] 单条删除 / 清空全部会删除对应加密附件文件
- [ ] 连按快捷键面板不收起，唤出前后能看到最新复制内容
- [ ] 重启后历史仍存在
- [ ] `cargo test`、`pnpm build` 通过；README / 文档同步更新
