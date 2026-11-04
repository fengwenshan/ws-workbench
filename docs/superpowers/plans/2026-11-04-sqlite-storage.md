# SQLite 本地存储 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 SQLite（rusqlite bundled）承载分组/项目/笔记/附件元信息/VPN 配置/规则集，payload 沿用现有 SM4 信封加密；`config.json` 收缩为应用设置，前端命令与数据流不变。

**Architecture:** 新增 `db.rs` 负责建库、加密载荷、全量读取与「事务内 diff 写入」；`config.rs` 拆出 `AppSettings`（存 `config.json`）与 `AppConfig` 的组装/拆分，并把加载到的业务快照缓存为下一次保存的 diff 基线；`get_config`/`save_config` 签名不变，新增只读的 `storage_info` 供设置页展示。

**Tech Stack:** Rust（rusqlite bundled）、Tauri 2、Vue 3 + ant-design-vue；现有 `crypto.rs`（SM4-CBC + HMAC-SM3 信封）与 `tauri-plugin-store`。

---

## 文件结构

- 新增 `src-tauri/src/db.rs`：数据库路径解析、连接/建表、实体信封加解密、`load_all`、`save_diff`、AppHandle 薄封装。
- 修改 `src-tauri/src/crypto.rs`：`encrypt_with_keys` / `decrypt_with_keys` 提升为 `pub(crate)`（`db.rs` 用固定密钥做单测）。
- 修改 `src-tauri/src/models.rs`：新增 `AppSettings`（业务数据以外的设置）。
- 修改 `src-tauri/src/config.rs`：`config.json` 只读写 `AppSettings`；`load_config`/`save_config` 组装/拆分；删除旧数据迁移；新增 `settings_path`。
- 修改 `src-tauri/src/commands.rs`：新增 `storage_info` 命令。
- 修改 `src-tauri/src/lib.rs`：`mod db;` 与 `commands::storage_info` 注册。
- 修改 `src/components/settings/SettingsPanel.vue`：显示数据库路径、设置文件路径与存储状态。
- 修改 `src-tauri/Cargo.toml`：新增 `rusqlite`。
- 修改 `README.md`：「配置与数据」章节同步。

---

### Task 1: 引入 rusqlite 与数据库骨架（路径 / 连接 / 表结构）

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/db.rs`
- Modify: `src-tauri/src/lib.rs:1-12`
- Modify: `src-tauri/src/config.rs:166`（`get_write_candidates` → `pub(crate) storage_candidates`）

- [ ] **Step 1: 添加依赖**

Run:
```bash
cd src-tauri && cargo add rusqlite --features bundled
```
Expected: `Cargo.toml` 的 `[dependencies]` 出现 `rusqlite = { version = "…", features = ["bundled"] }`。

- [ ] **Step 2: 重命名候选目录函数**

`src-tauri/src/config.rs:166` 的 `fn get_write_candidates(app: &AppHandle) -> Vec<(String, std::path::PathBuf)>` 改成：
```rust
pub(crate) fn storage_candidates(app: &AppHandle) -> Vec<(String, std::path::PathBuf)> {
```
函数体保持不变；同时把 `find_writable_file_path` 里的 `for (_, dir) in get_write_candidates(app)` 改为 `for (_, dir) in storage_candidates(app)`。

- [ ] **Step 3: 声明模块**

`src-tauri/src/lib.rs` 顶部模块列表里，在 `mod config;` 后加一行：
```rust
mod db;
```

- [ ] **Step 4: 写失败测试**

创建 `src-tauri/src/db.rs`，先只写模块文档、`DB_FILE` 常量和测试：

```rust
//! SQLite 本地数据库：业务实体（分组/项目/VPN 配置/规则集）存储层。
//!
//! 表里只放 `id`、`group_id`、`sort_order` 等少量明文列；实体内容序列化成
//! JSON 后用 [`crate::crypto`] 的信封（SM4-CBC + HMAC-SM3）加密存进 `payload`。

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use tauri::AppHandle;

/// 数据库文件名，与 config.json 放在同一个可写目录下
pub const DB_FILE: &str = "ws-workbench.db";

/// 表结构版本
const SCHEMA_VERSION: i64 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn init_schema_sets_user_version_and_tables() {
        let conn = memory_conn();
        init_schema(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' \
                 AND name IN ('groups', 'projects', 'vpn_profiles', 'vpn_rule_sets')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn init_schema_rejects_newer_version() {
        let conn = memory_conn();
        conn.pragma_update(None, "user_version", 2i64).unwrap();
        let err = init_schema(&conn).unwrap_err();
        assert!(err.contains("版本过新"), "实际错误：{}", err);
    }
}
```

- [ ] **Step 5: 运行测试确认失败**

Run:
```bash
cd src-tauri && cargo test --lib db::tests
```
Expected: 编译失败，提示 `init_schema` 未定义。

- [ ] **Step 6: 实现路径解析、连接与建表**

在 `DB_FILE` / `SCHEMA_VERSION` 之后追加（同时把 `Path`、`PathBuf`、`Duration`、`AppHandle` 用起来）：

```rust
const SCHEMA_SQL: &str = r#"
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
"#;

/// 解析出可写的存储目录，优先创建 app_data_dir 等候选目录
fn resolve_storage_dir(app: &AppHandle) -> Result<PathBuf, String> {
    use std::fs;
    for (_, dir) in crate::config::storage_candidates(app) {
        if !dir.exists() && fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let probe = dir.join(".db_write_test");
        if fs::write(&probe, b"probe").is_ok() {
            let _ = fs::remove_file(&probe);
            return Ok(dir);
        }
    }
    Err("没有可写的本地存储目录".to_string())
}

/// 数据库文件的绝对路径
pub fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(resolve_storage_dir(app)?.join(DB_FILE))
}

