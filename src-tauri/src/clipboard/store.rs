//! 剪贴板 SQLite 存储：只做 DB 读写，加密/解密由调用方负责。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

/// 附件目录名（App 私有）
pub const ATTACHMENTS_DIR: &str = "clipboard-attachments";

#[derive(Debug, Clone)]
pub struct NewItem {
    pub id: String,
    pub kind: String,
    pub created_at: i64,
    pub content_hash: String,
    pub text_enc: String,
    pub html_enc: String,
    pub image_count: i64,
    pub byte_size: i64,
    /// 来源应用富文本私有表示的密文信封（见 `clipboard::stored_extra_flavors`）
    pub extra_enc: String,
}

#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub id: String,
    pub item_id: String,
    pub seq: i64,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub orig_size: i64,
    pub thumb_size: i64,
}

#[derive(Debug, Clone)]
pub struct ItemRow {
    pub id: String,
    pub kind: String,
    pub created_at: i64,
    pub text_enc: String,
    pub html_enc: String,
    pub image_count: i64,
    pub byte_size: i64,
    /// 来源应用富文本私有表示的密文信封（见 `clipboard::stored_extra_flavors`）
    pub extra_enc: String,
}

#[derive(Debug, Clone)]
pub struct AttachmentRow {
    pub id: String,
    pub item_id: String,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub orig_size: i64,
}

pub fn insert_item(
    conn: &Connection,
    item: &NewItem,
    attachments: &[NewAttachment],
) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开始事务失败: {e}"))?;
    tx.execute(
        "INSERT INTO clipboard_items
         (id, kind, created_at, content_hash, text_enc, html_enc, image_count, byte_size,
          extra_enc)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            item.id,
            item.kind,
            item.created_at,
            item.content_hash,
            item.text_enc,
            item.html_enc,
            item.image_count,
            item.byte_size,
            item.extra_enc
        ],
    )
    .map_err(|e| format!("写入剪贴板记录失败: {e}"))?;
    for att in attachments {
        tx.execute(
            "INSERT INTO clipboard_attachments
             (id, item_id, seq, mime, width, height, orig_size, thumb_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                att.id,
                att.item_id,
                att.seq,
                att.mime,
                att.width,
                att.height,
                att.orig_size,
                att.thumb_size
            ],
        )
        .map_err(|e| format!("写入剪贴板附件失败: {e}"))?;
    }
    tx.commit().map_err(|e| format!("提交事务失败: {e}"))
}

pub fn find_by_hash(conn: &Connection, hash: &str) -> Result<Option<ItemRow>, String> {
    conn.query_row(
        "SELECT id, kind, created_at, text_enc, html_enc, image_count, byte_size, extra_enc
         FROM clipboard_items WHERE content_hash = ?1 LIMIT 1",
        params![hash],
        row_to_item,
    )
    .map(Some)
    .or_else(|err| {
        if err == rusqlite::Error::QueryReturnedNoRows {
            Ok(None)
        } else {
            Err(err.to_string())
        }
    })
}

pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<ItemRow>, String> {
    conn.query_row(
        "SELECT id, kind, created_at, text_enc, html_enc, image_count, byte_size, extra_enc
         FROM clipboard_items WHERE id = ?1 LIMIT 1",
        params![id],
        row_to_item,
    )
    .map(Some)
    .or_else(|err| {
        if err == rusqlite::Error::QueryReturnedNoRows {
            Ok(None)
        } else {
            Err(err.to_string())
        }
    })
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        created_at: row.get(2)?,
        text_enc: row.get(3)?,
        html_enc: row.get(4)?,
        image_count: row.get(5)?,
        byte_size: row.get(6)?,
        extra_enc: row.get(7)?,
    })
}

/// 给已有记录补上来源应用的富文本私有表示（同样内容再次复制时可能才带上）
pub fn set_extra_enc(conn: &Connection, id: &str, extra_enc: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE clipboard_items SET extra_enc = ?2 WHERE id = ?1",
        params![id, extra_enc],
    )
    .map(|_| ())
    .map_err(|e| format!("更新富文本表示失败: {e}"))
}

