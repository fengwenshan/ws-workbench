//! SQLite 本地数据库：业务实体（分组/项目/VPN 配置/规则集）存储层。
//!
//! 表里只放 `id`、`group_id`、`sort_order` 等少量明文列；实体内容序列化成
//! JSON 后用 [`crate::crypto`] 的信封（SM4-CBC + HMAC-SM3）加密存进 `payload`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tauri::AppHandle;

use crate::crypto::VaultKeys;
use crate::models::{AppConfig, ProjectConfig, ProjectGroup};
use crate::vpn::models::{VpnProfile, VpnRuleSet};

/// 数据库文件名，与 config.json 放在同一个可写目录下
pub const DB_FILE: &str = "ws-workbench.db";

/// 表结构版本
const SCHEMA_VERSION: i64 = 1;

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
    for (_, dir) in crate::config::storage_candidates(app) {
        if !dir.exists() && crate::util::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        if crate::util::fs::is_writable_dir(&dir) {
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
            created_at: "2026-11-04T00:00:00Z".to_string(),
            updated_at: "2026-11-04T00:00:00Z".to_string(),
            note: note.to_string(),
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
            attachments: vec![],
        }
    }

    fn sample_data() -> DbData {
        let profile: VpnProfile = serde_json::from_str(
            r#"{"id":"p1","name":"办公室","server":"vpn.example.com","username":"u"}"#,
        )
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
        assert_eq!(
            loaded.groups[0].projects[0].note,
            expected.groups[0].projects[0].note
        );
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
            conn.query_row(
                "SELECT payload FROM groups WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
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
        next.groups.remove(0);
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
}
