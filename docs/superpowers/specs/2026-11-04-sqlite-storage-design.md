# SQLite 本地存储 设计文档

- 日期：2026-11-04
- 分支：feat/vpn
- 状态：待评审
- 关联：`2026-09-29-local-storage-encryption-design.md`（本地存储加密）、`2026-09-30-group-project-notes-and-encrypted-file-upload-design.md`（分组/项目/笔记/附件数据模型）

## 1. 背景与目标

当前所有数据都塞在一份加密的 `config.json` 里：分组、项目、笔记、附件元信息、VPN 配置、规则集以及应用设置，全部序列化成 `AppConfig` 后用 SM4 信封加密，通过 `tauri-plugin-store` 落盘。每保存一次就整份重新序列化、重新加密，数据量增长后单文件会持续膨胀，也没有结构化查询能力。

本次引入 SQLite 作为业务数据的存储层，`config.json` 收缩为只存应用设置；前端使用的 `get_config` / `save_config` 命令与 `AppConfig` 结构保持不变，组件与数据流不动。

### 目标（In Scope）

1. 使用 `rusqlite`（bundled SQLite）在本地建立 `ws-workbench.db`，业务实体分表存储。
2. 每行内容字段使用现有 SM4 信封加密后写入 `payload` 列，主密钥复用 `crypto` 的钥匙串 / DPAPI 体系。
3. 分组、项目、笔记、附件元信息、VPN 配置、规则集全部入库；`config.json` 只保留应用设置。
4. 保持「整份读、整份写」的前端接口：加载时从多表组装 `AppConfig`，保存时与内存快照做 diff，事务内只写变化的行。
5. 引入 `PRAGMA user_version` 管理表结构版本。
6. 单测使用内存数据库，不依赖 `AppHandle`。

### 非目标（Out of Scope）

- 不使用 SQLCipher / 整库加密；数据库文件本身不做加密，只有 `payload` 列是密文。
- 不迁移旧数据：开发阶段，旧 `config.json` 里的分组/项目/笔记/VPN 配置不导入，首次启动应为空库。
- 附件正文不入库，仍以 `.wst` 密文文件存放在上传目录。
- 不做前端细粒度查询（不新增按实体读取/保存的命令），不改 `useConfig` 的数据流。
- 不删除 `config.json` 与 `tauri-plugin-store`（方案 B 不在本次范围）。
- 不处理多进程并发写数据库；数据库只由主进程访问。

## 2. 现状与约束

- `crypto.rs`：`pub fn encrypt_str(app, plain)` / `decrypt_str(app, envelope)` / `is_encrypted(s)`；密文信封 `v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`，SM4-CBC + HMAC-SM3；主密钥 macOS 存钥匙串、Windows 经 DPAPI 存 `<app_data_dir>/.vault_key`。`vault_keys` 目前是模块内私有函数，需要提升可见性给 `db.rs` 使用。
- `config.rs`：`load_config` / `save_config` / `get_config_path`；保存顺序为 Store 插件 → 加密文件 → 内存缓存；可写目录候选为 `app_data_dir` → `app_config_dir` → `~/.ws-workbench` → `$HOME/.ws-workbench` → `cwd/.config`。
- `models.rs`：`AppConfig { groups, projects, default_exclude, default_exclude_version, vpn_profiles, vpn_rule_sets, vpn_active_profile_id, app_paths, vpn_worker_session_dir, default_upload_dir }`；`ProjectGroup` 内嵌 `projects`，分组与项目都带 `note`、`upload_dir`、`attachments`。
- 前端：`useConfig` 通过 `get_config` / `save_config` 整份读写；`useVpn` 复用同一份 `config` 与 `saveConfig`。
- 项目规则：凡写入本地磁盘的数据必须密文存储。

## 3. 选型与依赖

- 新增 Rust 依赖：`rusqlite = { version = "0.32", features = ["bundled"] }`。`bundled` 会把 SQLite 源码编进二进制，不依赖系统库、不引入 OpenSSL。
- 不新增前端依赖，不新增 Tauri 插件或 capability 权限。
- 数据库访问只在 Rust 侧，前端仍通过 `invoke` 调用命令。

## 4. 数据库设计

### 4.1 文件与连接