pub fn touch(conn: &Connection, id: &str, created_at: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE clipboard_items SET created_at = ?2 WHERE id = ?1",
        params![id, created_at],
    )
    .map(|_| ())
    .map_err(|e| format!("更新时间失败: {e}"))
}

pub fn page(
    conn: &Connection,
    before: Option<(i64, String)>,
    limit: usize,
) -> Result<(Vec<ItemRow>, bool), String> {
    let limit = limit.clamp(1, 50);
    let mut rows = Vec::with_capacity(limit + 1);
    match before {
        Some((created, id)) => {
            let mut stmt = conn
                .prepare(
                    "SELECT id, kind, created_at, text_enc, html_enc, image_count, byte_size, extra_enc
                     FROM clipboard_items
                     WHERE (created_at < ?1) OR (created_at = ?1 AND id < ?2)
                     ORDER BY created_at DESC, id DESC LIMIT ?3",
                )
                .map_err(|e| e.to_string())?;
            let iter = stmt
                .query_map(params![created, id, (limit + 1) as i64], row_to_item)
                .map_err(|e| e.to_string())?;
            for row in iter {
                rows.push(row.map_err(|e| e.to_string())?);
            }
        }
        None => {
            let mut stmt = conn
                .prepare(
                    "SELECT id, kind, created_at, text_enc, html_enc, image_count, byte_size, extra_enc
                     FROM clipboard_items ORDER BY created_at DESC, id DESC LIMIT ?1",
                )
                .map_err(|e| e.to_string())?;
            let iter = stmt
                .query_map(params![(limit + 1) as i64], row_to_item)
                .map_err(|e| e.to_string())?;
            for row in iter {
                rows.push(row.map_err(|e| e.to_string())?);
            }
        }
    }
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    Ok((rows, has_more))
}

/// 供搜索使用：从新到旧逐条回调，回调返回 false 停止。
pub fn for_each_newest(
    conn: &Connection,
    mut f: impl FnMut(ItemRow) -> bool,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, created_at, text_enc, html_enc, image_count, byte_size, extra_enc
             FROM clipboard_items ORDER BY created_at DESC, id DESC",
        )
        .map_err(|e| e.to_string())?;
    let mut iter = stmt.query_map([], row_to_item).map_err(|e| e.to_string())?;
    while let Some(row) = iter.next() {
        let row = row.map_err(|e| e.to_string())?;
        if !f(row) {
            break;
        }
    }
    Ok(())
}

pub fn stats(conn: &Connection) -> Result<(i64, i64, i64), String> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(image_count), 0), COALESCE(SUM(byte_size), 0)
         FROM clipboard_items",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map_err(|e| format!("读取统计失败: {e}"))
}

pub fn attachments(conn: &Connection, item_id: &str) -> Result<Vec<AttachmentRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, item_id, mime, width, height, orig_size
             FROM clipboard_attachments WHERE item_id = ?1 ORDER BY seq",
        )
        .map_err(|e| e.to_string())?;
    let iter = stmt
        .query_map(params![item_id], row_to_attachment)
        .map_err(|e| e.to_string())?;
    iter.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn attachment(conn: &Connection, id: &str) -> Result<Option<AttachmentRow>, String> {
    conn.query_row(
        "SELECT id, item_id, mime, width, height, orig_size
         FROM clipboard_attachments WHERE id = ?1 LIMIT 1",
        params![id],
        row_to_attachment,
    )
    .map(Some)
    .or_else(|err| {
        if err == rusqlite::Error::QueryReturnedNoRows {
            Ok(None)
        } else {
            Err(err.to_string())
        }
    })
}

fn row_to_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRow> {
    Ok(AttachmentRow {
        id: row.get(0)?,
        item_id: row.get(1)?,
        mime: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        orig_size: row.get(5)?,
    })
}

