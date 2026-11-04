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
