//! 发送端：探活 → prepare-upload → 按 token 逐文件上传 → 进度事件。
//!
//! 只用 `std::net::TcpStream` 手写 HTTP 请求，不引第三方 HTTP 客户端；
//! 大文件按 256KB 分块读写，内存占用与文件大小无关。
//!
//! 失败约定（设计文档 6）：任何失败都推一条 `done=true, error=Some(..)` 的进度，
//! 面板能直接展示原因；已经建立会话的，会尽力 POST cancel 让接收端清理。

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde::de::DeserializeOwned;
use tauri::AppHandle;

use super::protocol::{
    encode_query, ChatRequest, ErrorResponse, FileToken, InfoResponse, PrepareUploadRequest,
    PrepareUploadResponse, SenderInfo,
};
use super::{
    emit_chat, emit_progress, emit_snapshot, lock_inner, unix_millis, InnerRef, OfferFile,
    SendProgress, MAX_FILE_SIZE,
};

/// 上传分块大小：每块推一次进度
const CHUNK_SIZE: usize = 256 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// prepare 最长等 60 秒确认，这里留 30 秒余量
const PREPARE_TIMEOUT: Duration = Duration::from_secs(90);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
/// 发一条聊天消息的等待上限（局域网内应该毫秒级返回）
const CHAT_TIMEOUT: Duration = Duration::from_secs(5);

/// 发送目标（从 peer 快照里取）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendPeer {
    pub fingerprint: String,
    pub alias: String,
    pub ip: String,
    pub port: u16,
    /// 对端自报的全部 IPv4（含 `ip`）；发送前逐个探测，挑第一个连得上的
    pub addresses: Vec<String>,
}

/// 一个待发送的本地文件（`send_files` 也要用它来生成聊天里的文件消息）
#[derive(Debug, Clone)]
pub(crate) struct LocalFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
}

/// 发送入口：`send_files` 命令在后台线程里调用它。
///
/// `message_ids` 与文件一一对应，用来把发送结果写回聊天里的文件消息；
/// 测试 / 不关心聊天记录时传空切片即可。
pub(crate) fn run_send(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    peer: &SendPeer,
    paths: &[String],
    transfer_id: &str,
    message_ids: &[String],
) -> Result<(), String> {
    let files = match collect_files(paths) {
        Ok(files) => files,
        Err(message) => {
            // 极窄竞态：send_files 收集后、后台线程收集前文件被删/改名。
            // 这时还没有任何网络请求，但聊天消息已经落库，必须回写 failed，
            // 否则气泡会永久停在「发送中」。
            fail_messages(inner, app, peer, message_ids, 0, &message);
            return Err(message);
        }
    };
    let file_total = files.len();
    let (sender_alias, sender_fingerprint) = {
        let guard = lock_inner(inner);
        (guard.effective_alias(), guard.settings.fingerprint.clone())
    };

    // 1. 选地址 + 探活一次搞定：逐个候选发 /info，第一个通的既是「能用的地址」也是「对面在监听」
    let peer = match probe_peer(peer, inner, app) {
        Ok(peer) => peer,
        Err(message) => {
            fail_messages(inner, app, peer, message_ids, 0, &message);
            emit_failure(inner, app, peer, transfer_id, &files, 0, &message);
            return Err(message);
        }
    };
    let peer = &peer;

    // 2. 提交文件清单，等接收方确认（最长 60 秒，由服务端控制）
    let offer_files: Vec<OfferFile> = files
        .iter()
        .enumerate()
        .map(|(index, file)| OfferFile {
            id: format!("f{index}"),
            file_name: file.name.clone(),
            size: file.size,
        })
        .collect();
    let prepare_body = serde_json::to_vec(&PrepareUploadRequest {
        sender: SenderInfo {
            alias: sender_alias,
            fingerprint: sender_fingerprint,
        },
        files: offer_files,
    })
    .map_err(|err| format!("整理发送清单失败：{err}"))?;
    let prepared: PrepareUploadResponse = match request_json(
        peer,
        "POST",
        "/api/lanshare/v1/prepare-upload",
        Some(&prepare_body),
        PREPARE_TIMEOUT,
    ) {
        Ok(response) => response,
        Err(err) => {
            let message = format!("对方未接收：{err}");
            fail_messages(inner, app, peer, message_ids, 0, &message);
            emit_failure(inner, app, peer, transfer_id, &files, 0, &message);
            return Err(message);
        }
    };

    // 3. 逐个文件上传
    for (index, file) in files.iter().enumerate() {
        let file_id = format!("f{index}");
        let token = match prepared.files.iter().find(|item| item.id == file_id) {
            Some(token) => token.clone(),
            None => {
                let message = format!("对方没有返回「{}」的上传凭证", file.name);
                fail_messages(inner, app, peer, message_ids, index, &message);
                emit_failure(inner, app, peer, transfer_id, &files, index, &message);
                cancel_session(peer, &prepared.session_id);
                return Err(message);
            }
        };
        let path = upload_path(&prepared.session_id, &file_id, &token);
        let mut progress = SendProgress {
            transfer_id: transfer_id.to_string(),
            peer_alias: peer.alias.clone(),
            file_name: file.name.clone(),
            file_index: index,
            file_total,
            sent_bytes: 0,
            total_bytes: file.size,
            done: false,
            error: None,
        };
        emit_progress(inner, app, progress.clone());

        let result = upload_file(peer, &path, &file.path, file.size, |sent| {
            progress.sent_bytes = sent;
            emit_progress(inner, app, progress.clone());
        });
        match result {
            Ok(()) => {
                progress.sent_bytes = file.size;
                progress.done = true;
                emit_progress(inner, app, progress);
                update_file_message(inner, app, peer, message_ids, index, "sent", None);
            }
            Err(err) => {
                let message = format!("发送「{}」失败：{err}", file.name);
                progress.done = true;
                progress.error = Some(message.clone());
                emit_progress(inner, app, progress);
                fail_messages(inner, app, peer, message_ids, index, &message);
                cancel_session(peer, &prepared.session_id);
                return Err(message);
            }
        }
    }
    Ok(())
}

