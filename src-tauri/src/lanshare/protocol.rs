//! 局域网分享的报文结构、文件名清洗、重名处理与 HTTP 请求解析。
//!
//! 这里只放「与网络无关的纯逻辑」：结构体定义、JSON 约定、字符串清洗、
//! 路径去重和 HTTP 头解析。真正收发字节的代码在 `discovery` / `server` /
//! `client` 里，方便单测覆盖解析规则，不用起网络。

use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{APP_TAG, DEVICE_TYPE, PROTOCOL_VERSION};

/// 组播 announce 报文。
///
/// 字段名与设计文档 3.1 一致；`type` 是 Rust 关键字，用 `kind` 承载再改名。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Announce {
    pub app: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u32,
    pub alias: String,
    pub fingerprint: String,
    pub port: u16,
    #[serde(default = "default_device_type")]
    pub device_type: String,
}

fn default_device_type() -> String {
    DEVICE_TYPE.to_string()
}

impl Announce {
    pub fn new(alias: impl Into<String>, fingerprint: impl Into<String>, port: u16) -> Self {
        Self {
            app: APP_TAG.to_string(),
            kind: "announce".to_string(),
            version: PROTOCOL_VERSION,
            alias: alias.into(),
            fingerprint: fingerprint.into(),
            port,
            device_type: DEVICE_TYPE.to_string(),
        }
    }

    /// 是否是本应用、本协议版本的合法 announce。
    ///
    /// 版本只做「可兼容」判断：当前只认 1，后续加字段不一定要升版本，
    /// 升了版本旧端直接忽略，避免把不认识的设备列出来。
    pub fn is_valid(&self) -> bool {
        self.app == APP_TAG
            && self.kind == "announce"
            && self.version == PROTOCOL_VERSION
            && !self.fingerprint.trim().is_empty()
            && self.port != 0
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice::<Self>(bytes).map_err(|err| format!("announce 解析失败: {err}"))
    }
}

/// `GET /api/lanshare/v1/info` 响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InfoResponse {
    pub alias: String,
    pub fingerprint: String,
    pub port: u16,
    pub version: u32,
}

/// 发送方身份（prepare-upload 请求体里）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SenderInfo {
    pub alias: String,
    pub fingerprint: String,
}

/// `POST /api/lanshare/v1/prepare-upload` 请求体
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrepareUploadRequest {
    pub sender: SenderInfo,
    pub files: Vec<super::OfferFile>,
}

/// prepare-upload 接受后，每个文件对应的上传 token
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileToken {
    pub id: String,
    pub token: String,
}

/// prepare-upload 接受响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrepareUploadResponse {
    pub session_id: String,
    pub files: Vec<FileToken>,
}

/// upload 成功响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadResponse {
    pub ok: bool,
    pub path: String,
}

/// 统一错误响应体
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub error: String,
}

impl ErrorResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
        }
    }
}

/// 清洗文件名：只保留 basename，删掉路径分隔符、控制字符和首尾空白/点。
///
/// 返回 `None` 表示清洗后没有可用名字（调用方用 `file` 兜底）。
///
/// 规则：
/// - `\` 与 `/` 都当路径分隔符（发送方可能是 Windows）；
/// - 控制字符（含 `\0`、`\r`、`\n`）直接删掉，避免写盘/日志时被截断；
/// - 首尾空白和结尾的点去掉：Windows 会把 `abc.` 当 `abc`，留着容易读错；
/// - 长度按字符截到 200，给扩展名留位置（极长文件名在部分文件系统会直接失败）。
pub fn sanitize_file_name(raw: &str) -> Option<String> {
    let name = raw.replace('\\', "/");
    // 末尾的分隔符不构成文件名的一部分（发送方把目录拖进来时会出现）
    let name = name
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let cleaned: String = name
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .trim()
        .to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }
    // 按字符截断而不是按字节，避免把多字节字符切坏
    let mut chars: Vec<char> = cleaned.chars().collect();
    if chars.len() > 200 {
        chars.truncate(200);
        // 截断后可能刚好落在扩展名前，补一个点意义不大，直接返回截断结果
        return Some(chars.into_iter().collect());
    }
    Some(cleaned)
}

