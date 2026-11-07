# 分组与项目：加密笔记与加密附件上传 设计文档

- 日期：2026-09-30
- 分支：feat/vpn
- 状态：待评审
- 关联：`2026-09-29-local-storage-encryption-design.md`（本地存储加密）、`2026-09-29-antd-migration-design.md`（antd 接入）

## 1. 背景与目标

`ws-workbench` 是 Tauri 2 + Vue 3.5 + Vite 6 + TypeScript 桌面应用。现有 AppConfig 已实现「整份 JSON 加密落盘」，并复用同一把机器绑定主密钥。

本特性为**每个分组**和**每个项目**增加：

1. **单条 Markdown 笔记**：本地密文存储，展示时解密。
2. **加密附件上传**：[内容] 与 [文件名] 均密文落盘；预览/下载/导出时解密还原。
3. **可分别设置的上传位置**：每个分组、每个项目在「上传」页签顶部可各自指定附件目录。

### 目标（In Scope）

1. 数据模型扩展：`ProjectGroup` / `ProjectConfig` 各增 `note`、`upload_dir`、`attachments`；`AppConfig` 增全局兜底 `default_upload_dir`。
2. 新增二进制流式加解密通道（`WST1` 容器），复用现有主密钥派生，无需新增密钥存储。
3. 新增 4 个 Tauri 命令：上传加密、导出解密、预览解密到临时文件、删除附件文件。
4. 前端：分组右侧视图 + 项目右侧三页签；笔记编辑器；附件面板（含上传位置设置、进度条）。
5. 进度事件：上传/解密过程中通过 `app.emit` 推送进度。

### 非目标（Out of Scope）

- 不做跨机器解密方案（主密钥机器绑定，与现有配置同命运）。
- 不做多笔记 / 笔记版本历史（仅单条 Markdown）。
- 不做附件的服务端/云端存储与同步。
- 不做暗色模式。
- 不动现有 `ProjectConfig.vue` 的业务逻辑（仅作为页签内嵌内容）。
- 不做 antd 全量迁移（仅接入 + 本特性新组件用 antd）。

## 2. 数据模型

### 2.1 后端（`src-tauri/src/models.rs`）

```rust
pub struct AttachmentMeta {
    pub id: String,          // uuid v4，同时是密文 blob 文件名（不含扩展名）
    pub name: String,        // 原始文件名（含扩展名）
    pub size: u64,           // 原始字节数
    pub ext: String,         // 扩展名（小写，不含点）
    pub created_at: i64,     // 毫秒时间戳
    pub dir: String,         // 上传时的绝对目录（做法 A：blob 路径 = <dir>/<id>.wst）
}

pub struct ProjectGroup {
    // ...现有字段
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub upload_dir: Option<String>,
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
}

pub struct ProjectConfig {
    // ...现有字段
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub upload_dir: Option<String>,
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
}

pub struct AppConfig {
    // ...现有字段
    #[serde(default)]
    pub default_upload_dir: Option<String>,
}
```

- 全部新字段加 `#[serde(default)]`，旧配置反序列化不失败，等价于免迁移。
- 笔记正文、文件名、元数据全部位于 AppConfig 内，随整份配置信封加密落盘。

### 2.2 前端（`src/types/index.ts`）

镜像同名 `AttachmentMeta`、`ProjectGroup`、`ProjectConfig`、`AppConfig` 新字段。

### 2.3 数据流总览

| 数据 | 存哪 | 加密方式 | 通道 |
| --- | --- | --- | --- |
| 笔记 Markdown 正文 | AppConfig 内 | 整份配置信封 | 现有 `save_config` |
| 附件元数据（含文件名） | AppConfig 内 | 整份配置信封 | 现有 `save_config` |
| 附件二进制内容 | 独立 `.wst` blob | 新二进制信封 `WST1` | 新增命令 |

## 3. 加密文件格式（`WST1`）

现有 `crypto::encrypt_str` 是一次性字符串信封（需整段明文入内存、base64 膨胀约 33%），不适合大文件；因此新增流式二进制通道。

### 3.1 容器结构（小端序）

```
┌─ Header（明文，纳入 MAC 覆盖）──────────────┐
│ magic     "WST1"                     4 B  │
│ version   0x01                       1 B  │
│ flags     0x00（保留）               1 B  │
│ orig_size 原始字节数 u64             8 B  │
├─ Body（若干帧，顺序排列）──────────────────┤
│ frame_0, frame_1, ...                     │
├─ Trailer ─────────────────────────────────┤
│ end_mark  0x00000000（plain_len=0）   4 B  │
│ mac       HMAC-SM3(header ‖ frames) 32 B  │
└───────────────────────────────────────────┘

frame :=
│ plain_len 本帧明文长度 u32（>0）      4 B  │
│ iv        随机 IV                   16 B  │
│ cipher    SM4-CBC(PKCS7(plain))     变长  │
```