pub fn delete_item(conn: &Connection, id: &str) -> Result<Vec<AttachmentRow>, String> {
    let attachments = attachments(conn, id)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM clipboard_attachments WHERE item_id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(attachments)
}

pub fn clear(conn: &Connection) -> Result<Vec<AttachmentRow>, String> {
    let attachments = all_attachments(conn)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clipboard_attachments", [])
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clipboard_items", [])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(attachments)
}

pub fn all_attachments(conn: &Connection) -> Result<Vec<AttachmentRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, item_id, mime, width, height, orig_size
             FROM clipboard_attachments",
        )
        .map_err(|e| e.to_string())?;
    let iter = stmt
        .query_map([], row_to_attachment)
        .map_err(|e| e.to_string())?;
    iter.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn attachments_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    // 和数据库同一个目录（`app_data_dir` 不可写时会回退到其它候选目录，
    // 这里跟着走，避免数据库写成功而图片写失败导致图文条目被丢弃）
    let dir = crate::db::storage_dir(app)?.join(ATTACHMENTS_DIR);
    crate::util::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn blob_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.wst"))
}

pub fn thumb_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.thumb.wst"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let conn = db::open(&path).unwrap();
        (dir, conn)
    }

    fn item(id: &str, hash: &str, created: i64) -> NewItem {
        NewItem {
            id: id.into(),
            kind: "text".into(),
            created_at: created,
            content_hash: hash.into(),
            text_enc: "v1.x".into(),
            html_enc: String::new(),
            image_count: 0,
            byte_size: 3,
            extra_enc: String::new(),
        }
    }

    fn att(id: &str, item_id: &str) -> NewAttachment {
        NewAttachment {
            id: id.into(),
            item_id: item_id.into(),
            seq: 0,
            mime: "image/png".into(),
            width: 2,
            height: 3,
            orig_size: 4,
            thumb_size: 5,
        }
    }

    #[test]
    fn inserts_and_pages_newest_first() {
        let (_dir, conn) = conn();
        for (id, created) in [("a", 1), ("b", 2), ("c", 3)] {
            insert_item(&conn, &item(id, id, created), &[]).unwrap();
        }
        let (page1, has_more) = page(&conn, None, 2).unwrap();
        assert_eq!(
            page1.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c", "b"]
        );
        assert!(has_more);
        let cursor = (page1[1].created_at, page1[1].id.clone());
        let (page2, has_more2) = page(&conn, Some(cursor), 2).unwrap();
        assert_eq!(page2.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert!(!has_more2);
    }

    #[test]
    fn finds_by_hash_and_touches() {
        let (_dir, conn) = conn();
        insert_item(&conn, &item("a", "h1", 1), &[]).unwrap();
        assert!(find_by_hash(&conn, "h1").unwrap().is_some());
        touch(&conn, "a", 99).unwrap();
        assert_eq!(find_by_hash(&conn, "h1").unwrap().unwrap().created_at, 99);
    }

    #[test]
    fn delete_item_returns_attachments_for_file_cleanup() {
        let (_dir, conn) = conn();
        let mut new = item("a", "h1", 1);
        new.image_count = 1;
        insert_item(&conn, &new, &[att("att1", "a")]).unwrap();

        let removed = delete_item(&conn, "a").unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, "att1");
        assert!(find_by_hash(&conn, "h1").unwrap().is_none());
        assert!(attachment(&conn, "att1").unwrap().is_none());
    }

    #[test]
    fn clear_returns_every_attachment() {
        let (_dir, conn) = conn();
        let mut a = item("a", "h1", 1);
        a.image_count = 1;
        let b = item("b", "h2", 2);
        insert_item(&conn, &a, &[att("att1", "a")]).unwrap();
        insert_item(&conn, &b, &[]).unwrap();

        let removed = clear(&conn).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(stats(&conn).unwrap(), (0, 0, 0));
    }
}