/// 目标目录下不覆盖已有文件的落盘路径：同名时依次加 ` (1)`、` (2)`…
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    let ext = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..=9999u32 {
        let name = format!("{stem} ({index}){ext}");
        let path = dir.join(name);
        if !path.exists() {
            return path;
        }
    }
    // 极端情况（同目录 9999 个重名）：退回原名，由调用方决定是否覆盖
    dir.join(file_name)
}

/// 解析 URL 查询串（`a=1&b=2`），值做 percent-decode。
pub fn parse_query(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        let key = percent_decode(key);
        let value = percent_decode(value);
        if !key.is_empty() {
            map.insert(key, value);
        }
    }
    map
}

/// 只处理 `%XX` 与 `+`；非法转义保持原样（查询参数都是我们自己拼的，不需要严格模式）
pub fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 把查询参数编成 `k=v&k2=v2`（值按 percent 规则转义）。
pub fn encode_query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 一条 HTTP 请求的头部信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    /// 不含查询串的路径
    pub path: String,
    pub query: HashMap<String, String>,
    /// 头部键统一小写
    pub headers: HashMap<String, String>,
    /// `Content-Length`，缺失为 0
    pub content_length: usize,
}

impl RequestHead {
    #[cfg(test)]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(|value| value.as_str())
    }
}

/// 读取并解析 HTTP/1.1 请求头。
///
/// 只读到空行为止，请求体留在 reader 里由调用方按 `content_length` 流式读取。
/// 头部总长超过 64KB 直接报错，避免被恶意长头部拖住线程。
pub fn read_request_head<R: BufRead>(reader: &mut R) -> Result<RequestHead, String> {
    const MAX_HEADER_BYTES: usize = 64 * 1024;
    let mut total = 0usize;

    let mut read_line = |total: &mut usize| -> Result<String, String> {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|err| format!("读取请求行失败: {err}"))?;
        if read == 0 {
            return Err("连接已关闭".to_string());
        }
        *total += read;
        if *total > MAX_HEADER_BYTES {
            return Err("请求头过大".to_string());
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    };

    let request_line = read_line(&mut total)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_ascii_uppercase();
    let target = parts.next().unwrap_or_default().to_string();
    let version = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
        return Err("请求行格式错误".to_string());
    }

    let (path, query_raw) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (target, String::new()),
    };
    let query = parse_query(&query_raw);

    let mut headers = HashMap::new();
    loop {
        let line = read_line(&mut total)?;
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);

    Ok(RequestHead {
        method,
        path,
        query,
        headers,
        content_length,
    })
}

/// 从请求体里读取 `content_length` 字节；调用方负责先校验长度上限。
pub fn read_body<R: Read>(reader: &mut R, content_length: usize) -> Result<Vec<u8>, String> {
    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|err| format!("读取请求体失败: {err}"))?;
    Ok(body)
}