### 3.2 加解密流程

**加密**：流式读原文，每满 `CHUNK = 1 MiB` 明文 →
1. 生成随机 16B IV；
2. 以 `enc_key[0..16]` 为 SM4 密钥，CBC + PKCS7 加密该块；
3. 写 `plain_len + iv + cipher`，并把这三段喂入增量 `Hmac<Sm3>`；
4. 头部先写并已喂入 HMAC；
5. EOF 写 `end_mark`，再写最终 `mac`。

**解密**：读 header 校验 magic/version → 循环读帧至 `end_mark`，边读边 HMAC、边 CBC 解密写出 → 末尾比对 `mac`；不一致直接报错，不产出。

### 3.3 参数与密钥

- 明文块大小：`1 MiB`。
- **每帧独立随机 IV**（接受「文件大小/块数」这点长度信息；换取可读性与未来可定位单块）。
- 密钥：复用 `vault_keys(app)` 的 `VaultKeys { enc_key, mac_key }`；SM4 key = `enc_key[0..16]`，HMAC key = `mac_key`（全 32B）。**不新增主密钥、不新增存储。**

### 3.4 新增函数（新建 `src-tauri/src/filecrypto.rs`）

```rust
pub fn encrypt_file_stream(app: &AppHandle, src: &Path, dst: &Path) -> Result<(), String>;
pub fn decrypt_file_stream(app: &AppHandle, src: &Path, dst: &Path) -> Result<(), String>;
```

使用 `BufReader/BufWriter` 顺序读写，内存占用 ≈ 常数。

## 4. 目录布局与 `upload_dir` 语义

### 4.1 目录结构

某个 `upload_dir` 下扁平存放 uuid 命名的密文 blob：

```
<upload_dir>/
  ├─ 3f9a1c2e-....wst
  └─ 8b02d7aa-....wst
```

- 扁平 + uuid：防重名、规避中文/非法字符，原文件名只存在于加密配置中。
- 目录首次上传时按需 `create_dir_all`。

### 4.2 解析链（两层兜底）

```
entity.upload_dir                 // 分组/项目各自设置，优先
   ?? global.default_upload_dir   // 全局默认
   ?? app_data_dir/attachments    // 最终兜底：App 私有目录
```

- 「统一指定」= 只设全局一次，全体生效；「单独设置」= 改某实体 `upload_dir`，只影响它。

### 4.3 改目录后的旧文件（做法 A，已选定）

- `upload_dir` 只决定「新文件放哪」。
- `AttachmentMeta.dir` 记录上传时目录，blob 路径 = `dir/<id>.wst`；改 `upload_dir` 后**旧附件仍可预览/下载**，仅不迁移。
- 若用户手动搬动文件夹，旧附件会失联（可接受）。

## 5. 命令接口

### 5.1 原则

沿用现有分工：**前端改配置 → `save_config` 落盘**。只有「碰二进制文件」的动作才新增命令。

- 笔记正文、文件名、`upload_dir`、附件增删改（元数据）：仅动前端状态 + `save_config`。
- 选文件 / 选保存位置 / 选目录：前端用 `@tauri-apps/plugin-dialog`。
- 重命名附件：只改 `meta.name`（blob 名是 uuid）。
- 打开所在文件夹：复用现有目录打开命令。

### 5.2 新增命令（4 个，`src-tauri/src/commands.rs`）

```rust
#[tauri::command]
pub fn encrypt_upload_file(app: AppHandle, src_path: String, upload_dir: String, name: String)
    -> Result<AttachmentMeta, String>;

#[tauri::command]
pub fn decrypt_export_file(app: AppHandle, blob_dir: String, id: String, target_path: String)
    -> Result<(), String>;

#[tauri::command]
pub fn decrypt_to_temp(app: AppHandle, blob_dir: String, id: String, name: String)
    -> Result<String, String>;

#[tauri::command]
pub fn remove_attachment_file(blob_dir: String, id: String) -> Result<(), String>;
```

