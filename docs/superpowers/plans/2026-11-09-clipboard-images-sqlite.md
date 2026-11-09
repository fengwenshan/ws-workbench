# 剪贴板 v2（图文 + SQLite 加密历史）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把剪贴板历史从「纯文本 + 内存 20 条」升级为「文本 / 图片 / 图文 + SQLite 加密持久化 + 面板懒加载 + 管理页 + 附件级联删除」。

**Architecture:** macOS 用 `NSPasteboard.changeCount` 驱动变更检测，`arboard` 读 text/html/image；HTML 内嵌图片抽出后用 WST1 加密落盘，正文/HTML 用现有 `crypto` 信封加密写入 SQLite v2 新表；面板与管理页共用分页命令 + `wsclip://` 协议出缩略图；删除/清空级联删除附件文件。

**Tech Stack:** Rust（tauri 2.12、rusqlite 0.41、arboard 3.7、image 0.25、objc2-app-kit NSPasteboard）、Vue 3.5 + TS + Vite 6、ant-design-vue。

**Spec:** `docs/superpowers/specs/2026-11-09-clipboard-images-sqlite-design.md`

**通用命令（在仓库根执行）**

```bash
# Rust 单测（用仓库内 CARGO_HOME，避免写 ~/.cargo）
cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test clipboard -- --nocapture
# Rust 全量
cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test
# 前端类型检查 + 构建
pnpm build
```

---

## Phase A：后端存储与采集

### Task 1: SQLite schema v2 + 迁移

**Files:**
- Modify: `src-tauri/src/db.rs`（`SCHEMA_VERSION`、新增 `SCHEMA_SQL_V2`、`init_schema`）
- Test: `src-tauri/src/db.rs` 的 `#[cfg(test)]` 模块

- [ ] **Step 1: 写失败测试**

在 `db.rs` 测试模块加：

```rust
#[test]
fn migrates_v1_db_to_v2_and_creates_clipboard_tables() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("v1.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_SQL_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }

    let conn = open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    for table in ["clipboard_items", "clipboard_attachments"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test clipboard_tables -- --nocapture`
Expected: 编译失败（`SCHEMA_SQL_V1` 不存在）

- [ ] **Step 3: 实现**

`db.rs` 里把现有 `SCHEMA_SQL` 改名 `SCHEMA_SQL_V1`，`SCHEMA_VERSION = 2`，新增：

```rust
const SCHEMA_SQL_V2: &str = r#"
CREATE TABLE IF NOT EXISTS clipboard_items (
  id           TEXT PRIMARY KEY NOT NULL,
  kind         TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  text_enc     TEXT NOT NULL,
  html_enc     TEXT NOT NULL,
  image_count  INTEGER NOT NULL DEFAULT 0,
  byte_size    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_created
  ON clipboard_items(created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_hash
  ON clipboard_items(content_hash);

CREATE TABLE IF NOT EXISTS clipboard_attachments (
  id         TEXT PRIMARY KEY NOT NULL,
  item_id    TEXT NOT NULL,
  seq        INTEGER NOT NULL,
  mime       TEXT NOT NULL,
  width      INTEGER NOT NULL,
  height     INTEGER NOT NULL,
  orig_size  INTEGER NOT NULL,
  thumb_size INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_clipboard_attachments_item
  ON clipboard_attachments(item_id, seq);
"#;
```

`init_schema` 改为：

```rust
fn init_schema(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("读取数据库版本失败: {}", e))?;
    if version > SCHEMA_VERSION {
        return Err(format!("数据库版本过新（{}），请升级应用后再打开", version));
    }
    if version < 1 {
        conn.execute_batch(SCHEMA_SQL_V1)
            .map_err(|e| format!("创建数据库表失败: {}", e))?;
    }
    if version < 2 {
        conn.execute_batch(SCHEMA_SQL_V2)
            .map_err(|e| format!("创建剪贴板表失败: {}", e))?;
    }
    if version < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| format!("写入数据库版本失败: {}", e))?;
    }
    Ok(())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test`
Expected: 全部 PASS（老 db 测试 `load_all/save_diff` 不受影响）

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/db.rs
git commit -m "feat(db): 剪贴板表 schema v2 与迁移"
```

### Task 2: `clipboard/rich.rs` —— HTML 图片抽取 / 还原 / 哈希 / 上限

**Files:**
- Create: `src-tauri/src/clipboard/rich.rs`
- Modify: `src-tauri/src/clipboard.rs`（文件头加 `mod rich;`）

- [ ] **Step 1: 写测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_data_uri_image_and_rewrites_src() {
        let html = r#"<p>x</p><img src="data:image/png;base64,iVBORw0KGgo=" width="2">"#;
        let (stored, images) = extract_inline_images(html);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime, "image/png");
        assert!(images[0].placeholder.starts_with(PLACEHOLDER_PREFIX));
        assert!(stored.contains("wsclip-img://"));
        assert!(!stored.contains("data:image/png"));
    }

    #[test]
    fn keeps_remote_image_src() {
        let html = r#"<img src="https://example.com/a.png">"#;
        let (stored, images) = extract_inline_images(html);
        assert!(images.is_empty());
        assert_eq!(stored, html);
    }

    #[test]
    fn restore_replaces_placeholder_with_data_uri() {
        let stored = r#"<img src="wsclip-img://abcd">"#;
        let out = restore_html(stored, &[("abcd".into(), "image/png".into(), vec![1, 2, 3])]);
        assert!(out.contains("data:image/png;base64,"));
        assert!(!out.contains("wsclip-img://"));
    }

    #[test]
    fn restore_removes_img_when_attachment_missing() {
        let out = restore_html(r#"a<img src="wsclip-img://missing">b"#, &[]);
        assert_eq!(out, "ab");
    }

    #[test]
    fn strips_tags_for_search() {
        assert_eq!(strip_tags("<p>hello <b>world</b></p>"), "hello world");
    }

    #[test]
    fn hash_changes_with_images() {
        let a = content_hash("t", "<p>h</p>", &["1".into()]);
        let b = content_hash("t", "<p>h</p>", &["2".into()]);
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test rich:: -- --nocapture`
Expected: FAIL（模块不存在）

- [ ] **Step 3: 实现**

```rust
//! 剪贴板 HTML 图文处理：抽取 data URI 图片、回贴还原、搜索去标签、内容哈希。

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use sm3::{Digest, Sm3};

/// 存储 HTML 里的图片占位符前缀
pub const PLACEHOLDER_PREFIX: &str = "wsclip-img://";

/// 从 HTML 里抽出的内嵌图片
pub struct ExtractedImage {
    pub mime: String,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// 写到存储 HTML 里的占位符，形如 `wsclip-img://<随机 id>`
    pub placeholder: String,
}

/// 扫 HTML 里的 `<img ...>`，把 `data:image/...;base64,...` 换成占位符。
/// 返回（改写后的 HTML，抽出的图片）。远程 URL / file URL 原样保留。
pub fn extract_inline_images(html: &str) -> (String, Vec<ExtractedImage>) {
    let mut out = String::with_capacity(html.len());
    let mut images = Vec::new();
    let mut rest = html;

    while let Some(start) = find_img_start(rest) {
        let (before, from_tag) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = from_tag.find('>') else {
            out.push_str(from_tag);
            return (out, images);
        };
        let tag = &from_tag[..=end];
        match replace_data_uri(tag) {
            Some((new_tag, image)) => {
                out.push_str(&new_tag);
                images.push(image);
            }
            None => out.push_str(tag),
        }
        rest = &from_tag[end + 1..];
    }
    out.push_str(rest);
    (out, images)
}