/// 打开（必要时创建）数据库并确保表结构就绪
pub fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("打开数据库失败: {}", e))?;
    conn.busy_timeout(Duration::from_millis(2000))
        .map_err(|e| format!("设置 busy_timeout 失败: {}", e))?;
    let _: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|e| format!("设置 WAL 失败: {}", e))?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("读取数据库版本失败: {}", e))?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version > SCHEMA_VERSION {
        return Err(format!("数据库版本过新（{}），请升级应用后再打开", version));
    }
    conn.execute_batch(SCHEMA_SQL)
        .map_err(|e| format!("创建数据库表失败: {}", e))?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|e| format!("写入数据库版本失败: {}", e))?;
    Ok(())
}

/// 校验数据库可打开并返回路径（供 `storage_info` 使用）
pub fn check(app: &AppHandle) -> Result<PathBuf, String> {
    let path = db_path(app)?;
    open(&path)?;
    Ok(path)
}
```

- [ ] **Step 7: 运行测试确认通过**

Run:
```bash
cd src-tauri && cargo test --lib db::tests
```
Expected: `2 passed`。

- [ ] **Step 8: 提交**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/db.rs src-tauri/src/lib.rs src-tauri/src/config.rs
git commit -m "feat(db): 引入 rusqlite 并建立数据库骨架"
```

---

### Task 2: 实体加密载荷、全量读取与事务内 diff 写入

**Files:**
- Modify: `src-tauri/src/crypto.rs:87`、`:113`（两个 with_keys 函数变 `pub(crate)`）
- Modify: `src-tauri/src/db.rs`（追加 `DbData` 与读写实现、测试）

- [ ] **Step 1: 提升加密原语的可见性**

把 `crypto.rs` 中：
```rust
fn encrypt_with_keys(keys: &VaultKeys, plain: &str) -> Result<String, String> {
```
改为：
```rust
pub(crate) fn encrypt_with_keys(keys: &VaultKeys, plain: &str) -> Result<String, String> {
```
把：
```rust
fn decrypt_with_keys(keys: &VaultKeys, envelope: &str) -> Result<String, String> {
```
改为：
```rust
pub(crate) fn decrypt_with_keys(keys: &VaultKeys, envelope: &str) -> Result<String, String> {
```

- [ ] **Step 2: 写失败测试**

在 `db.rs` 的测试模块里追加（保留 Task 1 的两个测试）：

```rust
    use crate::models::{AttachmentMeta, ProjectConfig, ProjectGroup};
    use crate::vpn::models::{VpnProfile, VpnRuleSet};

    fn test_keys() -> crate::crypto::VaultKeys {
        crate::crypto::VaultKeys {
            enc_key: [7u8; 32],
            mac_key: [9u8; 32],
        }
    }

    fn sample_project(id: &str, note: &str) -> ProjectConfig {
        ProjectConfig {
            id: id.to_string(),
            name: format!("项目-{}", id),
            source_dir: format!("/tmp/{}", id),
            output_dir: format!("/tmp/{}-dist", id),
            exclude: vec![".git".to_string()],
            created_at: "2026-11-04T00:00:00Z".to_string(),
            updated_at: "2026-11-04T00:00:00Z".to_string(),
            note: note.to_string(),
            upload_dir: None,
            attachments: vec![AttachmentMeta {
                id: format!("{}-att", id),
                name: "a.txt".to_string(),
                size: 3,
                ext: "txt".to_string(),
                created_at: 0,
                dir: "/tmp".to_string(),
            }],
        }
    }

    fn sample_group(id: &str, projects: Vec<ProjectConfig>) -> ProjectGroup {
        ProjectGroup {
            id: id.to_string(),
            name: format!("分组-{}", id),
            projects,
            is_default: false,
            created_at: "2026-11-04T00:00:00Z".to_string(),
            note: "分组的加密笔记".to_string(),
            upload_dir: None,
            attachments: vec![],
        }
    }

    fn sample_data() -> DbData {
        let profile: VpnProfile =
            serde_json::from_str(r#"{"id":"p1","name":"办公室","server":"vpn.example.com","username":"u"}"#)
                .unwrap();
        let rule_set: VpnRuleSet =
            serde_json::from_str(r#"{"id":"r1","name":"规则集"}"#).unwrap();
        DbData {
            groups: vec![
                sample_group("g1", vec![sample_project("a1", "项目 A 的笔记")]),
                sample_group("g2", vec![]),
            ],
            vpn_profiles: vec![profile],
            vpn_rule_sets: vec![rule_set],
        }
    }

    fn memory_db() -> (Connection, crate::crypto::VaultKeys) {
        let mut conn = memory_conn();
        init_schema(&conn).unwrap();
        save_diff(&mut conn, &test_keys(), &DbData::default(), &sample_data()).unwrap();
        (conn, test_keys())
    }

    #[test]
    fn save_and_load_roundtrip() {
        let (conn, keys) = memory_db();
        let loaded = load_all(&conn, &keys).unwrap();
        let expected = sample_data();
        assert_eq!(loaded.groups.len(), 2);
        assert_eq!(loaded.groups[0].projects[0].note, expected.groups[0].projects[0].note);
        assert_eq!(loaded.groups[0].note, expected.groups[0].note);
        assert_eq!(loaded.groups[1].name, expected.groups[1].name);
        assert_eq!(loaded.vpn_profiles[0].name, expected.vpn_profiles[0].name);
        assert_eq!(loaded.vpn_rule_sets[0].name, expected.vpn_rule_sets[0].name);
    }

    #[test]
    fn payload_is_encrypted_at_rest() {
        let (conn, _) = memory_db();
        let payloads: Vec<String> = {
            let mut stmt = conn.prepare("SELECT payload FROM groups").unwrap();
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        assert!(payloads.iter().all(|p| p.starts_with("v1.")));
        assert!(payloads.iter().all(|p| !p.contains("加密笔记")));
    }

    #[test]
    fn unchanged_rows_keep_their_payload_bytes() {
        let mut conn = memory_conn();
        init_schema(&conn).unwrap();
        let keys = test_keys();
        let prev = sample_data();
        save_diff(&mut conn, &keys, &DbData::default(), &prev).unwrap();

        let payload_of = |conn: &Connection, id: &str| -> String {
            conn.query_row("SELECT payload FROM groups WHERE id = ?1", [id], |row| row.get(0))
                .unwrap()
        };
        let before_g2 = payload_of(&conn, "g2");

        let mut next = prev.clone();
        next.groups[0].projects[0].note = "改过的笔记".to_string();
        save_diff(&mut conn, &keys, &prev, &next).unwrap();

        assert_eq!(payload_of(&conn, "g2"), before_g2, "未变化的分组不应重写");
        let loaded = load_all(&conn, &keys).unwrap();
        assert_eq!(loaded.groups[0].projects[0].note, "改过的笔记");
    }

    #[test]
    fn delete_removes_rows() {
        let mut conn = memory_conn();
        init_schema(&conn).unwrap();
        let keys = test_keys();
        let prev = sample_data();
        save_diff(&mut conn, &keys, &DbData::default(), &prev).unwrap();

        let mut next = prev.clone();
        next.groups.remove(0); // 连同 g1 下的项目一起删
        save_diff(&mut conn, &keys, &prev, &next).unwrap();

        let loaded = load_all(&conn, &keys).unwrap();
        assert_eq!(loaded.groups.len(), 1);
        assert_eq!(loaded.groups[0].id, "g2");
        let projects: i64 = conn
            .query_row("SELECT count(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(projects, 0);
    }

    #[test]
    fn corrupt_payload_errors() {
        let (conn, keys) = memory_db();
        conn.execute("UPDATE groups SET payload = 'v1.bad' WHERE id = 'g1'", [])
            .unwrap();
        assert!(load_all(&conn, &keys).is_err());
    }

    #[test]
    fn project_without_group_errors() {
        let (conn, keys) = memory_db();
        conn.execute("DELETE FROM groups WHERE id = 'g1'", []).unwrap();
        let err = load_all(&conn, &keys).unwrap_err();
        assert!(err.contains("不属于任何分组"), "实际错误：{}", err);
    }
```

