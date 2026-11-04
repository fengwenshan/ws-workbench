//! 附件加密上传、解密导出、目录解析与笔记内联图片服务。

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use tauri::{http, AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::filecrypto;
use crate::models::AttachmentMeta;

/// 发出附件加解密进度事件：`{ id, done, total }`
fn emit_attachment_progress(app: &AppHandle, id: &str, done: u64, total: u64) {
    let _ = app.emit(
        "attachment-progress",
        serde_json::json!({ "id": id, "done": done, "total": total }),
    );
}

/// 解析附件的统一上传目录：全局默认 > App 私有 `attachments` 目录
///
/// 分组/项目不再单独配置上传目录，由前端在展示「上传位置」行时调用，
/// 保证展示路径与实际落盘位置一致。
#[tauri::command]
pub fn resolve_upload_dir(app: AppHandle, default_dir: Option<String>) -> Result<String, String> {
    if let Some(dir) = default_dir.as_deref() {
        if !dir.trim().is_empty() {
            return Ok(dir.to_string());
        }
    }
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {}", e))?;
    Ok(base.join("attachments").to_string_lossy().to_string())
}

/// 校验目录路径在当前系统下是否合法。
///
/// - Windows：必须为绝对路径（盘符 `C:\` 或 UNC `\\server\share`），且盘符之后
///   不得出现 `< > : " | ? *` 等保留字符。
/// - macOS / Linux：必须是以 `/` 开头的绝对路径。
/// - 通用：不允许空字符。
fn validate_dir_path(path: &str) -> Result<(), String> {
    if path.contains('\0') {
        return Err("地址包含非法字符".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let bytes = path.as_bytes();
        let is_drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        let is_unc = path.starts_with("\\\\") || path.starts_with("//");
        if !is_drive && !is_unc {
            return Err("请填写绝对路径，例如 C:\\work".to_string());
        }
        // 盘符之后不允许出现 Windows 保留字符
        let rest = if is_drive { &path[2..] } else { path };
        if rest.contains(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')) {
            return Err("地址包含 Windows 不允许的字符：< > : \" | ? *".to_string());
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if !path.starts_with('/') {
            return Err("请填写绝对路径，例如 /Users/you/work".to_string());
        }
    }

    Ok(())
}

/// 校验分组地址并确保目录存在：不存在则按需创建，返回最终使用的路径。
///
/// 供「新建分组」填写/选择地址时调用：路径不合法直接报错；已存在但不是
/// 目录则报错；合法且不存在则递归创建。
#[tauri::command]
pub fn ensure_dir(path: String) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("请填写地址".to_string());
    }
    validate_dir_path(trimmed)?;
    let dir = PathBuf::from(trimmed);
    if dir.is_dir() {
        return Ok(trimmed.to_string());
    }
    if dir.exists() {
        return Err(format!("该地址已存在同名文件，无法作为目录: {}", trimmed));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败（{}）: {}", trimmed, e))?;
    Ok(trimmed.to_string())
}

/// 上传并加密文件：原子写入 `<upload_dir>/<uuid>.wst`，返回附件元信息
#[tauri::command]
pub fn encrypt_upload_file(
    app: AppHandle,
    src_path: String,
    upload_dir: String,
    name: String,
) -> Result<AttachmentMeta, String> {
    let src = Path::new(&src_path);
    if !src.is_file() {
        return Err(format!("源文件不存在: {}", src_path));
    }
    let size = std::fs::metadata(src)
        .map_err(|e| format!("读取源文件信息失败: {}", e))?
        .len();

    let dir = PathBuf::from(&upload_dir);
    crate::util::fs::create_dir_all(&dir)?;

    let id = Uuid::new_v4().to_string();
    let tmp = dir.join(format!("{}.wst.tmp", id));
    let final_path = dir.join(format!("{}.wst", id));

    {
        let mut on_progress = |done: u64, total: u64| {
            emit_attachment_progress(&app, &id, done, total);
        };
        if let Err(e) = filecrypto::encrypt_file_stream(&app, src, &tmp, &mut on_progress) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }

    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }

    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let created_at = crate::util::time::now_utc_millis();

    Ok(AttachmentMeta {
        id,
        name,
        size,
        ext,
        created_at,
        dir: upload_dir,
    })
}

/// 上传内存字节：base64 解码 → 内存加密 → 原子写入 `<upload_dir>/<uuid>.wst`
///
/// 供笔记编辑器粘贴 / 拖拽 / 选择图片使用，全程不产生明文临时文件。
#[tauri::command]
pub fn encrypt_upload_bytes(
    app: AppHandle,
    upload_dir: String,
    name: String,
    ext: String,
    data_b64: String,
) -> Result<AttachmentMeta, String> {
    let data = B64
        .decode(data_b64.as_bytes())
        .map_err(|e| format!("附件数据解码失败: {}", e))?;
    if data.is_empty() {
        return Err("附件内容为空".to_string());
    }
    let size = data.len() as u64;

    let dir = PathBuf::from(&upload_dir);
    crate::util::fs::create_dir_all(&dir)?;

    let blob = filecrypto::encrypt_bytes(&app, &data)?;

    let id = Uuid::new_v4().to_string();
    let tmp = dir.join(format!("{}.wst.tmp", id));
    let final_path = dir.join(format!("{}.wst", id));

    if let Err(e) = std::fs::write(&tmp, &blob) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }

    let ext = ext.trim().trim_start_matches('.').to_lowercase();
    let created_at = crate::util::time::now_utc_millis();

    Ok(AttachmentMeta {
        id,
        name,
        size,
        ext,
        created_at,
        dir: upload_dir,
    })
}