fn upload_path(session_id: &str, file_id: &str, token: &FileToken) -> String {
    format!(
        "/api/lanshare/v1/upload?{}",
        encode_query(&[
            ("session_id", session_id),
            ("file_id", file_id),
            ("token", token.token.as_str()),
        ])
    )
}

/// 把单个文件的发送结果写回聊天里的文件消息（测试 / 无聊天记录时 `message_ids` 为空）。
fn update_file_message(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    peer: &SendPeer,
    message_ids: &[String],
    index: usize,
    status: &str,
    error: Option<&str>,
) {
    let Some(message_id) = message_ids.get(index).map(String::as_str) else {
        return;
    };
    if message_id.is_empty() {
        return;
    }
    let Some(store) = lock_inner(inner).chat.clone() else {
        return;
    };
    if let Err(err) =
        store.update_message_status(&peer.fingerprint, message_id, status, error, None)
    {
        eprintln!("更新文件消息状态失败：{err}");
        return;
    }
    emit_chat(inner, app, &peer.fingerprint, "message");
}

/// 从 `from_index` 起把还没发出去的文件消息标成失败。
fn fail_messages(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    peer: &SendPeer,
    message_ids: &[String],
    from_index: usize,
    message: &str,
) {
    for index in from_index..message_ids.len() {
        update_file_message(inner, app, peer, message_ids, index, "failed", Some(message));
    }
}