fn find_img_start(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    lower.find("<img")
}

/// 在单个 img 标签里找到 src 的 data URI 并解码。
fn replace_data_uri(tag: &str) -> Option<(String, ExtractedImage)> {
    let src = find_src_range(tag)?;
    let value = &tag[src.0..src.1];
    let rest = value.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    let mime = meta.trim_end_matches(";base64").to_string();
    let bytes = B64.decode(payload.trim()).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .unwrap_or((0, 0));
    let placeholder = format!("{PLACEHOLDER_PREFIX}{}", uuid::Uuid::new_v4());
    let mut new_tag = String::with_capacity(tag.len());
    new_tag.push_str(&tag[..src.0]);
    new_tag.push_str(&placeholder);
    new_tag.push_str(&tag[src.1..]);
    Some((
        new_tag,
        ExtractedImage { mime, bytes, width, height, placeholder },
    ))
}

/// 返回 src 属性值在 tag 里的字节范围（不含引号）。
fn find_src_range(tag: &str) -> Option<(usize, usize)> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(index) = lower[from..].find("src") {
        let attr = from + index;
        let after = attr + 3;
        let mut cursor = after;
        let bytes = tag.as_bytes();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            from = after;
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        return match bytes.get(cursor) {
            Some(&quote @ (b'"' | b'\'')) => {
                let start = cursor + 1;
                let end = tag[start..].find(quote as char).map(|i| start + i)?;
                Some((start, end))
            }
            Some(_) => {
                let start = cursor;
                let end = tag[start..]
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .map(|i| start + i)
                    .unwrap_or(tag.len());
                Some((start, end))
            }
            None => None,
        };
    }
    None
}

/// 回贴：把占位符换成 data URI；附件缺失时删掉整个 img 标签。
pub fn restore_html(html: &str, images: &[(String, String, Vec<u8>)]) -> String {
    let mut out = html.to_string();
    for (id, mime, bytes) in images {
        let placeholder = format!("{PLACEHOLDER_PREFIX}{id}");
        let data_uri = format!("data:{mime};base64,{}", B64.encode(bytes));
        out = out.replace(&placeholder, &data_uri);
    }
    // 剩余的占位符对应缺失附件：抹掉所在 img 标签
    loop {
        let Some(pos) = out.find(PLACEHOLDER_PREFIX) else { break };
        let start = out[..pos].rfind("<img").unwrap_or(pos);
        let end = out[pos..].find('>').map(|i| pos + i + 1).unwrap_or(out.len());
        out.replace_range(start..end, "");
    }
    out
}

/// 搜索用：粗暴去标签（保留文字与空白）。
pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// 内容哈希：文本 + HTML + 各图片哈希，用于去重。
pub fn content_hash(text: &str, html: &str, image_hashes: &[String]) -> String {
    let mut hasher = Sm3::new();
    hasher.update(b"v1\n");
    hasher.update(text.as_bytes());
    hasher.update(b"\n");
    hasher.update(html.as_bytes());
    hasher.update(b"\n");
    for hash in image_hashes {
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}
```

注意：`image` crate 的 `ImageReader` 位于 `image::ImageReader`（0.25），若编译不过改用 `image::io::Reader` 的对应写法。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test rich:: -- --nocapture`
Expected: 6 passed

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/clipboard.rs src-tauri/src/clipboard/rich.rs
git commit -m "feat(clipboard): HTML 图文抽取/还原与内容哈希"
```

### Task 3: `clipboard/rich.rs` —— PNG 编码与缩略图

**Files:**
- Modify: `src-tauri/src/clipboard/rich.rs`

- [ ] **Step 1: 写测试**

```rust
#[test]
fn thumbnail_keeps_aspect_and_caps_edge() {
    let img = image::RgbaImage::from_pixel(1000, 500, image::Rgba([255, 0, 0, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let thumb = png_thumbnail(&png, 320).unwrap();
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&thumb))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!((w, h), (320, 160));
}

#[test]
fn encode_png_from_rgba_roundtrips() {
    let rgba = vec![255u8; 4 * 4 * 4];
    let png = encode_png_rgba(&rgba, 4, 4).unwrap();
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&png))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!((w, h), (4, 4));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test thumbnail_keeps_aspect -- --nocapture`
Expected: 函数不存在

- [ ] **Step 3: 实现**

```rust
/// 解码任意支持格式 → 最长边 max_edge 的 PNG 缩略图。
pub fn png_thumbnail(bytes: &[u8], max_edge: u32) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = image.thumbnail(max_edge, max_edge);
    let mut out = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// 把 RGBA 原始像素编码为 PNG。
pub fn encode_png_rgba(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}
```

- [ ] **Step 4: 跑测试确认通过并提交**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test rich:: -- --nocapture`
Expected: 8 passed

```bash
git add src-tauri/src/clipboard/rich.rs
git commit -m "feat(clipboard): PNG 编码与缩略图"
```

### Task 4: `clipboard/store.rs` —— 数据库操作与附件目录

**Files:**
- Create: `src-tauri/src/clipboard/store.rs`
- Modify: `src-tauri/src/clipboard.rs`（`mod store;`）
- Test: 同文件 `#[cfg(test)]`

- [ ] **Step 1: 写测试**（临时目录 + 临时 DB；不需要 AppHandle）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> (tempfile::TempDir, rusqlite::Connection) {
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
        }
    }

    #[test]
    fn inserts_and_pages_newest_first() {
        let (_dir, conn) = conn();
        for (id, created) in [("a", 1), ("b", 2), ("c", 3)] {
            insert_item(&conn, &item(id, id, created), &[]).unwrap();
        }
        let (page, has_more) = page(&conn, None, 2).unwrap();
        assert_eq!(page.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c", "b"]);
        assert!(has_more);
        let cursor = (page[1].created_at, page[1].id.clone());
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
        let att = NewAttachment {
            id: "att1".into(),
            item_id: "a".into(),
            seq: 0,
            mime: "image/png".into(),
            width: 2,
            height: 3,
            orig_size: 4,
            thumb_size: 5,
        };
        insert_item(&conn, &new, &[att]).unwrap();
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
        let mut b = item("b", "h2", 2);
        b.image_count = 0;
        insert_item(&conn, &a, &[NewAttachment {
            id: "att1".into(), item_id: "a".into(), seq: 0,
            mime: "image/png".into(), width: 1, height: 1, orig_size: 1, thumb_size: 1,
        }]).unwrap();
        insert_item(&conn, &b, &[]).unwrap();
        let removed = clear(&conn).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(stats(&conn).unwrap(), (0, 0, 0));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test store:: -- --nocapture`
Expected: 模块不存在

- [ ] **Step 3: 实现**

结构体与函数（SQL 全部参数化）：

```rust
//! 剪贴板 SQLite 存储：只做 DB 读写，加密/解密由调用方负责。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::db;

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
    pub content_hash: String,
    pub text_enc: String,
    pub html_enc: String,
    pub image_count: i64,
    pub byte_size: i64,
}

#[derive(Debug, Clone)]
pub struct AttachmentRow {
    pub id: String,
    pub item_id: String,
    pub seq: i64,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub orig_size: i64,
    pub thumb_size: i64,
}

pub fn insert_item(conn: &Connection, item: &NewItem, attachments: &[NewAttachment]) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO clipboard_items
         (id, kind, created_at, content_hash, text_enc, html_enc, image_count, byte_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![item.id, item.kind, item.created_at, item.content_hash, item.text_enc, item.html_enc, item.image_count, item.byte_size],
    )
    .map_err(|e| format!("写入剪贴板记录失败: {e}"))?;
    for att in attachments {
        tx.execute(
            "INSERT INTO clipboard_attachments
             (id, item_id, seq, mime, width, height, orig_size, thumb_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![att.id, att.item_id, att.seq, att.mime, att.width, att.height, att.orig_size, att.thumb_size],
        )
        .map_err(|e| format!("写入剪贴板附件失败: {e}"))?;
    }
    tx.commit().map_err(|e| e.to_string())
}

pub fn find_by_hash(conn: &Connection, hash: &str) -> Result<Option<ItemRow>, String> {
    conn.query_row(
        "SELECT id, kind, created_at, content_hash, text_enc, html_enc, image_count, byte_size
         FROM clipboard_items WHERE content_hash = ?1 LIMIT 1",
        params![hash],
        row_to_item,
    )
    .map(Some)
    .or_else(|err| if err == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(err.to_string()) })
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        id: row.get(0)?, kind: row.get(1)?, created_at: row.get(2)?,
        content_hash: row.get(3)?, text_enc: row.get(4)?, html_enc: row.get(5)?,
        image_count: row.get(6)?, byte_size: row.get(7)?,
    })
}

