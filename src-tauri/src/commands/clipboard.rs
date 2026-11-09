//! 剪贴板历史命令。
//!
//! 命令只做参数/错误翻译；采集、加密、SQLite 读写都在 `crate::clipboard`。
//! 面板是独立窗口（label = `clipboard-panel`），前端不直接碰窗口 API。

use tauri::{AppHandle, Emitter};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::clipboard::{self, rich, store};
use crate::{crypto, filecrypto};

/// 面板列表项（正文只给预览，完整内容留在库里）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardItemSummary {
    pub id: String,
    pub kind: String,
    pub created_at: i64,
    pub text_preview: String,
    pub image_count: i64,
    pub byte_size: i64,
    pub images: Vec<ClipboardImageMeta>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardImageMeta {
    pub id: String,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub orig_size: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardPage {
    pub items: Vec<ClipboardItemSummary>,
    pub has_more: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardStats {
    pub items: i64,
    pub images: i64,
    pub bytes: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardClearResult {
    pub removed_items: i64,
    pub removed_images: i64,
}

/// 面板展示用到的一些静态信息：快捷键与文本上限
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardLimits {
    pub shortcut: String,
    pub max_text_chars: usize,
}

/// 分页读取历史（新 → 旧）
#[tauri::command]
pub async fn clipboard_page(
    app: AppHandle,
    before_created_at: Option<i64>,
    before_id: Option<String>,
    limit: Option<u32>,
) -> Result<ClipboardPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        page_impl(&app, before_created_at, before_id, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 搜索历史：后端解密一遍文本 / HTML 再匹配（历史都在本地，量级可接受）
#[tauri::command]
pub async fn clipboard_search(
    app: AppHandle,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<ClipboardItemSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || search_impl(&app, query, limit))
        .await
        .map_err(|e| e.to_string())?
}

/// 历史统计：条数 / 图片数 / 占用字节
#[tauri::command]
pub async fn clipboard_stats(app: AppHandle) -> Result<ClipboardStats, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_conn(&app)?;
        let (items, images, bytes) = store::stats(&conn)?;
        Ok(ClipboardStats {
            items,
            images,
            bytes,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 把一条历史写回系统剪贴板。
///
/// 只写回、不模拟 ⌘V：自动粘贴需要「辅助功能」授权，本功能不做。
/// 写回后同步 changeCount，避免采集线程把刚写的内容再存一遍。
#[tauri::command]
pub async fn clipboard_copy(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_conn(&app)?;
        let row = store::find_by_id(&conn, &id)?
            .ok_or_else(|| "这条剪贴板记录已经不存在了".to_string())?;
        write_item_to_clipboard(&app, &conn, &row)?;
        clipboard::note_self_write(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 只把某张图片附件写回剪贴板
#[tauri::command]
pub async fn clipboard_copy_image(app: AppHandle, attachment_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = store::attachments_dir(&app)?;
        let conn = open_conn(&app)?;
        let att =
            store::attachment(&conn, &attachment_id)?.ok_or_else(|| "图片附件不存在".to_string())?;
        let blob = std::fs::read(store::blob_path(&dir, &att.id))
            .map_err(|e| format!("读取图片附件失败: {e}"))?;
        let bytes = filecrypto::decrypt_bytes(&app, &blob)?;
        write_image_bytes(&app, &bytes)?;
        clipboard::note_self_write(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 删除单条历史；带附件的一并删掉本地密文文件
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
        let _ = app.emit(clipboard::EVENT_CHANGED, ());
        Ok(removed.len() as u32)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 清空全部历史与附件文件
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
        let _ = app.emit(clipboard::EVENT_CHANGED, ());
        Ok(ClipboardClearResult {
            removed_items: items,
            removed_images: removed.len() as i64,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 把某张图片附件导出成用户选择的文件（解密后原样写出）
#[tauri::command]
pub async fn clipboard_export_image(
    app: AppHandle,
    attachment_id: String,
    target_path: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if target_path.trim().is_empty() {
            return Err("导出路径为空".to_string());
        }
        let dir = store::attachments_dir(&app)?;
        let conn = open_conn(&app)?;
        let att =
            store::attachment(&conn, &attachment_id)?.ok_or_else(|| "图片附件不存在".to_string())?;
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
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn clipboard_limits() -> ClipboardLimits {
    ClipboardLimits {
        shortcut: if cfg!(target_os = "macos") {
            "⌘⇧V".to_string()
        } else {
            "—".to_string()
        },
        max_text_chars: clipboard::MAX_TEXT_CHARS,
    }
}

/// 收起面板（Esc / 点击外部后的兜底）
#[tauri::command]
pub fn clipboard_panel_hide(app: AppHandle) {
    clipboard::hide_panel(&app);
}

// ---------------- 内部工具 ----------------

fn open_conn(app: &AppHandle) -> Result<rusqlite::Connection, String> {
    clipboard::db_conn(app)
}

fn text_preview(app: &AppHandle, row: &store::ItemRow) -> String {
    if row.text_enc.is_empty() {
        return String::new();
    }
    crypto::decrypt_str(app, &row.text_enc)
        .map(|text| text.chars().take(240).collect())
        .unwrap_or_default()
}

fn summary(
    app: &AppHandle,
    conn: &rusqlite::Connection,
    row: store::ItemRow,
) -> Result<ClipboardItemSummary, String> {
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

fn page_impl(
    app: &AppHandle,
    before_created_at: Option<i64>,
    before_id: Option<String>,
    limit: Option<u32>,
) -> Result<ClipboardPage, String> {
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

fn search_impl(
    app: &AppHandle,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<ClipboardItemSummary>, String> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let cap = limit
        .unwrap_or(clipboard::SEARCH_LIMIT as u32)
        .clamp(1, clipboard::SEARCH_LIMIT as u32) as usize;
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
                .map(|html| rich::strip_tags(&html))
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

fn write_item_to_clipboard(
    app: &AppHandle,
    conn: &rusqlite::Connection,
    row: &store::ItemRow,
) -> Result<(), String> {
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
            let restored = rich::restore_html(&html, &blobs);
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

fn write_image_bytes(app: &AppHandle, bytes: &[u8]) -> Result<(), String> {
    let image = image::load_from_memory(bytes).map_err(|e| format!("图片解码失败: {e}"))?;
    let rgba = image.to_rgba8();
    let image = tauri::image::Image::new_owned(rgba.as_raw().clone(), rgba.width(), rgba.height());
    app.clipboard()
        .write_image(&image)
        .map_err(|e| format!("写回剪贴板失败: {e}"))
}

/// `wsclip` 自定义协议：给面板 / 管理页读图片缩略图与原图。
///
/// `id` = 附件 id，`variant` = `thumb` / `full`（默认 thumb）。
/// 数据永远来自本地密文，读时解密，`Cache-Control: no-store`。
pub fn serve_clipboard_image(
    app: &AppHandle,
    uri: &tauri::http::Uri,
) -> tauri::http::Response<Vec<u8>> {
    let params = clipboard_parse_query(uri.query().unwrap_or(""));
    let id = params.get("id").cloned().unwrap_or_default();
    let variant = params.get("variant").cloned().unwrap_or_default();
    if id.is_empty() || uuid::Uuid::parse_str(&id).is_err() {
        return plain_response(400, "请求参数不合法");
    }
    let dir = match store::attachments_dir(app) {
        Ok(dir) => dir,
        Err(_) => return plain_response(500, "获取附件目录失败"),
    };
    let conn = match open_conn(app) {
        Ok(conn) => conn,
        Err(_) => return plain_response(500, "打开数据库失败"),
    };
    let att = match store::attachment(&conn, &id) {
        Ok(Some(att)) => att,
        _ => return plain_response(404, "图片附件不存在"),
    };
    let path = if variant == "full" {
        store::blob_path(&dir, &id)
    } else {
        store::thumb_path(&dir, &id)
    };
    let blob = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return plain_response(404, "图片数据缺失"),
    };
    match filecrypto::decrypt_bytes(app, &blob) {
        Ok(plain) => tauri::http::Response::builder()
            .status(200)
            .header(tauri::http::header::CONTENT_TYPE, att.mime)
            .header(tauri::http::header::CACHE_CONTROL, "no-store")
            .body(plain)
            .unwrap_or_else(|_| plain_response(500, "构建响应失败")),
        Err(_) => plain_response(500, "图片数据损坏"),
    }
}

fn clipboard_parse_query(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn plain_response(status: u16, message: &str) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header(
            tauri::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
}