- [ ] **Step 3: 运行测试确认失败**

Run:
```bash
cd src-tauri && cargo test --lib db::tests
```
Expected: 编译失败，`DbData`、`save_diff`、`load_all` 未定义。

- [ ] **Step 4: 实现加载、保存与 diff**

在 `db.rs` 的 `check` 之后、测试模块之前追加（`use` 部分按注释补到文件顶部）：

```rust
use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::crypto::VaultKeys;
use crate::models::{AppConfig, ProjectConfig, ProjectGroup};
use crate::vpn::models::{VpnProfile, VpnRuleSet};

/// 从数据库读出的业务实体快照
#[derive(Debug, Clone, Default)]
pub struct DbData {
    pub groups: Vec<ProjectGroup>,
    pub vpn_profiles: Vec<VpnProfile>,
    pub vpn_rule_sets: Vec<VpnRuleSet>,
}

impl DbData {
    /// 从完整配置中抽出业务实体部分（`AppConfig.projects` 为旧字段，忽略）
    pub fn from_config(config: &AppConfig) -> Self {
        DbData {
            groups: config.groups.clone(),
            vpn_profiles: config.vpn_profiles.clone(),
            vpn_rule_sets: config.vpn_rule_sets.clone(),
        }
    }
}

fn json_value<T: Serialize>(value: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| format!("序列化实体失败: {}", e))
}

fn seal_json(keys: &VaultKeys, value: &serde_json::Value) -> Result<String, String> {
    let json = serde_json::to_string(value).map_err(|e| format!("序列化实体失败: {}", e))?;
    crate::crypto::encrypt_with_keys(keys, &json)
}

fn unseal<T: DeserializeOwned>(keys: &VaultKeys, payload: &str) -> Result<T, String> {
    let json = crate::crypto::decrypt_with_keys(keys, payload)?;
    serde_json::from_str(&json).map_err(|e| format!("解析实体失败: {}", e))
}

/// 分组入库前先摘掉内嵌项目（项目单独成行）
fn group_value(group: &ProjectGroup) -> Result<serde_json::Value, String> {
    let mut snapshot = group.clone();
    snapshot.projects.clear();
    json_value(&snapshot)
}

pub fn save_diff(
    conn: &mut Connection,
    keys: &VaultKeys,
    prev: &DbData,
    next: &DbData,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| format!("开启事务失败: {}", e))?;
    write_groups(&tx, keys, prev, next)?;
    write_projects(&tx, keys, prev, next)?;
    write_vpn_profiles(&tx, keys, prev, next)?;
    write_vpn_rule_sets(&tx, keys, prev, next)?;
    tx.commit().map_err(|e| format!("提交事务失败: {}", e))
}

fn write_groups(
    tx: &rusqlite::Transaction,
    keys: &VaultKeys,
    prev: &DbData,
    next: &DbData,
) -> Result<(), String> {
    let mut prev_rows: HashMap<&str, (usize, serde_json::Value)> = HashMap::new();
    for (idx, group) in prev.groups.iter().enumerate() {
        prev_rows.insert(group.id.as_str(), (idx, group_value(group)?));
    }
    for (idx, group) in next.groups.iter().enumerate() {
        let value = group_value(group)?;
        let unchanged = prev_rows
            .get(group.id.as_str())
            .map(|(prev_idx, prev_value)| *prev_idx == idx && prev_value == &value)
            .unwrap_or(false);
        if unchanged {
            continue;
        }
        let payload = seal_json(keys, &value)?;
        tx.execute(
            "INSERT INTO groups (id, sort_order, payload) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET sort_order = excluded.sort_order, payload = excluded.payload",
            rusqlite::params![group.id, idx as i64, payload],
        )
        .map_err(|e| format!("写入分组失败: {}", e))?;
    }
    for group in &prev.groups {
        if !next.groups.iter().any(|g| g.id == group.id) {
            tx.execute("DELETE FROM groups WHERE id = ?1", rusqlite::params![group.id])
                .map_err(|e| format!("删除分组失败: {}", e))?;
        }
    }
    Ok(())
}

fn write_projects(
    tx: &rusqlite::Transaction,
    keys: &VaultKeys,
    prev: &DbData,
    next: &DbData,
) -> Result<(), String> {
    let mut prev_rows: HashMap<&str, (&str, usize, serde_json::Value)> = HashMap::new();
    for group in &prev.groups {
        for (idx, project) in group.projects.iter().enumerate() {
            prev_rows.insert(
                project.id.as_str(),
                (group.id.as_str(), idx, json_value(project)?),
            );
        }
    }
    for group in &next.groups {
        for (idx, project) in group.projects.iter().enumerate() {
            let value = json_value(project)?;
            let unchanged = prev_rows
                .get(project.id.as_str())
                .map(|(prev_group, prev_idx, prev_value)| {
                    *prev_group == group.id.as_str() && *prev_idx == idx && prev_value == &value
                })
                .unwrap_or(false);
            if unchanged {
                continue;
            }
            let payload = seal_json(keys, &value)?;
            tx.execute(
                "INSERT INTO projects (id, group_id, sort_order, payload) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET group_id = excluded.group_id,
                   sort_order = excluded.sort_order, payload = excluded.payload",
                rusqlite::params![project.id, group.id, idx as i64, payload],
            )
            .map_err(|e| format!("写入项目失败: {}", e))?;
        }
    }
    for group in &prev.groups {
        for project in &group.projects {
            let still_exists = next
                .groups
                .iter()
                .any(|g| g.projects.iter().any(|p| p.id == project.id));
            if !still_exists {
                tx.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project.id])
                    .map_err(|e| format!("删除项目失败: {}", e))?;
            }
        }
    }
    Ok(())
}

/// 写入「id + 排序 + 内容」的一类实体（VPN 配置 / 规则集）
fn write_simple_rows<T: Serialize>(
    tx: &rusqlite::Transaction,
    keys: &VaultKeys,
    table: &str,
    prev_items: &[T],
    next_items: &[T],
    id_of: fn(&T) -> String,
    is_deleted: impl Fn(&T) -> bool,
) -> Result<(), String> {
    let mut prev_rows: HashMap<String, (usize, serde_json::Value)> = HashMap::new();
    for (idx, item) in prev_items.iter().enumerate() {
        prev_rows.insert(id_of(item), (idx, json_value(item)?));
    }
    for (idx, item) in next_items.iter().enumerate() {
        let id = id_of(item);
        let value = json_value(item)?;
        let unchanged = prev_rows
            .get(&id)
            .map(|(prev_idx, prev_value)| *prev_idx == idx && prev_value == &value)
            .unwrap_or(false);
        if unchanged {
            continue;
        }
        let payload = seal_json(keys, &value)?;
        tx.execute(
            &format!(
                "INSERT INTO {} (id, sort_order, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET sort_order = excluded.sort_order, payload = excluded.payload",
                table
            ),
            rusqlite::params![id, idx as i64, payload],
        )
        .map_err(|e| format!("写入 {} 失败: {}", table, e))?;
    }
    for item in prev_items {
        if is_deleted(item) {
            tx.execute(
                &format!("DELETE FROM {} WHERE id = ?1", table),
                rusqlite::params![id_of(item)],
            )
            .map_err(|e| format!("删除 {} 失败: {}", table, e))?;
        }
    }
    Ok(())
}

fn write_vpn_profiles(
    tx: &rusqlite::Transaction,
    keys: &VaultKeys,
    prev: &DbData,
    next: &DbData,
) -> Result<(), String> {
    write_simple_rows(
        tx,
        keys,
        "vpn_profiles",
        &prev.vpn_profiles,
        &next.vpn_profiles,
        |profile| profile.id.clone(),
        |profile| !next.vpn_profiles.iter().any(|p| p.id == profile.id),
    )
}

fn write_vpn_rule_sets(
    tx: &rusqlite::Transaction,
    keys: &VaultKeys,
    prev: &DbData,
    next: &DbData,
) -> Result<(), String> {
    write_simple_rows(
        tx,
        keys,
        "vpn_rule_sets",
        &prev.vpn_rule_sets,
        &next.vpn_rule_sets,
        |set| set.id.clone(),
        |set| !next.vpn_rule_sets.iter().any(|s| s.id == set.id),
    )
}

fn load_rows<T: DeserializeOwned>(
    conn: &Connection,
    keys: &VaultKeys,
    sql: &str,
) -> Result<Vec<T>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("查询实体失败: {}", e))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("查询实体失败: {}", e))?;
    let mut out = Vec::new();
    for row in rows {
        let payload = row.map_err(|e| format!("读取实体失败: {}", e))?;
        out.push(unseal::<T>(keys, &payload)?);
    }
    Ok(out)
}

pub fn load_all(conn: &Connection, keys: &VaultKeys) -> Result<DbData, String> {
    let mut groups = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, payload FROM groups ORDER BY sort_order")
            .map_err(|e| format!("查询分组失败: {}", e))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| format!("查询分组失败: {}", e))?;
        for row in rows {
            let (id, payload) = row.map_err(|e| format!("读取分组失败: {}", e))?;
            let mut group: ProjectGroup = unseal(keys, &payload)?;
            if group.id != id {
                return Err(format!("分组数据不一致：行 id 为 {}，内容为 {}", id, group.id));
            }
            group.projects.clear();
            groups.push(group);
        }
    }
    {
        let mut stmt = conn
            .prepare("SELECT group_id, payload FROM projects ORDER BY group_id, sort_order")
            .map_err(|e| format!("查询项目失败: {}", e))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| format!("查询项目失败: {}", e))?;
        for row in rows {
            let (group_id, payload) = row.map_err(|e| format!("读取项目失败: {}", e))?;
            let project: ProjectConfig = unseal(keys, &payload)?;
            let group = groups
                .iter_mut()
                .find(|g| g.id == group_id)
                .ok_or_else(|| format!("项目「{}」不属于任何分组", project.name))?;
            group.projects.push(project);
        }
    }
    let vpn_profiles = load_rows::<VpnProfile>(
        conn,
        keys,
        "SELECT payload FROM vpn_profiles ORDER BY sort_order",
    )?;
    let vpn_rule_sets = load_rows::<VpnRuleSet>(
        conn,
        keys,
        "SELECT payload FROM vpn_rule_sets ORDER BY sort_order",
    )?;
    Ok(DbData {
        groups,
        vpn_profiles,
        vpn_rule_sets,
    })
}

/// 从应用目录加载全部业务实体
pub fn load_for_app(app: &AppHandle) -> Result<DbData, String> {
    let keys = crate::crypto::vault_keys(app)?;
    let conn = open(&db_path(app)?)?;
    load_all(&conn, &keys)
}

/// 把业务实体差异写入数据库
pub fn save_for_app(app: &AppHandle, prev: &DbData, next: &DbData) -> Result<(), String> {
    let keys = crate::crypto::vault_keys(app)?;
    let mut conn = open(&db_path(app)?)?;
    save_diff(&mut conn, &keys, prev, next)
}
```