/// 读取 JSON 请求体（只用于 prepare-upload 这类小报文）
pub fn read_json_body<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
    content_length: usize,
) -> Result<T, String> {
    let body = read_body(reader, content_length)?;
    serde_json::from_slice::<T>(&body).map_err(|err| format!("请求体解析失败: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn sanitize_keeps_plain_name() {
        assert_eq!(
            sanitize_file_name("report.pdf").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            sanitize_file_name(" 笔记 2026.md ").as_deref(),
            Some("笔记 2026.md")
        );
    }

    #[test]
    fn sanitize_strips_path_components() {
        assert_eq!(
            sanitize_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            sanitize_file_name(r"C:\Users\me\Desktop\a.txt").as_deref(),
            Some("a.txt")
        );
        assert_eq!(sanitize_file_name("/tmp/x/").as_deref(), Some("x"));
        assert_eq!(sanitize_file_name("..").as_deref(), None);
        assert_eq!(sanitize_file_name("/").as_deref(), None);
    }

    #[test]
    fn sanitize_removes_control_chars_and_trailing_dots() {
        assert_eq!(
            sanitize_file_name("a\u{0}b\nc\t.txt").as_deref(),
            Some("abc.txt")
        );
        assert_eq!(sanitize_file_name("name.").as_deref(), Some("name"));
        assert_eq!(sanitize_file_name("   ").as_deref(), None);
    }

    #[test]
    fn sanitize_truncates_long_names_by_chars() {
        let long = "文".repeat(260);
        let cleaned = sanitize_file_name(&long).unwrap();
        assert_eq!(cleaned.chars().count(), 200);
    }

    #[test]
    fn unique_path_adds_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (1).txt")
        );
        std::fs::write(dir.path().join("a (1).txt"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (2).txt")
        );
        // 没有扩展名也适用
        std::fs::write(dir.path().join("README"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "README"),
            dir.path().join("README (1)")
        );
    }

    #[test]
    fn query_parse_and_encode() {
        let query = parse_query("session_id=abc&token=t%2B1&file_id=f1&empty=");
        assert_eq!(query.get("session_id").map(String::as_str), Some("abc"));
        assert_eq!(query.get("token").map(String::as_str), Some("t+1"));
        assert_eq!(query.get("file_id").map(String::as_str), Some("f1"));
        assert_eq!(
            parse_query("flag").get("flag").map(String::as_str),
            Some("")
        );

        let encoded = encode_query(&[("session_id", "a b"), ("token", "t+1%")]);
        let decoded = parse_query(&encoded);
        assert_eq!(decoded.get("session_id").map(String::as_str), Some("a b"));
        assert_eq!(decoded.get("token").map(String::as_str), Some("t+1%"));
    }

    #[test]
    fn announce_roundtrip_and_validation() {
        let announce = Announce::new("我的 Mac", "fp-1", 53317);
        let json = announce.to_json();
        let parsed = Announce::from_json(json.as_bytes()).unwrap();
        assert_eq!(parsed, announce);
        assert!(parsed.is_valid());

        let mut wrong_app = announce.clone();
        wrong_app.app = "other-app".to_string();
        assert!(!wrong_app.is_valid());

        let mut wrong_version = announce.clone();
        wrong_version.version = 2;
        assert!(!wrong_version.is_valid());

        assert!(Announce::from_json(b"not-json").is_err());
    }

    #[test]
    fn parse_request_head_with_query_and_body() {
        let raw = b"POST /api/lanshare/v1/upload?session_id=s1&token=t1 HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nContent-Type: application/json\r\n\r\nhello";
        let mut reader = Cursor::new(raw.to_vec());
        let head = read_request_head(&mut reader).unwrap();
        assert_eq!(head.method, "POST");
        assert_eq!(head.path, "/api/lanshare/v1/upload");
        assert_eq!(head.query.get("session_id").map(String::as_str), Some("s1"));
        assert_eq!(head.content_length, 5);
        assert_eq!(head.header("content-type"), Some("application/json"));
        assert_eq!(read_body(&mut reader, 5).unwrap(), b"hello");
    }

    #[test]
    fn parse_request_head_rejects_bad_request_line() {
        let mut reader = Cursor::new(b"garbage\r\n\r\n".to_vec());
        assert!(read_request_head(&mut reader).is_err());
    }

    #[test]
    fn parse_json_body_works() {
        let request = PrepareUploadRequest {
            sender: SenderInfo {
                alias: "a".into(),
                fingerprint: "fp".into(),
            },
            files: vec![super::super::OfferFile {
                id: "f0".into(),
                file_name: "x.txt".into(),
                size: 3,
            }],
        };
        let bytes = serde_json::to_vec(&request).unwrap();
        let mut reader = Cursor::new(bytes);
        let parsed: PrepareUploadRequest =
            read_json_body(&mut reader, request_size(&request)).unwrap();
        assert_eq!(parsed, request);
    }

    fn request_size(request: &PrepareUploadRequest) -> usize {
        serde_json::to_vec(request).unwrap().len()
    }
}