pub fn touch(conn: &Connection, id: &str, created_at: i64) -> Result<(), String> {
    conn.execute("UPDATE clipboard_items SET created_at = ?2 WHERE id = ?1", params![id, created_at])
        .map(|_| ())
        .map_err(|e| format!("更新时间失败: {e}"))
}

pub fn page(conn: &Connection, before: Option<(i64, String)>, limit: usize) -> Result<(Vec<ItemRow>, bool), String> {
    let limit = limit.clamp(1, 50);
    let mut rows = Vec::with_capacity(limit + 1);
    match before {
        Some((created, id)) => {
            let mut stmt = conn.prepare(
                "SELECT id, kind, created_at, content_hash, text_enc, html_enc, image_count, byte_size
                 FROM clipboard_items
                 WHERE (created_at < ?1) OR (created_at = ?1 AND id < ?2)
                 ORDER BY created_at DESC, id DESC LIMIT ?3",
            ).map_err(|e| e.to_string())?;
            let iter = stmt.query_map(params![created, id, (limit + 1) as i64], row_to_item).map_err(|e| e.to_string())?;
            for row in iter { rows.push(row.map_err(|e| e.to_string())?); }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT id, kind, created_at, content_hash, text_enc, html_enc, image_count, byte_size
                 FROM clipboard_items ORDER BY created_at DESC, id DESC LIMIT ?1",
            ).map_err(|e| e.to_string())?;
            let iter = stmt.query_map(params![(limit + 1) as i64], row_to_item).map_err(|e| e.to_string())?;
            for row in iter { rows.push(row.map_err(|e| e.to_string())?); }
        }
    }
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    Ok((rows, has_more))
}

/// 供搜索使用：从新到旧逐条回调，回调返回 false 停止。
pub fn for_each_newest(conn: &Connection, mut f: impl FnMut(ItemRow) -> bool) -> Result<(), String> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, created_at, content_hash, text_enc, html_enc, image_count, byte_size
         FROM clipboard_items ORDER BY created_at DESC, id DESC",
    ).map_err(|e| e.to_string())?;
    let mut iter = stmt.query_map([], row_to_item).map_err(|e| e.to_string())?;
    while let Some(row) = iter.next() {
        let row = row.map_err(|e| e.to_string())?;
        if !f(row) { break; }
    }
    Ok(())
}

pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<ItemRow>, String> {
    conn.query_row(
        "SELECT id, kind, created_at, content_hash, text_enc, html_enc, image_count, byte_size
         FROM clipboard_items WHERE id = ?1 LIMIT 1",
        params![id],
        row_to_item,
    )
    .map(Some)
    .or_else(|err| if err == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(err.to_string()) })
}

pub fn stats(conn: &Connection) -> Result<(i64, i64, i64), String> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(image_count), 0), COALESCE(SUM(byte_size), 0) FROM clipboard_items",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).map_err(|e| format!("读取统计失败: {e}"))
}

pub fn attachments(conn: &Connection, item_id: &str) -> Result<Vec<AttachmentRow>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, item_id, seq, mime, width, height, orig_size, thumb_size
         FROM clipboard_attachments WHERE item_id = ?1 ORDER BY seq",
    ).map_err(|e| e.to_string())?;
    let iter = stmt.query_map(params![item_id], row_to_attachment).map_err(|e| e.to_string())?;
    iter.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn attachment(conn: &Connection, id: &str) -> Result<Option<AttachmentRow>, String> {
    conn.query_row(
        "SELECT id, item_id, seq, mime, width, height, orig_size, thumb_size
         FROM clipboard_attachments WHERE id = ?1 LIMIT 1",
        params![id],
        row_to_attachment,
    )
    .map(Some)
    .or_else(|err| if err == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(err.to_string()) })
}

fn row_to_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRow> {
    Ok(AttachmentRow {
        id: row.get(0)?, item_id: row.get(1)?, seq: row.get(2)?, mime: row.get(3)?,
        width: row.get(4)?, height: row.get(5)?, orig_size: row.get(6)?, thumb_size: row.get(7)?,
    })
}

pub fn delete_item(conn: &Connection, id: &str) -> Result<Vec<AttachmentRow>, String> {
    let attachments = attachments(conn, id)?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clipboard_attachments WHERE item_id = ?1", params![id]).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clipboard_items WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(attachments)
}

pub fn clear(conn: &Connection) -> Result<Vec<AttachmentRow>, String> {
    let attachments = all_attachments(conn)?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clipboard_attachments", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clipboard_items", []).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(attachments)
}

pub fn all_attachments(conn: &Connection) -> Result<Vec<AttachmentRow>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, item_id, seq, mime, width, height, orig_size, thumb_size
         FROM clipboard_attachments",
    ).map_err(|e| e.to_string())?;
    let iter = stmt.query_map([], row_to_attachment).map_err(|e| e.to_string())?;
    iter.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn attachments_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app.path().app_data_dir().map_err(|e| format!("获取应用数据目录失败: {e}"))?;
    let dir = base.join(ATTACHMENTS_DIR);
    crate::util::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn blob_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.wst")) }
pub fn thumb_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.thumb.wst")) }
```

- [ ] **Step 4: 跑测试确认通过并提交**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test store:: -- --nocapture`
Expected: 4 passed