- 文件路径：`<storage_dir>/ws-workbench.db`，其中 `storage_dir` 复用 `config.rs` 的可写目录解析（与 `config.json` 同目录），避免应用数据分散。
- 每次命令打开一次连接即可（本地单文件、量级很小）；打开后设置 `busy_timeout = 2000ms`、`journal_mode = WAL`。
- 写操作统一包在 `BEGIN IMMEDIATE` 事务中，失败回滚。
- 表结构版本用 `PRAGMA user_version` 记录。

### 4.2 表结构（`user_version = 1`）

```sql
CREATE TABLE IF NOT EXISTS groups (
  id         TEXT PRIMARY KEY NOT NULL,
  sort_order INTEGER NOT NULL,
  payload    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
  id         TEXT PRIMARY KEY NOT NULL,
  group_id   TEXT NOT NULL,
  sort_order INTEGER NOT NULL,
  payload    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_projects_group_order ON projects(group_id, sort_order);

CREATE TABLE IF NOT EXISTS vpn_profiles (
  id         TEXT PRIMARY KEY NOT NULL,
  sort_order INTEGER NOT NULL,
  payload    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS vpn_rule_sets (
  id         TEXT PRIMARY KEY NOT NULL,
  sort_order INTEGER NOT NULL,
  payload    TEXT NOT NULL
);
```

- `payload` = 实体 JSON 序列化后用 `crypto::encrypt_str` 加密得到的字符串。
- 分组的 payload 不包含内嵌的 `projects` 数组（序列化前克隆并清空），项目单独成行；加载时按 `group_id` 和 `sort_order` 组装回 `ProjectGroup { projects }`。
- 笔记、附件元信息仍嵌在分组/项目的 payload 里，`AppConfig` 与前端类型不变。
- VPN 密码继续以明文放在实体 JSON 中、由 payload 整体加密保护（与现状一致，`password_encrypted` 保持 `false`）。
- `AppConfig.projects` 字段保留以维持前后端类型一致，但恒为空数组；数据以 `groups[].projects` 为准。

### 4.3 加密与密钥

- `crypto.rs` 把现有私有的 `vault_keys` 函数与 `VaultKeys` 类型提升为 `pub(crate)`，供 `db.rs` 取主密钥；对外行为不变。
- `db.rs` 对外提供带 `&VaultKeys` 参数的纯函数（`load_all`、`save_diff`），另提供接收 `AppHandle` 的薄封装供命令层调用；前者是可单测的边界。
- 只加密 `payload`；`id`、`group_id`、`sort_order` 为明文。可接受的泄露面仅限记录数量与 UUID，不包含名称、笔记、密码等内容。

### 4.4 加载流程

1. `SELECT id, sort_order, payload FROM groups ORDER BY sort_order`，逐行解密并反序列化为 `ProjectGroup { projects: vec![] }`。
2. `SELECT id, group_id, sort_order, payload FROM projects ORDER BY group_id, sort_order`，解密后挂到对应分组的 `projects` 末尾；找不到所属分组视为数据不一致，整体返回 `Err`。
3. 同样方式读取 `vpn_profiles`、`vpn_rule_sets`，按 `sort_order` 排序。
4. 与 `config.json` 中的设置合并成 `AppConfig`，确保默认分组存在（复用 `mark_default_group`）。
5. 任一行解密或反序列化失败都整体报错，不静默丢弃、不回写。

### 4.5 保存流程（事务内 diff）

1. 序列化每个实体（分组先清空 `projects`），与内存快照中的同 id 实体比较 `serde_json::Value`；相同则跳过。
2. 发生变化的实体用 `crypto::encrypt_str` 重新加密后 `INSERT ... ON CONFLICT(id) DO UPDATE`；快照中不存在的新实体同样 upsert。
3. 快照有、新配置没有的 id 执行 `DELETE`。
4. 只调整顺序、内容没变的实体，单独 `UPDATE sort_order`。
5. 所有写操作在一个事务里完成，失败回滚并返回错误；只有事务提交成功后才更新内存快照。
6. 设置部分继续走 `config.rs` 现有的 Store / 文件保存逻辑。两个存储无法跨库原子提交：设置与数据库分别保存，任一失败都向调用方返回错误，但数据库事务本身仍保证原子性。

### 4.6 版本与错误处理

