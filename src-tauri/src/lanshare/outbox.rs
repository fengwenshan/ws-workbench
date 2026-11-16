//! 粘贴 / 拖拽发送的暂存落盘。
//!
//! 设计：`docs/superpowers/specs/2026-11-16-lan-share-session-ux-design.md` §5。
//!
//! 为什么要有这个目录：拖拽进来的 `File` 只有字节没有路径，剪贴板里的图片也只有
//! 位图；发送流程（`LanShareState::send_files`）只接受本地路径，所以先把它们落到
//! `<storage_dir>/lan_share_outbox/`，再走正常的文件发送链路。
//!
//! 暂存副本不会在删除聊天记录/附件时清理：发送失败重试、聊天里「打开」都还指着它。

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// 单次拖拽最多几个文件（和协议上限取齐，避免一次塞爆）
pub const MAX_DROPPED_FILES: usize = 20;
/// 单个拖拽文件上限：超过就走「发送文件」的流式链路
pub const MAX_DROPPED_FILE_BYTES: usize = 100 * 1024 * 1024;

const OUTBOX_DIR_NAME: &str = "lan_share_outbox";

/// 前端拖拽文件：名字 + base64 字节
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DroppedFileInput {
    pub name: String,
    pub data_base64: String,
}

/// 暂存目录：`<可写数据目录>/lan_share_outbox/`，不存在时自动创建
pub fn staging_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = crate::db::storage_dir(app)?.join(OUTBOX_DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败: {}", e))?;
    Ok(dir)
}

/// 把拖拽的字节写成文件，返回本地路径。
///
/// 校验：文件数上限、单文件大小上限、base64 解码、文件名清洗。
pub fn save_dropped_files(
    app: &AppHandle,
    files: &[DroppedFileInput],
) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Err("没有收到文件".to_string());
    }
    if files.len() > MAX_DROPPED_FILES {
        return Err(format!("一次最多发送 {} 个文件", MAX_DROPPED_FILES));
    }
    let dir = staging_dir(app)?;
    let mut paths = Vec::with_capacity(files.len());
    for file in files {
        let bytes = decode_dropped(file)?;
        paths.push(write_dropped_to(&dir, &file.name, &bytes)?);
    }
    Ok(paths)
}

/// 读系统剪贴板：优先文件列表（Finder 里复制的文件），其次图片。
///
/// 返回本地路径；两者都没有时报错。目录暂不支持（和「发送文件」的 picker 一致）。
pub fn clipboard_paths(app: &AppHandle) -> Result<Vec<String>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("读取剪贴板失败: {}", e))?;

    let mut paths = Vec::new();
    let mut saw_dir = false;
    if let Ok(list) = clipboard.get().file_list() {
        for path in list {
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_dir() => saw_dir = true,
                Ok(meta) if meta.file_type().is_file() => {
                    paths.push(path.to_string_lossy().to_string());
                }
                _ => {}
            }
        }
    }
    if !paths.is_empty() {
        return Ok(paths);
    }
    if saw_dir {
        return Err("暂不支持发送文件夹".to_string());
    }

    if let Ok(image) = clipboard.get().image() {
        return Ok(vec![save_clipboard_image(app, &image)?]);
    }

    Err("剪贴板里没有可发送的文件或图片".to_string())
}

/// 剪贴板位图 → PNG 暂存文件，返回路径
pub fn save_clipboard_image(
    app: &AppHandle,
    image: &arboard::ImageData<'_>,
) -> Result<String, String> {
    let dir = staging_dir(app)?;
    save_clipboard_image_to(
        &dir,
        image.width as u32,
        image.height as u32,
        image.bytes.as_ref(),
    )
}

/// base64 长度（含 padding 前的粗略上界）是否超过单文件上限。
///
/// 放在解码之前判断，避免为了校验先分配几百 MB。
fn encoded_too_large(encoded_len: usize) -> bool {
    let max_encoded = MAX_DROPPED_FILE_BYTES.div_ceil(3) * 4;
    encoded_len > max_encoded
}