```bash
git add src-tauri/src/clipboard.rs src-tauri/src/clipboard/store.rs
git commit -m "feat(clipboard): SQLite 存储层与附件目录"
```

### Task 5: `clipboard.rs` 采集运行时重写（changeCount + 加密入库 + 去重）

**Files:**
- Rewrite: `src-tauri/src/clipboard.rs`（删除 `ClipboardHistory`/`MAX_ITEMS`，保留面板定位与 `classify`）

- [ ] **Step 1: 先保留/迁移可测纯函数**

`classify` 的既有 8 个测试保留；新增：

```rust
#[test]
fn hotkey_debounce_allows_first_and_blocks_repeats() {
    assert!(allow_hotkey(0, 1_000));
    assert!(allow_hotkey(1_000, 1_400));
    assert!(!allow_hotkey(1_000, 1_100));
}
```

`allow_hotkey(last_ms, now) -> bool { now.saturating_sub(last_ms) >= HOTKEY_DEBOUNCE_MS }`

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test hotkey_debounce -- --nocapture` → 先失败

- [ ] **Step 2: 实现运行时结构**

关键代码（保持面板定位函数不动）：

```rust
pub const HOTKEY_DEBOUNCE_MS: u64 = 300;
const POLL_INTERVAL: Duration = Duration::from_millis(600);
pub const MAX_TEXT_CHARS: usize = 200_000;
pub const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_IMAGES_PER_ITEM: usize = 50;
pub const MAX_ITEM_BYTES: usize = 24 * 1024 * 1024;
pub const THUMB_MAX_EDGE: u32 = 320;
pub const PAGE_SIZE: usize = 20;
pub const SEARCH_LIMIT: usize = 100;

pub struct ClipboardRuntime {
    pub(crate) last_change_count: AtomicI64,
    pub(crate) last_hotkey_ms: AtomicU64,
    pub(crate) capture_lock: Mutex<()>,
}

impl ClipboardRuntime {
    pub fn new() -> Self {
        Self {
            last_change_count: AtomicI64::new(-1),
            last_hotkey_ms: AtomicU64::new(0),
            capture_lock: Mutex::new(()),
        }
    }
}

#[cfg(target_os = "macos")]
fn change_count() -> Option<i64> {
    use objc2_app_kit::NSPasteboard;
    Some(NSPasteboard::generalPasteboard().changeCount() as i64)
}

pub fn start_poller(app: AppHandle) {
    std::thread::Builder::new().name("ws-clipboard-poller".into()).spawn(move || {
        // 启动先收一次
        let _ = capture_once(&app, true);
        loop {
            std::thread::sleep(POLL_INTERVAL);
            if app.try_state::<ClipboardRuntime>().is_none() { return; }
            let _ = capture_once(&app, false);
        }
    }).ok();
}

fn now_ms() -> i64 {
    crate::util::time::now_utc_millis() as i64
}

fn sm3_hex(bytes: &[u8]) -> String {
    use sm3::{Digest, Sm3};
    let mut hasher = Sm3::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn cleanup_blobs(dir: &Path, attachments: &[store::NewAttachment]) {
    for att in attachments {
        let _ = std::fs::remove_file(store::blob_path(dir, &att.id));
        let _ = std::fs::remove_file(store::thumb_path(dir, &att.id));
    }
}

fn log_capture_err(stage: &str, err: &str) {
    eprintln!("[clipboard] capture failed ({stage}): {err}");
}

/// 打开剪贴板用数据库连接（路径沿用 `db::db_path`）
pub(crate) fn db_conn(app: &AppHandle) -> Result<rusqlite::Connection, String> {
    let path = crate::db::db_path(app)?;
    crate::db::open(&path)
}

/// 返回是否写入了新记录。`force` 用于启动 / 面板唤出。
pub fn capture_once(app: &AppHandle, force: bool) -> bool {
    let Some(runtime) = app.try_state::<ClipboardRuntime>() else { return false };
    let _guard = runtime.capture_lock.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(target_os = "macos")]
    {
        let Some(count) = change_count() else { return false };
        if !force && count == runtime.last_change_count.load(Ordering::Relaxed) { return false; }
        runtime.last_change_count.store(count, Ordering::Relaxed);
    }
    read_and_store(app)
}
```

`read_and_store` 流程（完整实现）：

```rust
fn read_and_store(app: &AppHandle) -> bool {
    let mut clipboard = match arboard::Clipboard::new() { Ok(c) => c, Err(err) => { log_capture_err("init", &err.to_string()); return false; } };
    let text = clipboard.get().text().unwrap_or_default();
    let html = clipboard.get().html().unwrap_or_default();
    let image = clipboard.get().image().ok();
    if text.chars().count() > MAX_TEXT_CHARS { return false; }
    if text.trim().is_empty() && html.is_empty() && image.is_none() { return false; }

    let mut images = Vec::<rich::ExtractedImage>::new();
    let mut stored_html = String::new();
    if !html.is_empty() && html.len() <= MAX_HTML_BYTES {
        let (rewritten, extracted) = rich::extract_inline_images(&html);
        stored_html = rewritten;
        images = extracted.into_iter().take(MAX_IMAGES_PER_ITEM)
            .filter(|img| img.bytes.len() <= MAX_IMAGE_BYTES)
            .collect();
    }

    let mut kind = if !images.is_empty() { "rich".to_string() }
        else if !text.trim().is_empty() { classify(&text).to_string() }
        else { "image".to_string() };

    // 纯图片：RGBA → PNG
    let mut image_item: Option<rich::ExtractedImage> = None;
    if images.is_empty() && text.trim().is_empty() {
        if let Some(img) = image {
            match rich::encode_png_rgba(img.bytes.as_ref(), img.width as u32, img.height as u32) {
                Some(png) => {
                    if png.len() <= MAX_IMAGE_BYTES {
                        image_item = Some(rich::ExtractedImage { mime: "image/png".into(), width: img.width as u32, height: img.height as u32, bytes: png });
                        kind = "image".into();
                    }
                }
                None => { log_capture_err("png", "encode failed"); return false; }
            }
        }
    }
    if kind == "image" && image_item.is_none() { return false; }
    if stored_html.is_empty() && text.trim().is_empty() && image_item.is_none() { return false; }

    // 图片列表（含纯图片条目）
    let mut all_images = images;
    if let Some(img) = image_item { all_images.push(img); }
    if all_images.is_empty() { kind = classify(&text).to_string(); }

    // 单条总量上限
    let total = text.len() + stored_html.len()
        + all_images.iter().map(|img| img.bytes.len()).sum::<usize>();
    if total > MAX_ITEM_BYTES { return false; }

    // 附件元数据 + 哈希
    let mut attachments = Vec::new();
    let mut hashes = Vec::new();
    let dir = match store::attachments_dir(app) { Ok(d) => d, Err(err) => { log_capture_err("dir", &err); return false; } };
    for (seq, image) in all_images.iter().enumerate() {
        let att_id = uuid::Uuid::new_v4().to_string();
        let blob = match filecrypto::encrypt_bytes(app, &image.bytes) { Ok(b) => b, Err(err) => { log_capture_err("encrypt", &err); cleanup_blobs(&dir, &attachments); return false; } };
        let thumb = rich::png_thumbnail(&image.bytes, THUMB_MAX_EDGE);
        let thumb_enc = thumb.as_ref().and_then(|t| filecrypto::encrypt_bytes(app, t).ok());
        let Some(thumb_enc) = thumb_enc else { cleanup_blobs(&dir, &attachments); return false; };
        if write_atomic(&store::blob_path(&dir, &att_id), &blob).is_err()
            || write_atomic(&store::thumb_path(&dir, &att_id), &thumb_enc).is_err() {
            cleanup_blobs(&dir, &attachments);
            return false;
        }
        // 把抽取时的随机占位符换成真正的 attachment_id
        stored_html = stored_html.replace(
            &image.placeholder,
            &format!("{}{att_id}", rich::PLACEHOLDER_PREFIX),
        );
        attachments.push(store::NewAttachment { id: att_id, item_id: String::new(), seq: seq as i64, mime: image.mime.clone(), width: image.width as i64, height: image.height as i64, orig_size: image.bytes.len() as i64, thumb_size: thumb.as_ref().map(|t| t.len()).unwrap_or(0) as i64 });
        hashes.push(sm3_hex(&image.bytes));
    }
    // 文本/HTML 加密
    let text_enc = if text.is_empty() { String::new() } else { match crypto::encrypt_str(app, &text) { Ok(v) => v, Err(err) => { log_capture_err("encrypt-text", &err); cleanup_blobs(&dir, &attachments); return false; } } };
    let html_enc = if stored_html.is_empty() { String::new() } else { match crypto::encrypt_str(app, &stored_html) { Ok(v) => v, Err(err) => { log_capture_err("encrypt-html", &err); cleanup_blobs(&dir, &attachments); return false; } } };
    let hash = rich::content_hash(&text, &stored_html, &hashes);

    // 去重
    let conn = match db_conn(app) { Ok(c) => c, Err(err) => { log_capture_err("db", &err); cleanup_blobs(&dir, &attachments); return false; } };
    if let Ok(Some(existing)) = store::find_by_hash(&conn, &hash) {
        let _ = store::touch(&conn, &existing.id, now_ms());
        cleanup_blobs(&dir, &attachments);
        let _ = app.emit(EVENT_CHANGED, ());
        return true;
    }
    // 入库
    let item_id = uuid::Uuid::new_v4().to_string();
    for att in &mut attachments { att.item_id = item_id.clone(); }
    let new_item = store::NewItem { id: item_id, kind, created_at: now_ms(), content_hash: hash, text_enc, html_enc, image_count: attachments.len() as i64, byte_size: (text.len() + stored_html.len()) as i64 + attachments.iter().map(|a| a.orig_size).sum::<i64>() };
    if let Err(err) = store::insert_item(&conn, &new_item, &attachments) {
        log_capture_err("insert", &err);
        cleanup_blobs(&dir, &attachments);
        return false;
    }
    let _ = app.emit(EVENT_CHANGED, ());
    true
}
```

实现注意：`extract_inline_images` 需要把占位符 id 与图片一起返回，便于把 `attachment_id` 塞回 HTML；把 Task 2 的返回类型改为 `(String, Vec<ExtractedImage>)`，其中 `ExtractedImage` 增加 `placeholder: String` 字段；`read_and_store` 在加密图片后执行 `stored_html = stored_html.replace(&image.placeholder, &format!("wsclip-img://{att_id}"))`。

- [ ] **Step 3: 面板显隐 / 快捷键行为**

```rust
/// 快捷键处理：去抖 + 只负责显示/聚焦 + 异步补采。
pub fn on_hotkey(app: &AppHandle) {
    let Some(runtime) = app.try_state::<ClipboardRuntime>() else { return };
    let now = now_ms() as u64;
    let last = runtime.last_hotkey_ms.load(Ordering::Relaxed);
    if !allow_hotkey(last, now) { return; }
    runtime.last_hotkey_ms.store(now, Ordering::Relaxed);
    show_panel(app);
    let app = app.clone();
    std::thread::spawn(move || { let _ = capture_once(&app, true); });
}