/// 发送一条聊天消息：逐个候选地址 POST `/chat`，结束后把消息状态写回并推事件。
pub(crate) fn run_send_chat(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    peer: &SendPeer,
    message_id: &str,
    text: &str,
) -> Result<(), String> {
    let (sender_alias, sender_fingerprint) = {
        let guard = lock_inner(inner);
        (guard.effective_alias(), guard.settings.fingerprint.clone())
    };
    let request = ChatRequest {
        sender: SenderInfo {
            alias: sender_alias,
            fingerprint: sender_fingerprint,
        },
        message_id: message_id.to_string(),
        text: text.to_string(),
        sent_at_ms: unix_millis(),
    };
    let body = serde_json::to_vec(&request).map_err(|err| format!("整理消息失败：{err}"))?;
    let outcome = post_chat(peer, &body);
    let (status, error) = match outcome {
        Ok((code, _payload)) if (200..300).contains(&code) => ("sent", None),
        Ok((code, payload)) => ("failed", Some(response_error(code, &payload))),
        Err(err) => ("failed", Some(err)),
    };
    let chat_store = lock_inner(inner).chat.clone();
    if let Some(store) = chat_store {
        if let Err(err) = store.update_message_status(
            &peer.fingerprint,
            message_id,
            status,
            error.as_deref(),
            None,
        ) {
            eprintln!("更新消息状态失败：{err}");
        }
        emit_chat(inner, app, &peer.fingerprint, "message");
    }
    match error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn post_chat(peer: &SendPeer, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let mut last_error = String::new();
    for ip in candidate_ips(peer) {
        let mut candidate = peer.clone();
        candidate.ip = ip;
        match request_raw(
            &candidate,
            "POST",
            "/api/lanshare/v1/chat",
            Some(body),
            CHAT_TIMEOUT,
        ) {
            Ok(response) => return Ok(response),
            Err(err) => last_error = err,
        }
    }
    Err(last_error)
}

pub(crate) fn collect_files(paths: &[String]) -> Result<Vec<LocalFile>, String> {
    if paths.is_empty() {
        return Err("请先选择要发送的文件".to_string());
    }
    let mut files = Vec::with_capacity(paths.len());
    for raw in paths {
        let path = PathBuf::from(raw);
        let metadata = std::fs::metadata(&path)
            .map_err(|err| format!("无法读取文件 {}：{err}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("不是文件：{}", path.display()));
        }
        if metadata.len() > MAX_FILE_SIZE {
            return Err(format!(
                "「{}」超过 20GB 上限，无法发送",
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| raw.clone())
            ));
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "file".to_string());
        files.push(LocalFile {
            path,
            name,
            size: metadata.len(),
        });
    }
    Ok(files)
}

fn emit_failure(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    peer: &SendPeer,
    transfer_id: &str,
    files: &[LocalFile],
    index: usize,
    message: &str,
) {
    let file = files.get(index.min(files.len().saturating_sub(1)));
    let progress = SendProgress {
        transfer_id: transfer_id.to_string(),
        peer_alias: peer.alias.clone(),
        file_name: file.map(|file| file.name.clone()).unwrap_or_default(),
        file_index: index,
        file_total: files.len(),
        sent_bytes: 0,
        total_bytes: file.map(|file| file.size).unwrap_or(0),
        done: true,
        error: Some(message.to_string()),
    };
    emit_progress(inner, app, progress);
}

fn cancel_session(peer: &SendPeer, session_id: &str) {
    let path = format!(
        "/api/lanshare/v1/cancel?{}",
        encode_query(&[("session_id", session_id)])
    );
    let _ = request_raw(peer, "POST", &path, Some(&[]), CANCEL_TIMEOUT);
}

fn upload_file(
    peer: &SendPeer,
    path: &str,
    file_path: &std::path::Path,
    size: u64,
    mut on_progress: impl FnMut(u64),
) -> Result<(), String> {
    let addr = peer_addr(peer)?;
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|err| format!("连接 {} 失败：{err}", addr))?;
    stream
        .set_write_timeout(Some(UPLOAD_TIMEOUT))
        .map_err(|err| format!("设置发送超时失败：{err}"))?;
    stream
        .set_read_timeout(Some(UPLOAD_TIMEOUT))
        .map_err(|err| format!("设置接收超时失败：{err}"))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/octet-stream\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|err| format!("发送请求头失败：{err}"))?;

    let mut file = File::open(file_path)
        .map_err(|err| format!("打开文件 {} 失败：{err}", file_path.display()))?;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut sent = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("读取文件 {} 失败：{err}", file_path.display()))?;
        if read == 0 {
            break;
        }
        stream
            .write_all(&buffer[..read])
            .map_err(|err| format!("写入对端连接失败：{}", io_error(&err)))?;
        sent += read as u64;
        on_progress(sent);
    }
    if sent != size {
        return Err(format!(
            "文件在发送过程中发生变化（登记 {} 字节，实际读了 {} 字节）",
            size, sent
        ));
    }
    let (status, payload) = read_response(stream)?;
    if !(200..300).contains(&status) {
        return Err(response_error(status, &payload));
    }
    Ok(())
}

fn request_json<T: DeserializeOwned>(
    peer: &SendPeer,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<T, String> {
    let (status, payload) = request_raw(peer, method, path, body, timeout)?;
    if !(200..300).contains(&status) {
        return Err(response_error(status, &payload));
    }
    serde_json::from_slice::<T>(&payload).map_err(|err| format!("解析响应失败：{err}"))
}

fn request_raw(
    peer: &SendPeer,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    let addr = peer_addr(peer)?;
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|err| format!("连接 {} 失败：{err}", addr))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| format!("设置读超时失败：{err}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|err| format!("设置写超时失败：{err}"))?;
    let body = body.unwrap_or(&[]);
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|err| format!("发送请求失败：{err}"))?;
    stream
        .write_all(body)
        .map_err(|err| format!("发送请求体失败：{err}"))?;
    read_response(stream)
}