/// 解密导出到用户指定的目标路径
#[tauri::command]
pub fn decrypt_export_file(
    app: AppHandle,
    blob_dir: String,
    id: String,
    target_path: String,
) -> Result<(), String> {
    let blob = PathBuf::from(&blob_dir).join(format!("{}.wst", id));
    if !blob.is_file() {
        return Err(format!("附件文件不存在: {}", blob.display()));
    }
    let target = PathBuf::from(&target_path);
    let mut on_progress = |done: u64, total: u64| {
        emit_attachment_progress(&app, &id, done, total);
    };
    filecrypto::decrypt_file_stream(&app, &blob, &target, &mut on_progress)
}

/// 解密到带原扩展名的临时文件，返回绝对路径（供系统默认程序打开预览）
#[tauri::command]
pub fn decrypt_to_temp(
    app: AppHandle,
    blob_dir: String,
    id: String,
    name: String,
) -> Result<String, String> {
    let blob = PathBuf::from(&blob_dir).join(format!("{}.wst", id));
    if !blob.is_file() {
        return Err(format!("附件文件不存在: {}", blob.display()));
    }

    let ext = Path::new(&name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let suffix = if ext.is_empty() || ext.eq_ignore_ascii_case("wst") {
        String::new()
    } else {
        format!(".{}", ext)
    };

    let named = tempfile::Builder::new()
        .prefix("ws-workbench-")
        .suffix(&suffix)
        .tempfile()
        .map_err(|e| format!("创建临时文件失败: {}", e))?;
    let (_file, path) = named
        .keep()
        .map_err(|e| format!("创建临时文件失败: {}", e))?;

    let mut on_progress = |done: u64, total: u64| {
        emit_attachment_progress(&app, &id, done, total);
    };
    filecrypto::decrypt_file_stream(&app, &blob, &path, &mut on_progress)?;

    Ok(path.to_string_lossy().to_string())
}

/// 删除附件密文文件；文件不存在视为成功
#[tauri::command]
pub fn remove_attachment_file(blob_dir: String, id: String) -> Result<(), String> {
    let blob = PathBuf::from(&blob_dir).join(format!("{}.wst", id));
    match std::fs::remove_file(&blob) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除附件失败: {}", e)),
    }
}

/// 解析笔记内联图片请求并返回响应：
/// `<prefix>/img?id=<uuid>&ext=<png>&dir=<urlencoded 绝对目录>`
///
/// 仅服务本应用密文（`.wst`）；`id` 必须为 uuid 形态，防目录穿越。
pub fn serve_note_image(app: &AppHandle, uri: &http::Uri) -> http::Response<Vec<u8>> {
    let params = parse_query(uri.query().unwrap_or(""));
    let id = params.get("id").cloned().unwrap_or_default();
    let ext = params.get("ext").cloned().unwrap_or_default();
    let dir = params.get("dir").cloned().unwrap_or_default();

    if id.is_empty() || dir.is_empty() || Uuid::parse_str(&id).is_err() {
        return plain_response(400, "请求参数不合法");
    }

    let blob_path = PathBuf::from(&dir).join(format!("{}.wst", id));
    let blob = match std::fs::read(&blob_path) {
        Ok(bytes) => bytes,
        Err(_) => return plain_response(404, "图片数据缺失"),
    };

    match filecrypto::decrypt_bytes(app, &blob) {
        Ok(plain) => http::Response::builder()
            .status(200)
            .header(http::header::CONTENT_TYPE, content_type_for_ext(&ext))
            .header(http::header::CACHE_CONTROL, "no-store")
            .body(plain)
            .unwrap_or_else(|_| plain_response(500, "构建响应失败")),
        Err(_) => plain_response(500, "图片数据损坏"),
    }
}

/// 构造一个 `text/plain` 的简单响应
fn plain_response(status: u16, message: &str) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

/// 扩展名 → `Content-Type`
fn content_type_for_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

/// 解析 `a=1&b=2` 形式的查询串（含百分号解码）
fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

/// 最小百分号解码（`%XX` 与 `+`），避免为解析 URL 引入新依赖
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