pub fn show_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    activate_app(app);
    if let Some((x, y, logical)) = panel_position(app, &window) {
        if logical {
            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
        } else {
            let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
        }
    }
    let _ = window.show();
    let _ = window.set_focus();
    let _ = app.emit(EVENT_SHOWN, ());
}

pub fn hide_panel(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(PANEL_LABEL) {
        let _ = window.hide();
    }
}

/// 自己写回剪贴板后调用：把已见 changeCount 同步到当前值。
pub fn note_self_write(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let (Some(runtime), Some(count)) = (app.try_state::<ClipboardRuntime>(), change_count()) {
        runtime.last_change_count.store(count, Ordering::Relaxed);
    }
}
```

- [ ] **Step 4: 编译 + 跑测试**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test clipboard -- --nocapture`
Expected: classify + debounce 等单测 PASS（采集链路依赖真实剪贴板，手动验收）

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/clipboard.rs src-tauri/src/clipboard/rich.rs
git commit -m "feat(clipboard): changeCount 采集与加密入库"
```

### Task 6: 命令层 + `wsclip` 协议 + 注册

**Files:**
- Rewrite: `src-tauri/src/commands/clipboard.rs`
- Modify: `src-tauri/src/lib.rs`（`manage(ClipboardRuntime)`、快捷键 handler 调 `on_hotkey`、注册 `wsclip` 协议、注册新命令）
- Modify: `src-tauri/Cargo.toml`（依赖）、`src-tauri/src/commands/mod.rs`（导出不变）

- [ ] **Step 1: 加依赖**

```toml
[target.'cfg(target_os = "macos")'.dependencies]
arboard = "3.7"
image = { version = "0.25", default-features = false, features = ["png", "jpeg", "gif", "webp"] }
# objc2-app-kit features 增加 "NSPasteboard"
```

- [ ] **Step 2: 命令实现**（`commands/clipboard.rs`）

文件头需要的 use：

```rust
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::clipboard::{self, rich, store};
use crate::{crypto, filecrypto};
```

```rust
#[tauri::command]
pub async fn clipboard_page(app: AppHandle, before_created_at: Option<i64>, before_id: Option<String>, limit: Option<u32>) -> Result<ClipboardPage, String> {
    tauri::async_runtime::spawn_blocking(move || page_impl(&app, before_created_at, before_id, limit))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_search(app: AppHandle, query: String, limit: Option<u32>) -> Result<Vec<ClipboardItemSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || search_impl(&app, query, limit))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_stats(app: AppHandle) -> Result<ClipboardStats, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_conn(&app)?;
        let (items, images, bytes) = store::stats(&conn)?;
        Ok(ClipboardStats { items, images, bytes })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_copy(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_conn(&app)?;
        let row = store::find_by_id(&conn, &id)?.ok_or_else(|| "这条剪贴板记录已经不存在了".to_string())?;
        write_item_to_clipboard(&app, &conn, &row)?;
        clipboard::note_self_write(&app);
        Ok(())
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_copy_image(app: AppHandle, attachment_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = store::attachments_dir(&app)?;
        let conn = open_conn(&app)?;
        let att = store::attachment(&conn, &attachment_id)?
            .ok_or_else(|| "图片附件不存在".to_string())?;
        let blob = std::fs::read(store::blob_path(&dir, &att.id))
            .map_err(|e| format!("读取图片附件失败: {e}"))?;
        let bytes = filecrypto::decrypt_bytes(&app, &blob)?;
        write_image_bytes(&app, &bytes)?;
        clipboard::note_self_write(&app);
        Ok(())
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_delete(app: AppHandle, id: String) -> Result<u32, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = store::attachments_dir(&app)?;
        let conn = open_conn(&app)?;
        let removed = store::delete_item(&conn, &id)?;
        for att in &removed {
            let _ = std::fs::remove_file(store::blob_path(&dir, &att.id));
            let _ = std::fs::remove_file(store::thumb_path(&dir, &att.id));
        }
        Ok(removed.len() as u32)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_clear(app: AppHandle) -> Result<ClipboardClearResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = store::attachments_dir(&app)?;
        let conn = open_conn(&app)?;
        let (items, _, _) = store::stats(&conn)?;
        let removed = store::clear(&conn)?;
        for att in &removed {
            let _ = std::fs::remove_file(store::blob_path(&dir, &att.id));
            let _ = std::fs::remove_file(store::thumb_path(&dir, &att.id));
        }
        // 兜底：清掉目录里残留的本功能密文文件
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".wst") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Ok(ClipboardClearResult {
            removed_items: items,
            removed_images: removed.len() as i64,
        })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_export_image(app: AppHandle, attachment_id: String, target_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if target_path.trim().is_empty() {
            return Err("导出路径为空".to_string());
        }
        let dir = store::attachments_dir(&app)?;
        let conn = open_conn(&app)?;
        let att = store::attachment(&conn, &attachment_id)?
            .ok_or_else(|| "图片附件不存在".to_string())?;
        let blob = std::fs::read(store::blob_path(&dir, &att.id))
            .map_err(|e| format!("读取图片附件失败: {e}"))?;
        let bytes = filecrypto::decrypt_bytes(&app, &blob)?;
        let target = std::path::PathBuf::from(&target_path);
        let tmp = target.with_file_name(format!(
            ".{}.tmp",
            target.file_name().and_then(|n| n.to_str()).unwrap_or("clipboard")
        ));
        std::fs::write(&tmp, &bytes).map_err(|e| format!("写入导出文件失败: {e}"))?;
        std::fs::rename(&tmp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("写入导出文件失败: {e}")
        })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn clipboard_limits() -> ClipboardLimits {
    ClipboardLimits {
        shortcut: "⌘⇧V".to_string(),
        max_text_chars: clipboard::MAX_TEXT_CHARS,
    }
}

// ---------------- 内部工具 ----------------

fn open_conn(app: &tauri::AppHandle) -> Result<rusqlite::Connection, String> {
    clipboard::db_conn(app)
}

fn text_preview(app: &tauri::AppHandle, row: &store::ItemRow) -> String {
    if row.text_enc.is_empty() {
        return String::new();
    }
    crypto::decrypt_str(app, &row.text_enc)
        .map(|text| text.chars().take(240).collect())
        .unwrap_or_default()
}

fn summary(app: &tauri::AppHandle, conn: &rusqlite::Connection, row: store::ItemRow) -> Result<ClipboardItemSummary, String> {
    let images = store::attachments(conn, &row.id)?
        .into_iter()
        .map(|att| ClipboardImageMeta {
            id: att.id,
            mime: att.mime,
            width: att.width,
            height: att.height,
            orig_size: att.orig_size,
        })
        .collect();
    Ok(ClipboardItemSummary {
        text_preview: text_preview(app, &row),
        id: row.id,
        kind: row.kind,
        created_at: row.created_at,
        image_count: row.image_count,
        byte_size: row.byte_size,
        images,
    })
}

fn page_impl(app: &tauri::AppHandle, before_created_at: Option<i64>, before_id: Option<String>, limit: Option<u32>) -> Result<ClipboardPage, String> {
    let conn = open_conn(app)?;
    let limit = limit.unwrap_or(clipboard::PAGE_SIZE as u32).clamp(1, 50) as usize;
    let cursor = match (before_created_at, before_id) {
        (Some(created), Some(id)) => Some((created, id)),
        _ => None,
    };
    let (rows, has_more) = store::page(&conn, cursor, limit)?;
    let items = rows
        .into_iter()
        .map(|row| summary(app, &conn, row))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ClipboardPage { items, has_more })
}