fn peer_addr(peer: &SendPeer) -> Result<SocketAddr, String> {
    format!("{}:{}", peer.ip, peer.port)
        .parse::<SocketAddr>()
        .map_err(|err| format!("对端地址无效 {}:{}：{err}", peer.ip, peer.port))
}

/// 单个候选地址的探测超时
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// 候选地址：`peer.ip` 优先，其余来自对端自报的 `addresses`（去重、保序）。
fn candidate_ips(peer: &SendPeer) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for raw in std::iter::once(&peer.ip).chain(peer.addresses.iter()) {
        let value = raw.trim();
        if value.is_empty() || candidates.iter().any(|item| item == value) {
            continue;
        }
        candidates.push(value.to_string());
    }
    candidates
}

/// 地址选定后写回 peer 表（面板上显示的就是这个地址）。
fn remember_peer_ip(peer: &SendPeer, ip: &str, inner: &InnerRef, app: Option<&AppHandle>) -> bool {
    if ip == peer.ip {
        return false;
    }
    let changed = lock_inner(inner).set_peer_ip(&peer.fingerprint, ip);
    if changed {
        emit_snapshot(inner, app);
    }
    changed
}

/// 探活 + 选地址：逐个候选发 `/info`，第一个通的就用它。
///
/// 现场问题：两台机器都在 `192.168.3.0/24`，但各自连着 VPN，VPN 又推送了
/// `192.168.3.0/24` 的路由 —— 于是「局域网地址」反而不可达，通的只有 VPN 地址。
/// 以前只用 announce 的来源地址发一次，必然失败。
fn probe_peer(
    peer: &SendPeer,
    inner: &InnerRef,
    app: Option<&AppHandle>,
) -> Result<SendPeer, String> {
    let mut tried: Vec<String> = Vec::new();
    let mut last_error = String::new();
    for ip in candidate_ips(peer) {
        let mut candidate = peer.clone();
        candidate.ip = ip.clone();
        match request_json::<InfoResponse>(
            &candidate,
            "GET",
            "/api/lanshare/v1/info",
            None,
            PROBE_TIMEOUT,
        ) {
            Ok(_) => {
                remember_peer_ip(peer, &ip, inner, app);
                return Ok(candidate);
            }
            Err(err) => {
                last_error = err;
                tried.push(format!("{ip}:{}", peer.port));
            }
        }
    }
    let mut message = format!("连接对方设备失败：{last_error}");
    if tried.len() > 1 {
        message.push_str(&format!(
            "（试过 {}；开了 VPN / 多网卡时请确认两端能直接互相访问）",
            tried.join("、")
        ));
    }
    Err(message)
}

/// 纯 TCP 探测（不发请求），给发现线程挑「面板上显示哪个地址」用。
///
/// 真正要发文件时还会走 `probe_peer` 用 `/info` 再确认一次。
pub(crate) fn resolve_peer(
    peer: &SendPeer,
    inner: &InnerRef,
    app: Option<&AppHandle>,
) -> Result<SendPeer, String> {
    let mut tried: Vec<String> = Vec::new();
    for ip in candidate_ips(peer) {
        let Ok(addr) = format!("{ip}:{}", peer.port).parse::<SocketAddr>() else {
            continue;
        };
        if TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok() {
            remember_peer_ip(peer, &ip, inner, app);
            let mut resolved = peer.clone();
            resolved.ip = ip;
            return Ok(resolved);
        }
        tried.push(format!("{ip}:{}", peer.port));
    }
    Err(format!("都连不上：{}", tried.join("、")))
}

/// 把网络 IO 错误翻成用户能对上号的说法。
fn io_error(err: &std::io::Error) -> String {
    let mut text = err.to_string();
    let reset = err.kind() == std::io::ErrorKind::ConnectionReset
        || matches!(err.raw_os_error(), Some(10054) | Some(104) | Some(54));
    if reset {
        text.push_str(
            "（对端在传输中途断开了连接：接收端应用被关掉、切换网络或 VPN 掉线都会这样）",
        );
    }
    text
}