实现要点：
- ① `create_dir_all` → `Uuid::new_v4()` → `encrypt_file_stream` 写 `<upload_dir>/<id>.wst.tmp` → `rename` 为 `<id>.wst` → 组装 `AttachmentMeta`。
- ② 直接 `decrypt_file_stream(blob, target)`。
- ③ 用 `tempfile` 建带原扩展名的临时文件，解密写入，返回绝对路径。
- ④ `fs::remove_file`，`NotFound` 视为成功。

### 5.3 进度事件

- 后端在分块循环中 `app.emit("attachment-progress", { id, done, total })`。
- 前端监听并驱动进度条；完成/失败后清理监听与进度状态。

## 6. UI 设计

### 6.1 接入 ant-design-vue（按需导入）

遵循 `2026-09-29-antd-migration-design.md` 的接入方式，**范围收缩**为本特性所需。

- dependencies：`ant-design-vue@^4`、`@ant-design/icons-vue`
- devDependencies：`unplugin-vue-components`、`unplugin-auto-import`
- `vite.config.ts`：`Components`（`AntDesignVueResolver({ importStyle: false, resolveIcons: true })`，`dts: 'src/components.d.ts'`）+ `AutoImport`（`dts: 'src/auto-imports.d.ts'`）。
- `src/main.ts`：引入 `ant-design-vue/dist/reset.css`（置于 `main.css` 之前或之后按原稿），不删现有 CSS 令牌。
- `App.vue`：根部包 `a-config-provider :locale="zhCN"`。

### 6.2 选择模型改造（`App.vue`）

```ts
type Selection = { kind: 'group' | 'project'; id: string } | null
```

- `ProjectList.vue` 的 `select` 事件携带 `kind`。
- 右侧按 `kind` 渲染 `GroupDetail.vue` 或 `ProjectDetail.vue`。

### 6.3 分组右侧 —— `GroupDetail.vue`

`<a-tabs>`，页签：`笔记 / 上传`。

### 6.4 项目右侧 —— `ProjectDetail.vue`

`<a-tabs>`，页签：`项目配置 / 笔记 / 上传`；第一页签内嵌现有 `ProjectConfig.vue`。

### 6.5 笔记页签 —— `NoteEditor.vue`（共用）

- 左侧编辑区 `<a-textarea>` + 右侧实时预览（左右分栏，窄屏上下堆叠）。
- 顶部小工具条：加粗 / 斜体 / 标题 / 列表 / 链接（插入语法）。
- 保存：沿用 `ProjectConfig` 的防抖 `emit('update', ...)` 模式。
- 预览：`marked` 渲染 → `DOMPurify` 消毒后 `v-html`。

### 6.6 上传页签 —— `AttachmentPanel.vue`（共用）

自上而下：
1. **上传位置行**：显示 4.2 解析出的实际路径 + 来源徽标（自定义 / 全局默认 / App 私有）；`[更改]` 选目录写入 `entity.upload_dir`；`[恢复默认]` 清空为 `None`。
2. **上传区**：拖拽或 `[选择文件]` → `encrypt_upload_file` → push meta → `save_config` → 刷新。
3. **附件列表**：名称 / 大小 / 时间 + 操作 `[预览][下载][重命名][删除]`。
   - 预览：图像直接走 `wst-img` 协议在应用内按需解密渲染，不落临时文件；非图像给出下载提示。原「打开」（`decrypt_to_temp` → shell 打开系统默认程序）会在系统临时目录堆积明文文件，已移除。
   - 下载：dialog `save` 选目标 → `decrypt_export_file`。
   - 重命名：行内编辑 `meta.name` → `save_config`。
   - 删除：单次确认 → `remove_attachment_file` → 删 meta → `save_config`。（删除**实体**时的附件联动见 §7.5）
4. **进度条**：由 5.3 事件驱动；进行中禁用该项重复操作。

### 6.7 新增依赖（前端）

`marked`、`dompurify`（Markdown 渲染与消毒）。

## 7. 异常处理与边界

### 7.1 上传
- 源文件不存在/不可读 → 报错，不产生 meta。
- 目标目录不可写 / `create_dir_all` 失败 → 报「路径 + 权限」。
- **原子写**：先写 `.tmp` 再 `rename`；失败清理临时文件。

### 7.2 解密（下载 / 预览）
- MAC 校验失败 → 拒绝产出，提示「文件损坏或无法解密」。
- 跨机器不可解：主密钥机器绑定，`upload_dir` 指向同步盘时别的机器解不开（与现有配置同命运），设计不做跨机方案，仅 UI 文案提示。
- 主密钥丢失/更换 → blob 与配置一并不可解（既有约束）。
- 导出目标路径非法/被占用 → 报错。
- 导出文件名非法字符 → 用 `meta.name`，必要时 sanitize。