fn search_impl(app: &tauri::AppHandle, query: String, limit: Option<u32>) -> Result<Vec<ClipboardItemSummary>, String> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let cap = limit.unwrap_or(clipboard::SEARCH_LIMIT as u32).clamp(1, clipboard::SEARCH_LIMIT as u32) as usize;
    let conn = open_conn(app)?;
    let mut out = Vec::new();
    store::for_each_newest(&conn, |row| {
        let text = if row.text_enc.is_empty() {
            String::new()
        } else {
            crypto::decrypt_str(app, &row.text_enc).unwrap_or_default()
        };
        let html_text = if row.html_enc.is_empty() {
            String::new()
        } else {
            crypto::decrypt_str(app, &row.html_enc)
                .map(|html| clipboard::rich::strip_tags(&html))
                .unwrap_or_default()
        };
        if text.to_lowercase().contains(&needle) || html_text.to_lowercase().contains(&needle) {
            if let Ok(item) = summary(app, &conn, row) {
                out.push(item);
            }
        }
        out.len() < cap
    })?;
    Ok(out)
}

fn write_item_to_clipboard(app: &tauri::AppHandle, conn: &rusqlite::Connection, row: &store::ItemRow) -> Result<(), String> {
    let dir = store::attachments_dir(app)?;
    match row.kind.as_str() {
        "image" => {
            let att = store::attachments(conn, &row.id)?
                .into_iter()
                .next()
                .ok_or_else(|| "图片附件缺失".to_string())?;
            let blob = std::fs::read(store::blob_path(&dir, &att.id))
                .map_err(|e| format!("读取图片附件失败: {e}"))?;
            let bytes = filecrypto::decrypt_bytes(app, &blob)?;
            write_image_bytes(app, &bytes)?;
        }
        "rich" => {
            let html = if row.html_enc.is_empty() {
                String::new()
            } else {
                crypto::decrypt_str(app, &row.html_enc)?
            };
            let text = if row.text_enc.is_empty() {
                String::new()
            } else {
                crypto::decrypt_str(app, &row.text_enc)?
            };
            let mut blobs = Vec::new();
            for att in store::attachments(conn, &row.id)? {
                let path = store::blob_path(&dir, &att.id);
                if let Ok(blob) = std::fs::read(&path) {
                    if let Ok(bytes) = filecrypto::decrypt_bytes(app, &blob) {
                        blobs.push((att.id, att.mime, bytes));
                    }
                }
            }
            let restored = clipboard::rich::restore_html(&html, &blobs);
            if app
                .clipboard()
                .write_html(restored, Some(text.clone()))
                .is_err()
            {
                app.clipboard()
                    .write_text(text)
                    .map_err(|e| format!("写回剪贴板失败: {e}"))?;
            }
        }
        _ => {
            let text = crypto::decrypt_str(app, &row.text_enc)?;
            app.clipboard()
                .write_text(text)
                .map_err(|e| format!("写回剪贴板失败: {e}"))?;
        }
    }
    Ok(())
}