fn read_response(stream: TcpStream) -> Result<(u16, Vec<u8>), String> {
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    let read = reader
        .read_line(&mut status_line)
        .map_err(|err| format!("读取响应失败：{}", io_error(&err)))?;
    if read == 0 {
        return Err("对端没有返回响应".to_string());
    }
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "响应状态行格式错误".to_string())?;

    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|err| format!("读取响应头失败：{err}"))?;
        if read == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut payload = Vec::new();
    match content_length {
        Some(length) => {
            payload.resize(length, 0);
            reader
                .read_exact(&mut payload)
                .map_err(|err| format!("读取响应体失败：{err}"))?;
        }
        None => {
            reader
                .read_to_end(&mut payload)
                .map_err(|err| format!("读取响应体失败：{err}"))?;
        }
    }
    Ok((status, payload))
}

fn response_error(status: u16, payload: &[u8]) -> String {
    let parsed = serde_json::from_slice::<ErrorResponse>(payload)
        .map(|error| error.error)
        .unwrap_or_else(|_| String::from_utf8_lossy(payload).to_string());
    let parsed = parsed.trim();
    if parsed.is_empty() {
        format!("对端返回 HTTP {status}")
    } else if status == 408 {
        "对方未在 60 秒内确认接收".to_string()
    } else {
        parsed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lanshare::protocol::{read_body, read_request_head};
    use crate::lanshare::{LanShareState, SendProgress};
    use std::io::BufReader;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[derive(Debug, Clone)]
    struct MockRequest {
        method: String,
        path: String,
        query: std::collections::HashMap<String, String>,
        headers: String,
        body: Vec<u8>,
    }

    /// 起一个只应答一次发送流程的 mock 接收端：
    /// info → prepare → upload × 文件数，共 `connections` 个连接。
    fn start_mock(
        connections: usize,
        prepare_status: u16,
        prepare_response: &'static str,
    ) -> (
        u16,
        Arc<Mutex<Vec<MockRequest>>>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_thread = captured.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..connections {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let head = read_request_head(&mut reader).expect("解析 mock 请求头");
                let body = read_body(&mut reader, head.content_length).unwrap();
                captured_thread.lock().unwrap().push(MockRequest {
                    method: head.method.clone(),
                    path: head.path.clone(),
                    query: head.query.clone(),
                    headers: format!("{:?}", head.headers),
                    body,
                });
                let (status, payload) = match head.path.as_str() {
                    "/api/lanshare/v1/info" => (
                        200,
                        r#"{"alias":"mock-peer","fingerprint":"mock-fp","port":53317,"version":1}"#
                            .to_string(),
                    ),
                    "/api/lanshare/v1/prepare-upload" => {
                        (prepare_status, prepare_response.to_string())
                    }
                    "/api/lanshare/v1/upload" => (
                        200,
                        r#"{"ok":true,"path":"/tmp/mock-received"}"#.to_string(),
                    ),
                    _ => (200, r#"{"ok":true}"#.to_string()),
                };
                write_mock_response(stream, status, &payload);
            }
        });
        (port, captured, handle)
    }

    fn write_mock_response(mut stream: std::net::TcpStream, status: u16, body: &str) {
        let head = format!(
            "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    fn temp_state() -> (tempfile::TempDir, LanShareState) {
        let dir = tempfile::tempdir().unwrap();
        let state = LanShareState::new(dir.path().join("recv"));
        (dir, state)
    }

    fn write_file(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().to_string()
    }

    fn peer(port: u16) -> SendPeer {
        SendPeer {
            fingerprint: "peer-fp".to_string(),
            alias: "对端设备".to_string(),
            ip: "127.0.0.1".to_string(),
            port,
            addresses: vec!["127.0.0.1".to_string()],
        }
    }

    const PREPARE_OK: &str = r#"{"session_id":"session-1","files":[{"id":"f0","token":"tok-0"},{"id":"f1","token":"tok-1"}]}"#;

    #[test]
    fn sends_two_files_with_tokens_and_progress() {
        let (_state_dir, state) = temp_state();
        let (port, captured, mock) = start_mock(4, 200, PREPARE_OK);
        let files_dir = tempfile::tempdir().unwrap();
        let first = write_file(files_dir.path(), "a.txt", b"hello lanshare");
        // 第二份大于一个分块，覆盖「分块进度」
        let big = vec![7u8; CHUNK_SIZE + 1024];
        let second = write_file(files_dir.path(), "b.bin", &big);

        let result = run_send(
            &state.inner,
            None,
            &peer(port),
            &[first.clone(), second.clone()],
            "transfer-1",
            &[],
        );
        mock.join().unwrap();
        assert!(result.is_ok(), "发送失败：{result:?}");

        let requests = captured.lock().unwrap().clone();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/api/lanshare/v1/info");
        assert_eq!(requests[1].method, "POST");
        assert_eq!(requests[1].path, "/api/lanshare/v1/prepare-upload");
        let prepared: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(
            prepared["sender"]["fingerprint"],
            state.snapshot().fingerprint
        );
        assert_eq!(prepared["files"][0]["file_name"], "a.txt");
        assert_eq!(prepared["files"][1]["size"], (CHUNK_SIZE + 1024) as u64);

        // 两个 upload 请求：token / session / 文件 id 都带上，body 字节完全一致
        assert_eq!(requests[2].path, "/api/lanshare/v1/upload");
        assert_eq!(
            requests[2].query.get("session_id").map(String::as_str),
            Some("session-1")
        );
        assert_eq!(
            requests[2].query.get("file_id").map(String::as_str),
            Some("f0")
        );
        assert_eq!(
            requests[2].query.get("token").map(String::as_str),
            Some("tok-0")
        );
        assert_eq!(requests[2].body, b"hello lanshare");
        assert!(requests[2].headers.contains("content-length"));
        assert_eq!(requests[3].body, big);
        assert_eq!(
            requests[3].query.get("file_id").map(String::as_str),
            Some("f1")
        );
        assert_eq!(
            requests[3].query.get("token").map(String::as_str),
            Some("tok-1")
        );

        // 进度：两个文件各有初始 0 字节、分块、完成（done=true）
        let progress: Vec<SendProgress> = state.inner.lock().unwrap().recorded_progress.clone();
        let done: Vec<&SendProgress> = progress.iter().filter(|item| item.done).collect();
        assert_eq!(done.len(), 2);
        assert!(done.iter().all(|item| item.error.is_none()));
        assert_eq!(done[0].file_name, "a.txt");
        assert_eq!(done[0].sent_bytes, done[0].total_bytes);
        assert_eq!(done[1].file_name, "b.bin");
        assert_eq!(done[1].sent_bytes, (CHUNK_SIZE + 1024) as u64);
        // 大文件至少推进两次（256KB 分块）
        let big_updates: Vec<&SendProgress> = progress
            .iter()
            .filter(|item| item.file_name == "b.bin")
            .collect();
        assert!(
            big_updates.len() >= 3,
            "大文件进度条数：{}",
            big_updates.len()
        );
        // 进度单调递增
        let mut last = 0u64;
        for item in &big_updates {
            assert!(item.sent_bytes >= last);
            last = item.sent_bytes;
        }
        assert_eq!(progress[0].done, false);
        assert_eq!(progress[0].sent_bytes, 0);
    }

    #[test]
    fn rejected_prepare_reports_error_progress() {
        let (_state_dir, state) = temp_state();
        let (port, _captured, mock) = start_mock(2, 403, r#"{"error":"对方拒绝了本次接收"}"#);
        let files_dir = tempfile::tempdir().unwrap();
        let file = write_file(files_dir.path(), "a.txt", b"hi");
        let result = run_send(&state.inner, None, &peer(port), &[file], "transfer-2", &[]);
        mock.join().unwrap();
        assert!(result.is_err());
        let message = result.unwrap_err();
        assert!(message.contains("拒绝"), "错误信息：{message}");

        let progress = state.inner.lock().unwrap().recorded_progress.clone();
        assert_eq!(progress.len(), 1);
        assert!(progress[0].done);
        assert_eq!(progress[0].file_index, 0);
        assert!(progress[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("拒绝"));
    }

    #[test]
    fn unreachable_peer_reports_error_progress() {
        let (_state_dir, state) = temp_state();
        let files_dir = tempfile::tempdir().unwrap();
        let file = write_file(files_dir.path(), "a.txt", b"hi");
        // 拿一个刚释放的端口，连接必然失败
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let started = Instant::now();
        let result = run_send(&state.inner, None, &peer(port), &[file], "transfer-3", &[]);
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
        let progress = state.inner.lock().unwrap().recorded_progress.clone();
        assert_eq!(progress.len(), 1);
        assert!(progress[0].done);
        assert!(progress[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("连接"));
    }

    #[test]
    fn missing_local_file_fails_before_network() {
        let (_state_dir, state) = temp_state();
        let result = run_send(
            &state.inner,
            None,
            &peer(53317),
            &["/definitely/not/here.txt".to_string()],
            "transfer-4",
            &[],
        );
        assert!(result.is_err());
        let progress = state.inner.lock().unwrap().recorded_progress.clone();
        assert!(progress.is_empty(), "本地文件缺失不应产生网络进度");
    }
}
