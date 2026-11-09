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
///
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

/// 找下一个 `<img` 的字节位置（大小写不敏感）
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
        ExtractedImage {
            mime,
            bytes,
            width,
            height,
            placeholder,
        },
    ))
}

/// 返回 src 属性值在 tag 里的字节范围（不含引号）。
fn find_src_range(tag: &str) -> Option<(usize, usize)> {
    let bytes = tag.as_bytes();
    let lower = tag.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("src") {
        let attr = search_from + rel;
        // 前面必须是空白或引号，避免命中 `data-src` 之类的属性
        let prev_ok = attr == 0
            || matches!(
                bytes[attr - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'"' | b'\''
            );
        if prev_ok {
            let mut cursor = attr + 3;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'=') {
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
                            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                            .map(|i| start + i)
                            .unwrap_or(tag.len());
                        Some((start, end))
                    }
                    None => None,
                };
            }
        }
        search_from = attr + 3;
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
        let Some(pos) = out.find(PLACEHOLDER_PREFIX) else {
            break;
        };
        let start = out[..pos].rfind("<img").unwrap_or(pos);
        let end = out[pos..]
            .find('>')
            .map(|i| pos + i + 1)
            .unwrap_or(out.len());
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

/// 解码任意支持格式 → 最长边 max_edge 的 PNG 缩略图。
pub fn png_thumbnail(bytes: &[u8], max_edge: u32) -> Option<Vec<u8>> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let thumb = decoded.thumbnail(max_edge, max_edge);
    let mut out = Vec::new();
    thumb
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
    fn ignores_data_src_attribute() {
        let html = r#"<img data-src="x" src="https://example.com/b.png">"#;
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
}