fn write_image_bytes(app: &tauri::AppHandle, bytes: &[u8]) -> Result<(), String> {
    let image = image::load_from_memory(bytes).map_err(|e| format!("图片解码失败: {e}"))?;
    let rgba = image.to_rgba8();
    let image = tauri::image::Image::new_owned(rgba.as_raw().clone(), rgba.width(), rgba.height());
    app.clipboard()
        .write_image(&image)
        .map_err(|e| format!("写回剪贴板失败: {e}"))
}

数据结构（每个都 `#[derive(Debug, Clone, serde::Serialize)]`）：

```rust
pub struct ClipboardItemSummary {
    pub id: String,
    pub kind: String,
    pub created_at: i64,
    pub text_preview: String,
    pub image_count: i64,
    pub byte_size: i64,
    pub images: Vec<ClipboardImageMeta>,
}
pub struct ClipboardImageMeta { pub id: String, pub mime: String, pub width: i64, pub height: i64, pub orig_size: i64 }
pub struct ClipboardPage { pub items: Vec<ClipboardItemSummary>, pub has_more: bool }
pub struct ClipboardStats { pub items: i64, pub images: i64, pub bytes: i64 }
pub struct ClipboardClearResult { pub removed_items: i64, pub removed_images: i64 }
pub struct ClipboardLimits { pub shortcut: String, pub max_text_chars: usize }
```

`clipboard_copy` 细节：

```rust
let row = store::find_by_id(&conn, &id)?;  // store 新增 find_by_id
match row.kind.as_str() {
    "image" => {
        let att = store::attachments(&conn, &row.id)?.into_iter().next().ok_or("图片附件缺失")?;
        let bytes = filecrypto::decrypt_bytes(&app, &std::fs::read(blob_path)?)?;
        let image = image::load_from_memory(&bytes).map_err(|e| format!("图片解码失败: {e}"))?;
        let rgba = image.to_rgba8();
        let image = tauri::image::Image::new_owned(
            rgba.as_raw().clone(),
            rgba.width(),
            rgba.height(),
        );
        app.clipboard().write_image(&image)?;
    }
    "rich" => {
        let html = crypto::decrypt_str(&app, &row.html_enc)?;
        let text = if row.text_enc.is_empty() { String::new() } else { crypto::decrypt_str(&app, &row.text_enc)? };
        let mut blobs = Vec::new();
        for att in store::attachments(&conn, &row.id)? {
            if let Ok(bytes) = filecrypto::decrypt_bytes(&app, &std::fs::read(store::blob_path(&dir, &att.id))?) {
                blobs.push((att.id, att.mime, bytes));
            }
        }
        let restored = clipboard::rich::restore_html(&html, &blobs);
        if app.clipboard().write_html(restored.clone(), Some(text.clone())).is_err() {
            app.clipboard().write_text(text)?;
        }
    }
    _ => {
        let text = crypto::decrypt_str(&app, &row.text_enc)?;
        app.clipboard().write_text(text)?;
    }
}
clipboard::note_self_write(&app);
```

- [ ] **Step 3: `wsclip` 协议**

```rust
pub fn serve_clipboard_image(app: &AppHandle, uri: &tauri::http::Uri) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{header, Response, StatusCode};

    fn plain(status: StatusCode, message: &str) -> Response<Vec<u8>> {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(message.as_bytes().to_vec())
            .unwrap_or_else(|_| Response::new(Vec::new()))
    }

    let params = parse_query(uri.query().unwrap_or(""));
    let id = params.get("id").cloned().unwrap_or_default();
    let variant = params.get("variant").cloned().unwrap_or_else(|| "full".into());
    if id.is_empty() {
        return plain(StatusCode::BAD_REQUEST, "缺少附件 id");
    }
    let dir = match crate::clipboard::store::attachments_dir(app) {
        Ok(dir) => dir,
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "附件目录不可用"),
    };
    let conn = match crate::clipboard::db_conn(app) {
        Ok(conn) => conn,
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "数据库不可用"),
    };
    let (mime, path) = match crate::clipboard::store::attachment(&conn, &id) {
        Ok(Some(att)) => (att.mime, if variant == "thumb" {
            crate::clipboard::store::thumb_path(&dir, &id)
        } else {
            crate::clipboard::store::blob_path(&dir, &id)
        }),
        Ok(None) => return plain(StatusCode::NOT_FOUND, "附件不存在"),
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "查询附件失败"),
    };
    let blob = match std::fs::read(&path) {
        Ok(blob) => blob,
        Err(_) => return plain(StatusCode::NOT_FOUND, "附件数据缺失"),
    };
    match crate::filecrypto::decrypt_bytes(app, &blob) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, if variant == "thumb" { "image/png".to_string() } else { mime })
            .header(header::CACHE_CONTROL, "no-store")
            .body(bytes)
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "响应构建失败")),
        Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "附件解密失败"),
    }
}

/// 与 attachments.rs 里同名函数等价的最小实现
fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.replace('+', " ")))
        .collect()
}
```

`lib.rs`：

```rust
.register_asynchronous_uri_scheme_protocol("wsclip", |ctx, request, responder| {
    let app = ctx.app_handle().clone();
    let uri = request.uri().clone();
    std::thread::spawn(move || responder.respond(commands::serve_clipboard_image(&app, &uri)));
})
```

- [ ] **Step 4: 注册 / 接线**

- `lib.rs`：`.manage(clipboard::ClipboardRuntime::new())`，`cargo test` 前先删掉旧 `ClipboardHistory`
- 快捷键 handler：`clipboard::on_hotkey(app)`（不再 toggle）
- 命令列表替换为：`clipboard_page`、`clipboard_search`、`clipboard_stats`、`clipboard_copy`、`clipboard_copy_image`、`clipboard_delete`、`clipboard_clear`、`clipboard_export_image`、`clipboard_limits`，保留 `clipboard_panel_hide`
- 移除 `clipboard_items` / 旧 `clipboard_clear` / 旧 `clipboard_limits.max_items`

- [ ] **Step 5: 编译 + 测试**

Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test clipboard -- --nocapture`
再 Run: `cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo check`
Expected: 编译通过，单测 PASS

- [ ] **Step 6: 提交**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lib.rs src-tauri/src/commands/clipboard.rs src-tauri/src/commands/mod.rs
git commit -m "feat(clipboard): 命令层、wsclip 协议与快捷键行为"
```

---

## Phase B：前端

### Task 7: API / 类型 / 共享 composable

**Files:**
- Rewrite: `src/api/clipboard.ts`
- Create: `src/types/clipboard.ts`
- Create: `src/composables/useClipboardHistory.ts`

- [ ] **Step 1: 类型与 API**

`src/types/clipboard.ts`：