注意：`write_simple_rows` 的 `deletes` 闭包捕获了 `next`，`&[T]` 借用与闭包同时存在没有问题；若编译器提示借用冲突，把 `deletes` 改成 `impl Fn(&T) -> bool` 的显式生命周期版本，或把“删除列表”先收集成 `Vec<String>` 再统一 DELETE。

- [ ] **Step 5: 运行测试确认通过**

Run:
```bash
cd src-tauri && cargo test --lib db::tests
```
Expected: Task 1 的 2 个 + 本任务 6 个，共 `8 passed`。

如果 `write_simple_rows` 出现借用/类型错误，最小修复方式是把删除判断内联进两个 `write_vpn_*` 函数，不再抽泛型。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/crypto.rs src-tauri/src/db.rs
git commit -m "feat(db): 加密载荷、全量读取与事务内 diff 写入"
```

---

### Task 3: `config.rs` 拆分设置与业务数据，`config.json` 只存 `AppSettings`

**Files:**
- Modify: `src-tauri/src/models.rs`（新增 `AppSettings`）
- Modify: `src-tauri/src/config.rs`（重写 load/save 与测试）

- [ ] **Step 1: 在 `models.rs` 新增 `AppSettings`**

在 `AppConfig` 定义之前插入：

```rust
/// 只随 config.json 保存的应用设置；业务数据改由 SQLite 承载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub default_exclude: Vec<String>,
    #[serde(default)]
    pub default_exclude_version: u32,
    /// 上次选中的 VPN 配置
    #[serde(default)]
    pub vpn_active_profile_id: String,
    /// 用户在设置页手动指定的应用可执行文件路径（Windows 专有设置）
    #[serde(default)]
    pub app_paths: Vec<crate::apps::models::AppPathOverride>,
    /// 注册免 UAC 计划任务时用的会话目录
    #[serde(default)]
    pub vpn_worker_session_dir: String,
    /// 全局默认附件上传目录（分组/项目未单独设置时使用）
    #[serde(default)]
    pub default_upload_dir: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            default_exclude: default_exclude_rules(),
            default_exclude_version: DEFAULT_EXCLUDE_VERSION,
            vpn_active_profile_id: String::new(),
            app_paths: vec![],
            vpn_worker_session_dir: String::new(),
            default_upload_dir: None,
        }
    }
}
```

- [ ] **Step 2: 写失败测试（新测试模块）**

把 `config.rs` 末尾的 `#[cfg(test)] mod tests { ... }` 整体替换为：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProjectGroup;

    fn group_from_json(json: &str) -> ProjectGroup {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn app_settings_default_uses_latest_exclude_rules() {
        let settings = AppSettings::default();
        assert_eq!(
            settings.default_exclude_version,
            crate::models::DEFAULT_EXCLUDE_VERSION
        );
        assert!(settings.default_exclude.iter().any(|r| r == ".*"));
    }

    #[test]
    fn app_settings_roundtrip_from_config() {
        let config = AppConfig {
            default_exclude: vec!["node_modules".to_string()],
            default_exclude_version: 7,
            vpn_active_profile_id: "p1".to_string(),
            vpn_worker_session_dir: "/tmp/session".to_string(),
            default_upload_dir: Some("/tmp/upload".to_string()),
            ..Default::default()
        };
        let settings = AppSettings::from_config(&config);
        let merged = settings.into_config(crate::db::DbData::default());
        assert_eq!(merged.default_exclude, config.default_exclude);
        assert_eq!(merged.default_exclude_version, 7);
        assert_eq!(merged.vpn_active_profile_id, "p1");
        assert_eq!(merged.vpn_worker_session_dir, "/tmp/session");
        assert_eq!(merged.default_upload_dir, config.default_upload_dir);
        assert!(merged.groups.is_empty());
    }

    #[test]
    fn mark_default_group_marks_named_group() {
        let mut config = AppConfig::default();
        assert!(!mark_default_group(&mut config));

        config.groups.push(group_from_json(r#"{"id":"g1","name":"默认分组"}"#));
        assert!(mark_default_group(&mut config));
        assert!(config.groups[0].is_default);
        assert!(!mark_default_group(&mut config));
    }

    #[test]
    fn mark_default_group_leaves_other_groups_alone() {
        let mut config = AppConfig::default();
        config.groups.push(group_from_json(r#"{"id":"g1","name":"工作"}"#));
        assert!(!mark_default_group(&mut config));
        assert!(!config.groups[0].is_default);
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run:
```bash
cd src-tauri && cargo test --lib config::tests
```
Expected: 编译失败（`AppSettings`、`AppSettings::from_config`、`into_config` 未定义）。

- [ ] **Step 4: 重写 `config.rs`**

用旧文件中的 `push_candidate`、`push_project_candidates`、`test_writable`、`find_writable_file_path` 四个辅助函数（保持原样，`find_writable_file_path` 内改调 `storage_candidates`）加上下面这段代码，拼成完整的新 `src-tauri/src/config.rs`（其余旧内容全部删除）：

```rust
use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;

use crate::db::DbData;
use crate::models::{AppConfig, AppSettings};

const STORE_KEY: &str = "app_config";

/// 设置内存缓存（config.json 不可用时的兜底）
static SETTINGS_CACHE: Mutex<Option<AppSettings>> = Mutex::new(None);

/// 上次从数据库加载的业务快照，作为保存时 diff 的基线
static DB_SNAPSHOT: Mutex<Option<DbData>> = Mutex::new(None);

fn get_settings_cache() -> AppSettings {
    SETTINGS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

fn set_settings_cache(settings: &AppSettings) {
    *SETTINGS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings.clone());
}

fn set_db_snapshot(snapshot: Option<DbData>) {
    *DB_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
}

fn db_snapshot() -> Option<DbData> {
    DB_SNAPSHOT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

impl AppSettings {
    /// 从完整配置中抽出设置字段
    pub fn from_config(config: &AppConfig) -> Self {
        AppSettings {
            default_exclude: config.default_exclude.clone(),
            default_exclude_version: config.default_exclude_version,
            vpn_active_profile_id: config.vpn_active_profile_id.clone(),
            app_paths: config.app_paths.clone(),
            vpn_worker_session_dir: config.vpn_worker_session_dir.clone(),
            default_upload_dir: config.default_upload_dir.clone(),
        }
    }

    /// 把设置与数据库实体合并成前端使用的完整配置
    pub fn into_config(self, data: DbData) -> AppConfig {
        AppConfig {
            groups: data.groups,
            projects: vec![],
            default_exclude: self.default_exclude,
            default_exclude_version: self.default_exclude_version,
            vpn_profiles: data.vpn_profiles,
            vpn_rule_sets: data.vpn_rule_sets,
            vpn_active_profile_id: self.vpn_active_profile_id,
            app_paths: self.app_paths,
            vpn_worker_session_dir: self.vpn_worker_session_dir,
            default_upload_dir: self.default_upload_dir,
        }
    }
}

/// 使用 Store 插件加载设置。
///
/// - `Ok(Some((设置, 是否为历史明文)))`：读取成功
/// - `Ok(None)`：Store 中没有设置
/// - `Err`：Store 中存在数据但无法解密/解析，调用方绝不能覆盖回写
fn load_settings_from_store(app: &AppHandle) -> Result<Option<(AppSettings, bool)>, String> {
    // 与旧实现相同，只是把 AppConfig 换成 AppSettings
    let store = match app.store("config.json") {
        Ok(store) => store,
        Err(_) => return Ok(None),
    };
    let value = match store.get(STORE_KEY) {
        Some(value) => value,
        None => return Ok(None),
    };
    match value {
        serde_json::Value::String(s) => {
            if crate::crypto::is_encrypted(&s) {
                let json = crate::crypto::decrypt_str(app, &s)?;
                let settings = serde_json::from_str::<AppSettings>(&json)
                    .map_err(|e| format!("解析设置失败: {}", e))?;
                Ok(Some((settings, false)))
            } else {
                let settings = serde_json::from_str::<AppSettings>(&s)
                    .map_err(|e| format!("解析设置失败: {}", e))?;
                Ok(Some((settings, true)))
            }
        }
        other => {
            let settings = serde_json::from_value::<AppSettings>(other)
                .map_err(|e| format!("解析设置失败: {}", e))?;
            Ok(Some((settings, true)))
        }
    }
}

/// 使用 Store 插件保存设置（以 SM4 密文信封写入）
fn save_settings_to_store(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let store = app
        .store("config.json")
        .map_err(|e| format!("创建 store 失败: {}", e))?;
    let json = serde_json::to_string(settings).map_err(|e| format!("序列化失败: {}", e))?;
    let envelope = crate::crypto::encrypt_str(app, &json)?;
    store.set(STORE_KEY, serde_json::Value::String(envelope));
    store.save().map_err(|e| format!("保存 store 失败: {}", e))?;
    Ok(())
}

/// 从文件系统加载设置（兜底），逻辑与旧版一致
fn load_settings_from_file(app: &AppHandle) -> Result<Option<(AppSettings, bool)>, String> {
    use std::fs;

    let mut saw_unreadable = false;
    for (_, dir) in storage_candidates(app) {
        let path = dir.join("config.json");
        if !path.exists() {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => {
                saw_unreadable = true;
                continue;
            }
        };
        let trimmed = content.trim();
        if crate::crypto::is_encrypted(trimmed) {
            if let Ok(json) = crate::crypto::decrypt_str(app, trimmed) {
                if let Ok(settings) = serde_json::from_str::<AppSettings>(&json) {
                    return Ok(Some((settings, false)));
                }
            }
            saw_unreadable = true;
            continue;
        }
        match serde_json::from_str::<AppSettings>(trimmed) {
            Ok(settings) => return Ok(Some((settings, true))),
            Err(_) => {
                saw_unreadable = true;
                continue;
            }
        }
    }
    if saw_unreadable {
        return Err("设置文件存在但无法读取或解密".to_string());
    }
    Ok(None)
}

/// 把设置写到文件系统（兜底）
fn save_settings_to_file(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    use std::fs;

    let path = find_writable_file_path(app).ok_or_else(|| "没有可写的设置目录".to_string())?;
    let json = serde_json::to_string(settings).map_err(|e| format!("序列化失败: {}", e))?;
    let envelope = crate::crypto::encrypt_str(app, &json)?;
    fs::write(&path, envelope).map_err(|e| format!("写入设置文件失败: {}", e))
}

/// 加载设置：Store → 文件 → 默认值
pub fn load_settings(app: &AppHandle) -> AppSettings {
    match load_settings_from_store(app) {
        Ok(Some((settings, was_plaintext))) => {
            if was_plaintext {
                let _ = save_settings_to_store(app, &settings);
            }
            set_settings_cache(&settings);
            return settings;
        }
        Err(err) => {
            eprintln!("[settings] 读取失败，使用内存/默认设置: {}", err);
            return get_settings_cache();
        }
        Ok(None) => {}
    }

    match load_settings_from_file(app) {
        Ok(Some((settings, _was_plaintext))) => {
            if save_settings_to_store(app, &settings).is_err() {
                let _ = save_settings_to_file(app, &settings);
            }
            set_settings_cache(&settings);
            return settings;
        }
        Err(err) => {
            eprintln!("[settings] 读取失败，使用内存/默认设置: {}", err);
            return get_settings_cache();
        }
        Ok(None) => {}
    }

    let settings = AppSettings::default();
    if save_settings(app, &settings).is_err() {
        eprintln!("[settings] 首次写入默认设置失败，仅保存在内存中");
    }
    settings
}

/// 保存设置：Store → 文件
pub fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    set_settings_cache(settings);
    let mut errors: Vec<String> = Vec::new();
    match save_settings_to_store(app, settings) {
        Ok(()) => return Ok(()),
        Err(e) => errors.push(format!("store: {}", e)),
    }
    match save_settings_to_file(app, settings) {
        Ok(()) => return Ok(()),
        Err(e) => errors.push(format!("file: {}", e)),
    }
    Err(format!(
        "设置保存失败（已保存在内存中，重启后会丢失）：{}",
        errors.join("; ")
    ))
}

/// 加载完整配置：设置 + 数据库业务实体
pub fn load_config(app: &AppHandle) -> AppConfig {
    let settings = load_settings(app);
    let data = match crate::db::load_for_app(app) {
        Ok(data) => {
            set_db_snapshot(Some(data.clone()));
            data
        }
        Err(err) => {
            eprintln!("[db] 读取失败，本次不写回数据库: {}", err);
            set_db_snapshot(None);
            DbData::default()
        }
    };
    let mut config = settings.into_config(data);
    mark_default_group(&mut config);
    config
}

/// 保存完整配置：设置写 config.json，业务实体按快照 diff 写数据库
pub fn save_config(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    let mut errors: Vec<String> = Vec::new();

    let settings = AppSettings::from_config(config);
    if let Err(err) = save_settings(app, &settings) {
        errors.push(format!("settings: {}", err));
    }

    match db_snapshot() {
        None => errors.push("本地数据库不可用，业务数据未写入".to_string()),
        Some(prev) => {
            let next = DbData::from_config(config);
            match crate::db::save_for_app(app, &prev, &next) {
                Ok(()) => set_db_snapshot(Some(next)),
                Err(err) => errors.push(format!("db: {}", err)),
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// 获取数据库文件路径（用于设置页展示）
pub fn get_config_path(app: &AppHandle) -> Result<String, String> {
    crate::db::db_path(app).map(|path| path.to_string_lossy().to_string())
}

/// 获取设置文件路径（用于设置页展示）
pub fn settings_path(app: &AppHandle) -> Result<String, String> {
    find_writable_file_path(app)
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "设置文件当前不可写（内存模式）".to_string())
}

/// 标记默认分组（兼容历史数据：旧数据没有 is_default 字段）
fn mark_default_group(config: &mut AppConfig) -> bool {
    if config.groups.iter().any(|g| g.is_default) {
        return false;
    }
    if let Some(group) = config.groups.iter_mut().find(|g| g.name == "默认分组") {
        group.is_default = true;
        return true;
    }
    false
}

/// 写入候选目录：优先可持久化的用户位置（应用数据/配置目录、用户主目录），
/// 最后才退到项目内目录（开发专用）。
pub(crate) fn storage_candidates(app: &AppHandle) -> Vec<(String, std::path::PathBuf)> {
    use std::env;
    use std::path::PathBuf;

    let mut candidates: Vec<(String, PathBuf)> = Vec::new();

    if let Ok(dir) = app.path().app_data_dir() {
        push_candidate(&mut candidates, "app_data_dir", dir);
    }
    if let Ok(dir) = app.path().app_config_dir() {
        push_candidate(&mut candidates, "app_config_dir", dir);
    }
    if let Ok(home) = app.path().home_dir() {
        push_candidate(
            &mut candidates,
            "home/.ws-workbench",
            home.join(".ws-workbench"),
        );
    }
    if let Ok(home) = env::var("HOME") {
        push_candidate(
            &mut candidates,
            "env_HOME/.ws-workbench",
            PathBuf::from(home).join(".ws-workbench"),
        );
    }
    push_project_candidates(&mut candidates);

    candidates
}
```

注意：`storage_candidates` 的函数体直接沿用旧 `get_write_candidates`（包含 `push_candidate` / `push_project_candidates` 的调用），只改名字与可见性；旧 `find_writable_file_path` 内部对 `get_write_candidates` 的调用同步改名。删除 `load_config_readonly`、`store_dir_writable`、`migrate_default_exclude`、`migrate_vpn_passwords`、`migrate_config`。

- [ ] **Step 5: 运行测试确认通过**

Run:
```bash
cd src-tauri && cargo test --lib config::tests
```
Expected: `4 passed`。

- [ ] **Step 6: 全量编译检查**

Run:
```bash
cd src-tauri && cargo check
```
Expected: 无错误；如提示 `DEFAULT_EXCLUDE_ADDITIONS` 或 `ProjectGroup` 未使用，删除对应 import。

- [ ] **Step 7: 提交**

```bash
git add src-tauri/src/models.rs src-tauri/src/config.rs
git commit -m "refactor(config): 拆分 AppSettings，业务数据改由 SQLite 保存"
```

---

### Task 4: `storage_info` 命令与设置页展示

**Files:**
- Modify: `src-tauri/src/commands.rs`（新增 `StorageInfo` + `storage_info`）
- Modify: `src-tauri/src/lib.rs`（注册命令）
- Modify: `src/components/settings/SettingsPanel.vue`

- [ ] **Step 1: 实现命令**

在 `commands.rs` 的 `get_config_path` 后面追加：

```rust
/// 本地存储信息（设置页展示用）
#[derive(serde::Serialize)]
pub struct StorageInfo {
    pub db_path: String,
    pub settings_path: String,
    pub ok: bool,
    pub error: String,
}

#[tauri::command]
pub fn storage_info(app: AppHandle) -> StorageInfo {
    let mut errors: Vec<String> = Vec::new();
    let db_path = match crate::db::check(&app) {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(err) => {
            errors.push(format!("数据库: {}", err));
            "—".to_string()
        }
    };
    let settings_path = match crate::config::settings_path(&app) {
        Ok(path) => path,
        Err(err) => {
            errors.push(format!("设置: {}", err));
            "—".to_string()
        }
    };
    StorageInfo {
        db_path,
        settings_path,
        ok: errors.is_empty(),
        error: errors.join("; "),
    }
}
```

- [ ] **Step 2: 注册命令**

`src-tauri/src/lib.rs` 的 `invoke_handler` 列表里，在 `commands::get_config_path,` 后加：
```rust
            commands::storage_info,
```

- [ ] **Step 3: 设置页展示**

`src/components/settings/SettingsPanel.vue`：

script 顶部（`configPath` 附近）加：
```ts
interface StorageInfo {
  db_path: string
  settings_path: string
  ok: boolean
  error: string
}
const storageInfo = ref<StorageInfo | null>(null)
```

`onMounted` 里读取配置路径之后加：
```ts
  try {
    storageInfo.value = await invoke<StorageInfo>('storage_info')
  } catch {
    storageInfo.value = null
  }
```

模板里把原来的「配置文件」行：
```vue
      <div class="info-row">
        <span class="info-label">配置文件</span>
        <span class="info-value mono">{{ configPath || '—' }}</span>
      </div>
```
替换为：
```vue
      <div class="info-row">
        <span class="info-label">数据库</span>
        <span class="info-value mono">{{ storageInfo?.db_path || configPath || '—' }}</span>
      </div>
      <div class="info-row">
        <span class="info-label">设置文件</span>
        <span class="info-value mono">{{ storageInfo?.settings_path || '—' }}</span>
      </div>
      <div v-if="storageInfo && !storageInfo.ok" class="info-row">
        <span class="info-label">存储状态</span>
        <span class="info-value">异常：{{ storageInfo.error }}</span>
      </div>
```

- [ ] **Step 4: 前端构建检查**

Run:
```bash
pnpm build
```
Expected: `vue-tsc` 与 `vite build` 均通过。

- [ ] **Step 5: 后端编译检查**

Run:
```bash
cd src-tauri && cargo check
```
Expected: 通过。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs src/components/settings/SettingsPanel.vue
git commit -m "feat(settings): 展示数据库与设置文件位置"
```

---

### Task 5: README 同步、全量测试与手动验收

**Files:**
- Modify: `README.md`（「配置与数据」章节）
- 无代码改动

- [ ] **Step 1: 更新 README**

找到「配置与数据」章节，改成新的事实：

```markdown
## 配置与数据

- 业务数据（分组、项目、笔记、附件元信息、VPN 配置、规则集）存在应用私有目录的 `ws-workbench.db`（SQLite）中，每行内容用 SM4 信封加密后写入 `payload` 列，标识与排序字段以明文保存。
- 应用设置（默认排除规则、上次选中的 VPN 配置、外部应用路径、默认上传目录等）仍存在同一目录的 `config.json`，整体加密。
- 附件正文以 `.wst` 密文文件存放在上传目录。
- 备份时一并复制 `ws-workbench.db`、`config.json` 与附件目录。
```

- [ ] **Step 2: 后端全量测试**

Run:
```bash
cd src-tauri && cargo test
```
Expected: 全部通过（此前已知与平台相关的失败项若仍存在，需在结果中说明，不新增失败）。

- [ ] **Step 3: 前端全量构建**

Run:
```bash
pnpm build
```
Expected: 通过。

- [ ] **Step 4: 手动验收（本地 GUI）**

Run:
```bash
pnpm exec tauri dev
```
按验收标准逐项检查：
1. 启动后在应用私有目录生成 `ws-workbench.db`；设置页显示数据库与设置文件路径。
2. 新建分组、项目，写笔记、上传一个附件、保存 VPN 配置与规则集。
3. 重启应用，数据仍在。
4. 关闭应用后执行 `strings <数据库路径> | grep 笔记内容`，不应出现明文。
5. 只改一条笔记再保存，确认其它行 `payload` 未变（可用 `sqlite3` 或脚本读取对比）。
6. 恢复原状后重启应用，确认运行正常（项目规则要求任务完成后重启应用验证）。

- [ ] **Step 5: 提交**

```bash
git add README.md
git commit -m "docs: 更新 SQLite 存储说明"
```

---

## 与 spec 的对应关系

- 存储范围 / 加密 / 不迁移 / 读写模型：Task 2、Task 3。
- `config.json` 只留设置、`AppConfig` 与命令不变：Task 3。
- `storage_info` 与设置页：Task 4。
- 验收标准 1–4、6：Task 2 单测 + Task 5 手动验收；验收标准 5：Task 3 的 DB 快照保护。
- 未实现项（SQLCipher、旧数据迁移、附件入库、细粒度查询、移除 config.json）与 spec 的 Out of Scope 一致。