- `user_version = 0`：执行建表 SQL 并置为 1；`user_version > 1`：返回「数据库版本过新」错误，不尝试降级。
- 数据库打开或建表失败：`load_config` 回退到内存默认值并记录日志，`save_config` 返回明确错误；绝不因写入失败清空既有数据。
- `storage_info` 命令返回 `{ db_path, settings_path, ok, error }`，供设置页展示和排查「库不可写」问题。

## 5. 命令接口

业务命令签名不变：

```rust
#[tauri::command] pub fn get_config(app: AppHandle) -> AppConfig;
#[tauri::command] pub fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String>;
#[tauri::command] pub fn get_config_path(app: AppHandle) -> Result<String, String>;
```

- `get_config_path` 改为返回数据库路径；设置页文案从「配置文件位置」改为「数据库位置」。
- 新增只读命令：

```rust
#[derive(serde::Serialize)]
pub struct StorageInfo {
    pub db_path: String,
    pub settings_path: String,
    pub ok: bool,
    pub error: String,
}

#[tauri::command] pub fn storage_info(app: AppHandle) -> StorageInfo;
```

## 6. 前端改动

- `useConfig` / `useVpn` / 其他组件不改。
- `SettingsPanel.vue` 增加一行「存储位置」：调用 `storage_info`，展示数据库路径、设置文件路径与可用状态。
- `src/types/index.ts` 的 `AppConfig` 不变。

## 7. 代码组织

- 新增 `src-tauri/src/db.rs`：可写目录解析、连接打开、建表与版本检查、`load_all`、`save_diff`。
- `src-tauri/src/config.rs`：
  - 新增 `AppSettings`（`app_paths`、`default_upload_dir`、`default_exclude`、`default_exclude_version`、`vpn_active_profile_id`、`vpn_worker_session_dir`），Store / 文件 / 内存回退逻辑收敛到设置部分。
  - `load_config` / `save_config` 负责设置与数据库的组装/拆分。
  - 删除字段级 VPN 密码迁移（`migrate_vpn_passwords`）与默认排除规则的旧数据升级分支；保留 `mark_default_group` 和排除规则版本常量。
- `src-tauri/src/models.rs`：新增 `AppSettings` 定义，与 `AppConfig` 一起维护序列化结构。
- `src-tauri/src/commands.rs`：`storage_info` 薄封装。
- `src-tauri/Cargo.toml`：新增 `rusqlite`。

## 8. 测试

- `db.rs` 单测（固定密钥 + `Connection::open_in_memory()`）：
  - 建表与 `user_version` 检查；
  - 分组/项目/笔记/VPN 配置往返一致；
  - diff：只改一条笔记时，其他行保存前后的 `payload` 字节完全相同；
  - 删除实体、调整排序；
  - 版本过新与坏密文返回错误。
- `config.rs` 现有测试调整：设置读写、`mark_default_group` 保留；删除已移除的迁移测试。
- 手动验收：`pnpm exec tauri dev` 后创建数据并重启；用 `strings` 扫 `.db` 文件确认查不到笔记明文。

## 9. 验收标准

1. 首次启动自动创建 `ws-workbench.db` 表结构与默认分组。
2. 增删改分组、项目、笔记、附件元信息、VPN 配置、规则集后重启，界面数据一致。
3. 数据库文件里 `payload` 均为 `v1.` 信封密文，`strings` 搜不到笔记正文。
4. 只修改一条笔记时，其他实体的 `payload` 字节不变。
5. 数据库不可写时，设置页能看到明确错误，且应用不会覆盖既有数据。
6. `cargo test` 与 `pnpm build` 通过。

## 10. 风险与备选

- **bundled SQLite 的交叉编译**：Windows 侧由 `cargo-xwin` + `llvm` 编译 C 源码，需在 CI 上验证；这是本次唯一新增的原生编译依赖。
- **启动全量解密**：笔记量大时启动会解密所有行，本次接受该取舍；后续可切换为「骨架全量 + 笔记按需」。
- **备份方式变化**：升级后需要备份 `ws-workbench.db`、`config.json` 与附件目录；README 的「配置与数据」需同步更新。
- 备选方案 B（设置也入库、移除 `config.json`）留到后续清理，不在本次实现。