### 7.3 列表与状态
- meta 在但 blob 缺失 → 提示「文件不存在」，提供「从列表移除」。
- 同名上传 → uuid 不同，允许共存。
- 大文件进行中 → 禁用重复操作。

### 7.4 笔记 / 目录
- 笔记空字符串正常保存。
- `upload_dir` 为空 → 走兜底链，不会为空。
- 改 `upload_dir` → 按做法 A，旧附件仍按各自 `dir` 可读写。

### 7.5 删除实体的附件联动（单次弹窗 + 勾选项）

删除某个分组 / 项目时，弹**单次确认弹窗**（`a-modal` / `Modal.confirm`）：

1. 提示文案：「确认删除<分组 / 项目名>？此操作不可恢复。」
2. **提示文本下方**放一个复选框：「同时删除附件文件」，**默认不勾选**。
   - 勾选 → 遍历该实体 `attachments` 逐个调 `remove_attachment_file` 删 blob；
   - 不勾选（默认）→ 不碰磁盘，仅删配置中的实体与元数据（blob 成为无引用孤儿）。
3. 点「确认」→ 删实体（含其 `note` / `upload_dir` / `attachments` 元数据）→ `save_config`；点「取消」则中止。

删除**分组**时，该弹窗与勾选项覆盖该分组**及其名下所有项目**的附件，一次处理。

## 8. 改动文件清单

### 后端
| 文件 | 动作 |
| --- | --- |
| `src-tauri/src/models.rs` | 改：新增 `AttachmentMeta` 与 `note/upload_dir/attachments/default_upload_dir` |
| `src-tauri/src/filecrypto.rs` | 新建：`WST1` 流式加解密 |
| `src-tauri/src/commands.rs` | 改：新增 4 个命令 |
| `src-tauri/src/lib.rs` | 改：注册新命令、声明 `mod filecrypto` |

### 前端
| 文件 | 动作 |
| --- | --- |
| `src/components/GroupDetail.vue` | 新建 |
| `src/components/ProjectDetail.vue` | 新建 |
| `src/components/NoteEditor.vue` | 新建 |
| `src/components/AttachmentPanel.vue` | 新建 |
| `src/App.vue` | 改：选择模型 + 右侧路由 + `a-config-provider` |
| `src/components/ProjectList.vue` | 改：`select` 带 `kind` |
| `src/composables/useConfig.ts` | 改：附件/笔记/`upload_dir` 增删改方法 |
| `src/types/index.ts` | 改：镜像新字段 |
| `src/main.ts` | 改：引入 antd reset 样式 |
| `vite.config.ts` | 改：antd 按需导入插件 |
| `package.json` | 改：antd / icons / unplugin / marked / dompurify |
| `src/components.d.ts`、`src/auto-imports.d.ts` | 生成并提交 |

## 9. 验收标准

1. 分组/项目均可编辑单条 Markdown 笔记；保存后重启应用，内容仍在且磁盘为密文。
2. 上传文件后：磁盘出现 `<uuid>.wst`，且用文本工具查看为密文（非原文件名/内容）；配置中的文件名亦为密文。
3. 预览/下载得到的内容与原文件一致（字节级）；文件名正确还原。
4. 分组/项目可分别设置 `upload_dir`；未设置时按兜底链落在 App 私有目录。
5. 上传/解密有进度显示；进行中不可重复触发。
6. 删除分组/项目为单次确认弹窗，且提示文本下方有**默认未勾选**的「同时删除附件文件」选项；勾选/不勾选两种情况下 blob 与元数据处理均正确。
7. `pnpm build`（含 `vue-tsc --noEmit`）通过。
8. `pnpm exec tauri dev` 正常启动，交互正常（改完重启验证）。

## 10. 风险

- **大文件内存**：若误用字符串信封会 OOM——固定走 `WST1` 流式通道。
- **XSS**：Markdown 预览必须经 DOMPurify 消毒。
- **antd 接入影响构建**：`components.d.ts` / `auto-imports.d.ts` 缺失会导致类型报错——构建前确认已生成并纳入 tsconfig。
- **原生拖拽回归**：`ProjectList.vue` 改动 `select` 时不得破坏现有拖拽命中结构。
- **跨机器误解**：用户可能误以为同步盘可跨机解密——UI 明确提示。
- **uuid 依赖**：`uuid v4` 已在 `Cargo.toml`，无需新增。