fn decode_dropped(file: &DroppedFileInput) -> Result<Vec<u8>, String> {
    if encoded_too_large(file.data_base64.len()) {
        return Err(format!(
            "文件「{}」超过 {}MB，请用「发送文件」选择",
            sanitize_name(&file.name),
            MAX_DROPPED_FILE_BYTES / 1024 / 1024
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file.data_base64.as_bytes())
        .map_err(|e| format!("解析拖拽文件失败: {}", e))?;
    if bytes.is_empty() {
        return Err("拖拽的文件是空的".to_string());
    }
    if bytes.len() > MAX_DROPPED_FILE_BYTES {
        return Err(format!(
            "文件「{}」超过 {}MB，请用「发送文件」选择",
            sanitize_name(&file.name),
            MAX_DROPPED_FILE_BYTES / 1024 / 1024
        ));
    }
    Ok(bytes)
}

/// 清洗文件名；清洗不出来时用 `file` 兜底，保证可落盘
fn sanitize_name(raw: &str) -> String {
    crate::lanshare::protocol::sanitize_file_name(raw).unwrap_or_else(|| "file".to_string())
}

/// 在目录里找一个不冲突的名字：`name`、`name-1`、`name-2`…
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    for index in 1..1000 {
        let file_name = match &ext {
            Some(ext) if !ext.is_empty() => format!("{}-{}.{}", stem, index, ext),
            _ => format!("{}-{}", stem, index),
        };
        let candidate = dir.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{}-{}", stem, uuid::Uuid::new_v4().simple()))
}

fn write_dropped_to(dir: &Path, raw_name: &str, bytes: &[u8]) -> Result<String, String> {
    let name = sanitize_name(raw_name);
    let path = unique_path(dir, &name);
    std::fs::write(&path, bytes).map_err(|e| format!("写入暂存文件失败: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

/// 位图 RGBA 编码成 PNG 落盘；文件名带秒级时间戳，便于用户识别
fn save_clipboard_image_to(
    dir: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<String, String> {
    if width == 0 || height == 0 {
        return Err("剪贴板图片尺寸无效".to_string());
    }
    let expected = width as usize * height as usize * 4;
    if rgba.len() < expected {
        return Err("剪贴板图片数据不完整".to_string());
    }
    let image = image::RgbaImage::from_raw(width, height, rgba[..expected].to_vec())
        .ok_or_else(|| "剪贴板图片数据无效".to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!("剪贴板图片_{}.png", stamp);
    let path = unique_path(dir, &name);
    image::DynamicImage::ImageRgba8(image)
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| format!("保存剪贴板图片失败: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn encoded_size_guard_rejects_oversize_without_decoding() {
        assert!(!encoded_too_large(16));
        let just_over = MAX_DROPPED_FILE_BYTES.div_ceil(3) * 4 + 4;
        assert!(encoded_too_large(just_over));
    }

    #[test]
    fn write_dropped_sanitizes_path_traversal_and_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_dropped_to(dir.path(), "../../evil.txt", b"a").unwrap();
        let second = write_dropped_to(dir.path(), "../../evil.txt", b"b").unwrap();
        assert!(first.ends_with("evil.txt"));
        assert!(second.ends_with("evil-1.txt"), "重名要加后缀: {}", second);
        assert_eq!(std::fs::read(&first).unwrap(), b"a");
        assert_eq!(std::fs::read(&second).unwrap(), b"b");
    }

    #[test]
    fn write_dropped_falls_back_to_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_dropped_to(dir.path(), "  ", b"x").unwrap();
        assert!(path.ends_with("file"));
    }

    #[test]
    fn save_clipboard_image_encodes_png() {
        let dir = tempfile::tempdir().unwrap();
        // 2x2 红像素
        let rgba = vec![255u8, 0, 0, 255].repeat(4);
        let path = save_clipboard_image_to(dir.path(), 2, 2, &rgba).unwrap();
        assert!(path.ends_with(".png"), "{}", path);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let decoded = image::open(&path).unwrap();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
    }

    #[test]
    fn save_clipboard_image_rejects_bad_size() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save_clipboard_image_to(dir.path(), 0, 2, &[]).is_err());
        assert!(save_clipboard_image_to(dir.path(), 2, 2, &[0u8; 4]).is_err());
    }

    #[test]
    fn decode_dropped_rejects_empty_and_oversize() {
        let empty = DroppedFileInput {
            name: "a.txt".into(),
            data_base64: encode(b""),
        };
        assert!(decode_dropped(&empty).is_err());
        let big = DroppedFileInput {
            name: "a.bin".into(),
            data_base64: "A".repeat(MAX_DROPPED_FILE_BYTES / 3 * 4 + 8),
        };
        let err = decode_dropped(&big).unwrap_err();
        assert!(err.contains("超过"), "{}", err);
    }
}