```ts
export type ClipboardKind = 'text' | 'url' | 'code' | 'image' | 'rich'

export interface ClipboardImageMeta {
  id: string
  mime: string
  width: number
  height: number
  orig_size: number
}

export interface ClipboardItemSummary {
  id: string
  kind: ClipboardKind
  created_at: number
  text_preview: string
  image_count: number
  byte_size: number
  images: ClipboardImageMeta[]
}

export interface ClipboardPage { items: ClipboardItemSummary[]; has_more: boolean }
export interface ClipboardStats { items: number; images: number; bytes: number }
export interface ClipboardLimits { shortcut: string; max_text_chars: number }
export interface ClipboardClearResult { removed_items: number; removed_images: number }
```

`src/api/clipboard.ts`：`clipboardPage(before?, limit?)`、`searchClipboard(query)`、`clipboardStats()`、`copyClipboardItem(id)`、`copyClipboardImage(id)`、`deleteClipboardItem(id)`、`clearClipboard()`、`exportClipboardImage(id, path)`、`hideClipboardPanel()`、`getClipboardLimits()`，以及缩略图地址：

```ts
import { isWindows } from '@/platform'

export function clipboardImageUrl(id: string, variant: 'thumb' | 'full'): string {
  const prefix = isWindows() ? 'http://wsclip.localhost' : 'wsclip://localhost'
  return `${prefix}/img?id=${encodeURIComponent(id)}&variant=${variant}`
}
```

- [ ] **Step 2: composable**

`useClipboardHistory()` 返回：

```ts
{
  items, hasMore, loading, hasMoreLoading, query, searching, stats,
  loadFirst(), loadMore(), search(keyword), resetSearch(),
  remove(id), clearAll(), refreshStats()
}
```

- 使用 `clipboard_page` 游标（最后一条 `created_at` + `id`）
- `search` 250ms 防抖、结果替换列表；`resetSearch` 恢复首屏
- 删除 / 清空后调用 `loadFirst` + `refreshStats`
- `listen('clipboard-changed')` 在面板/管理页 `onMounted` 里由组件负责（composable 提供 `onChanged` 注册）

- [ ] **Step 3: 类型检查 + 提交**

Run: `pnpm build`（此时旧组件还会报错，先确保新文件类型自洽；若报错继续 Task 8/9 后统一跑）

```bash
git add src/types/clipboard.ts src/api/clipboard.ts src/composables/useClipboardHistory.ts
git commit -m "feat(clipboard): 前端 API 与分页 search composable"
```

### Task 8: 面板重写（`src/clipboard/ClipboardPanel.vue`）

**Files:**
- Rewrite: `src/clipboard/ClipboardPanel.vue`

- [ ] **Step 1: 脚本**

- 用 `useClipboardHistory`
- `onMounted`：加载首屏 + `listen('clipboard-panel-shown' → focusSearch + loadFirst)`、`listen('clipboard-changed' → loadFirst)`
- 键盘：`↑↓` 选择（到已加载底部自动 `loadMore`）、`Enter` 复制并 `hideClipboardPanel()`、`Esc` 清搜索 / 收起
- 搜索：v-model + `watch` 调 `search`；`Esc` 清空
- 列表哨兵 `IntersectionObserver` 触发 `loadMore`
- 缩略图 `<img :src="clipboardImageUrl(img.id, 'thumb')" loading="lazy">`，≤3 张

- [ ] **Step 2: 模板 / 样式**

- 保留现有窗口容器、搜索框、`cp-row` 样式
- 行内：类型徽标（文本/链接/代码/图片/图文）、`text_preview` 两行截断、缩略图条、相对时间
- 头尾：总数（stats.items）、`⌘⇧V` 徽标、加载中 / 没有更多
- 空状态：区分「没有历史」与「搜索无结果」

- [ ] **Step 3: 构建 + 提交**

Run: `pnpm build`
Expected: PASS

```bash
git add src/clipboard/ClipboardPanel.vue
git commit -m "feat(clipboard): 面板图文列表与懒加载"
```

### Task 9: 管理页 + 系统页页签

**Files:**
- Create: `src/components/clipboard/ClipboardManager.vue`
- Modify: `src/components/cleaner/CleanerPanel.vue`（`TabKey` 加 `clipboard`，Hosts 后加页签）

- [ ] **Step 1: 管理页**

- 工具栏：搜索、`共 N 条 · M 张图片 · X MB`、清空历史（危险按钮，空时禁用）
- 列表：类型徽标、预览、缩略图、绝对时间；行操作「复制」「删除」「查看图片」
- 删除：单条即时 + `message.success('已删除（含 N 张图片附件）')`
- 清空：`Modal.confirm`，文案带条数 / 图片数
- 图片弹层：`a-modal` + 多图切换 + 复制图片 + 导出（`@tauri-apps/plugin-dialog` `save` → `exportClipboardImage`）
- 滚动懒加载 + 空状态 + 加载骨架
- Windows：`import { platform } from '@/platform'`，`!platform.is_macos` 时工具栏/列表替换为提示文案

- [ ] **Step 2: 挂页签**

`CleanerPanel.vue`：

```ts
type TabKey = 'apps' | 'popup' | 'sleep' | 'hosts' | 'clipboard'
```

Hosts 按钮之后加「剪贴板」按钮（同款样式），pane：

```vue
<div v-if="tab === 'clipboard'" class="pane"><ClipboardManager /></div>
```

- [ ] **Step 3: 构建 + 提交**

Run: `pnpm build`
Expected: PASS（`vue-tsc` 无错误）

```bash
git add src/components/clipboard/ClipboardManager.vue src/components/cleaner/CleanerPanel.vue
git commit -m "feat(clipboard): 管理页与系统页剪贴板页签"
```

---

## Phase C：文档与验收

### Task 10: 文档、README、完整验证

**Files:**
- Modify: `README.md`（剪贴板章节、插件说明、数据路径、限制）
- Modify: `docs/superpowers/specs/2026-11-07-clipboard-and-hosts-design.md`（标注剪贴板部分被 v2 取代）

- [ ] **Step 1: README**

- 剪贴板章节改为：图文 / 图片支持、SQLite 加密持久化、`clipboard-attachments` 目录、管理页入口、`⌘⇧V` 行为（连按不收起、Esc/失焦收起）、无上限 + 懒加载、删除级联
- 数据路径章节补 `<app_data_dir>/clipboard-attachments/`、`ws-workbench.db` 新表
- 已知取舍保留 `⌘⇧V` 与其他 app「粘贴为纯文本」冲突说明

- [ ] **Step 2: 跑全量验证**

```bash
cd src-tauri && CARGO_HOME="$PWD/../.tmp/cargo-home" cargo test
pnpm build
```

Expected: Rust 全量 PASS、前端构建 PASS

- [ ] **Step 3: 手动验收（真实剪贴板）**

1. dev 运行，复制文本 → 面板显示；重复复制同一段 → 单条去重置顶
2. 截图到剪贴板 → 图片条目 + 缩略图；点选复制可粘贴
3. 浏览器复制带图文选区 → 「图文」条目 + 缩略图；回贴到富文本编辑器
4. 连按 `⌘⇧V` 3 次 → 面板保持打开
5. 搜索、滚动加载 >20 条
6. 管理页单条删除 → `clipboard-attachments` 对应文件消失；清空 → 目录空
7. 重启应用 → 历史仍在

- [ ] **Step 4: 提交**

```bash
git add README.md docs/
git commit -m "docs(clipboard): README 与 v2 设计状态更新"
```
