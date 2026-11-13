//! 接收端 HTTP 服务：手写 HTTP/1.1（只用 std），承载 info / prepare-upload /
//! upload / cancel 四个接口。
//!
//! 关键点（对应设计文档 3.2 / 3.3 / 6）：
//! - `prepare-upload` 把请求登记成「待接收」，然后在该连接的线程里用
//!   `mpsc::recv_timeout` 等用户决定，最长 60 秒（测试可调短）；
//! - 用户接受后才生成 session 与一次性 token，upload 必须带齐 session / 文件 id / token；
//! - 落盘先写 `<name>.part`，完成后原子改名，重名自动加后缀，不覆盖已有文件；
//! - 文件名在 prepare 阶段就清洗成 basename，路径穿越写不出接收目录。

use std::collections::HashSet;
use std::io::{BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;

use super::protocol::{
    self, read_json_body, read_request_head, ErrorResponse, FileToken, InfoResponse,
    PrepareUploadRequest, PrepareUploadResponse, RequestHead, SenderInfo, UploadResponse,
};
use super::{
    emit_snapshot, lock_inner, unix_millis, InnerRef, OfferFile, PendingEntry, PendingOffer,
    Session, SessionFile, MAX_FILES, MAX_FILE_SIZE, MAX_PREPARE_BODY, PORT_BASE, PROTOCOL_VERSION,
};

const INFO_PATH: &str = "/api/lanshare/v1/info";
const PREPARE_PATH: &str = "/api/lanshare/v1/prepare-upload";
const UPLOAD_PATH: &str = "/api/lanshare/v1/upload";
const CANCEL_PATH: &str = "/api/lanshare/v1/cancel";

/// 接收服务句柄：Drop 时置停止标记并 join accept 线程。
pub(crate) struct HttpServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HttpServer {
    /// 从 `base` 开始依次尝试 `count` 个端口（`base=0` 时绑定随机端口，测试用）。
    pub(crate) fn start(
        inner: InnerRef,
        app: Option<AppHandle>,
        base: u16,
        count: u16,
    ) -> Result<Self, String> {
        let (listener, port) = bind_listener(base, count)?;
        listener
            .set_nonblocking(true)
            .map_err(|err| format!("设置局域网分享服务端口非阻塞失败：{err}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || {
            loop {
                if thread_stop.load(Ordering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, source)) => {
                        let connection_inner = inner.clone();
                        let connection_app = app.clone();
                        // 每个连接一个线程：prepare 会等用户确认，不能堵住 accept
                        std::thread::spawn(move || {
                            handle_connection(connection_inner, connection_app, stream, source);
                        });
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        });
        Ok(Self {
            port,
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 在端口段里找可监听的 TCP 端口。
pub(crate) fn bind_listener(base: u16, count: u16) -> Result<(TcpListener, u16), String> {
    if base == 0 {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .map_err(|err| format!("局域网分享服务启动失败：{err}"))?;
        let port = listener
            .local_addr()
            .map_err(|err| format!("读取监听端口失败：{err}"))?
            .port();
        return Ok((listener, port));
    }
    bind_in_range(base, count, |port| {
        TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
    })
}

/// 端口回退逻辑本体：从 `base` 起逐个试，交给 `bind` 决定能否用（方便测试注入）。
fn bind_in_range(
    base: u16,
    count: u16,
    mut bind: impl FnMut(u16) -> std::io::Result<TcpListener>,
) -> Result<(TcpListener, u16), String> {
    let count = count.max(1);
    let mut last_error = String::new();
    for offset in 0..count {
        let port = base.saturating_add(offset);
        match bind(port) {
            Ok(listener) => return Ok((listener, port)),
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(format!(
        "端口 {}..{} 都被占用，无法启动局域网分享服务：{}",
        base,
        base.saturating_add(count - 1),
        last_error
    ))
}

fn handle_connection(
    inner: InnerRef,
    app: Option<AppHandle>,
    stream: TcpStream,
    source: SocketAddr,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(120)));
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream);

    let head = match read_request_head(&mut reader) {
        Ok(head) => head,
        Err(err) => {
            let _ = write_json(reader.get_mut(), 400, &ErrorResponse::new(err));
            return;
        }
    };

    let result = match (head.method.as_str(), head.path.as_str()) {
        ("GET", INFO_PATH) => handle_info(&inner, &mut reader),
        ("POST", PREPARE_PATH) => handle_prepare(&inner, app.as_ref(), &mut reader, &head, source),
        ("POST", UPLOAD_PATH) => handle_upload(&inner, app.as_ref(), &mut reader, &head),
        ("POST", CANCEL_PATH) => handle_cancel(&inner, app.as_ref(), &head, &mut reader),
        (_, path)
            if path == INFO_PATH
                || path == PREPARE_PATH
                || path == UPLOAD_PATH
                || path == CANCEL_PATH =>
        {
            Err((405, "请求方法不支持".to_string()))
        }
        _ => Err((404, "接口不存在".to_string())),
    };

    if let Err((status, message)) = result {
        let _ = write_json(reader.get_mut(), status, &ErrorResponse::new(message));
    }
}

fn handle_info(inner: &InnerRef, reader: &mut BufReader<TcpStream>) -> Result<(), (u16, String)> {
    let (alias, fingerprint, port) = {
        let guard = lock_inner(inner);
        (
            guard.effective_alias(),
            guard.settings.fingerprint.clone(),
            guard.port.unwrap_or(PORT_BASE),
        )
    };
    let response = InfoResponse {
        alias,
        fingerprint,
        port,
        version: PROTOCOL_VERSION,
    };
    write_json(reader.get_mut(), 200, &response).map_err(|err| (500, format!("写响应失败：{err}")))
}

fn handle_prepare(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    reader: &mut BufReader<TcpStream>,
    head: &RequestHead,
    source: SocketAddr,
) -> Result<(), (u16, String)> {
    if head.content_length == 0 || head.content_length > MAX_PREPARE_BODY {
        return Err((400, "接收请求体为空或过大".to_string()));
    }
    let request: PrepareUploadRequest =
        read_json_body(reader, head.content_length).map_err(|err| (400, err))?;

    let sender = SenderInfo {
        alias: request.sender.alias.trim().to_string(),
        fingerprint: request.sender.fingerprint.trim().to_string(),
    };
    if sender.fingerprint.is_empty() {
        return Err((400, "发送方指纹为空".to_string()));
    }
    if request.files.is_empty() {
        return Err((400, "没有要接收的文件".to_string()));
    }
    if request.files.len() > MAX_FILES {
        return Err((400, format!("一次最多接收 {MAX_FILES} 个文件")));
    }

    let mut total_size = 0u64;
    let mut seen_ids = HashSet::new();
    let mut files: Vec<OfferFile> = Vec::with_capacity(request.files.len());
    for (index, file) in request.files.iter().enumerate() {
        if file.size > MAX_FILE_SIZE {
            return Err((400, "单个文件超过 20GB 上限".to_string()));
        }
        total_size = total_size.saturating_add(file.size);
        let id = if file.id.trim().is_empty() {
            format!("f{index}")
        } else {
            file.id.trim().to_string()
        };
        if !seen_ids.insert(id.clone()) {
            return Err((400, "文件 id 重复".to_string()));
        }
        let file_name =
            protocol::sanitize_file_name(&file.file_name).unwrap_or_else(|| "file".to_string());
        files.push(OfferFile {
            id,
            file_name,
            size: file.size,
        });
    }

    let offer = PendingOffer {
        id: uuid::Uuid::new_v4().to_string(),
        sender_alias: if sender.alias.is_empty() {
            "未知设备".to_string()
        } else {
            sender.alias.clone()
        },
        sender_fingerprint: sender.fingerprint.clone(),
        sender_ip: source.ip().to_string(),
        total_size,
        files,
        created_at_ms: unix_millis(),
        expires_in_ms: 0,
    };
    let offer_id = offer.id.clone();
    let offer_files = offer.files.clone();

    // 先建好通道再登记，命令层拿到的一定是能送回决定的那一半；
    // 通道断开（功能被关闭 / 请求被清理）按取消处理。
    let (tx, rx) = mpsc::channel::<bool>();
    let timeout = {
        let mut guard = lock_inner(inner);
        guard.prune_sessions();
        let timeout = guard.offer_timeout;
        guard.pending.push(PendingEntry {
            offer: PendingOffer {
                expires_in_ms: timeout.as_millis() as i64,
                ..offer
            },
            responder: tx,
        });
        timeout
    };
    emit_snapshot(inner, app);

    let decision = rx.recv_timeout(timeout);
    match decision {
        Ok(true) => {}
        Ok(false) => {
            remove_pending(inner, &offer_id);
            emit_snapshot(inner, app);
            return Err((403, "对方拒绝了本次接收".to_string()));
        }
        Err(RecvTimeoutError::Timeout) => {
            remove_pending(inner, &offer_id);
            emit_snapshot(inner, app);
            return Err((408, "等待接收确认超时，请重新发送".to_string()));
        }
        Err(RecvTimeoutError::Disconnected) => {
            remove_pending(inner, &offer_id);
            emit_snapshot(inner, app);
            return Err((403, "接收已取消".to_string()));
        }
    }

    // 用户接受：建目录、生成会话与每个文件的一次性 token
    //
    // 接受与建会话之间功能可能被关闭（stop_runtime 已清空会话），这里再确认一次，
    // 避免在已关闭状态下留下一个没人管理的会话。
    if !lock_inner(inner).running {
        remove_pending(inner, &offer_id);
        emit_snapshot(inner, app);
        return Err((403, "局域网分享已关闭".to_string()));
    }
    let dir = { lock_inner(inner).download_dir.clone() };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        remove_pending(inner, &offer_id);
        emit_snapshot(inner, app);
        return Err((500, format!("无法创建接收目录 {}：{err}", dir.display())));
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut session_files = Vec::with_capacity(offer_files.len());
    let mut response_files = Vec::with_capacity(offer_files.len());
    for file in &offer_files {
        let token = uuid::Uuid::new_v4().to_string();
        session_files.push(SessionFile {
            id: file.id.clone(),
            file_name: file.file_name.clone(),
            size: file.size,
            token: token.clone(),
            received: false,
            part_path: None,
        });
        response_files.push(FileToken {
            id: file.id.clone(),
            token,
        });
    }
    {
        let mut guard = lock_inner(inner);
        guard.take_pending(&offer_id);
        guard.sessions.insert(
            session_id.clone(),
            Session {
                dir,
                files: session_files,
                created: Instant::now(),
            },
        );
    }
    emit_snapshot(inner, app);

    let response = PrepareUploadResponse {
        session_id,
        files: response_files,
    };
    write_json(reader.get_mut(), 200, &response).map_err(|err| (500, format!("写响应失败：{err}")))
}

fn handle_upload(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    reader: &mut BufReader<TcpStream>,
    head: &RequestHead,
) -> Result<(), (u16, String)> {
    let session_id = required_query(head, "session_id")?;
    let file_id = required_query(head, "file_id")?;
    let token = required_query(head, "token")?;

    let (size, dir, file_name) = {
        let mut guard = lock_inner(inner);
        guard.prune_sessions();
        let session = guard
            .sessions
            .get(&session_id)
            .ok_or_else(|| (409, "接收会话不存在或已过期".to_string()))?;
        let file = session
            .files
            .iter()
            .find(|file| file.id == file_id)
            .ok_or_else(|| (409, "接收会话里没有这个文件".to_string()))?;
        if file.received {
            return Err((409, "该文件已经接收完成".to_string()));
        }
        if file.token != token {
            return Err((409, "上传凭证无效".to_string()));
        }
        (file.size, session.dir.clone(), file.file_name.clone())
    };

    if head.content_length as u64 != size {
        return Err((
            400,
            format!(
                "文件大小与登记不一致（登记 {} 字节，实际 {} 字节）",
                size, head.content_length
            ),
        ));
    }
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return Err((500, format!("无法创建接收目录 {}：{err}", dir.display())));
    }

    let target = protocol::unique_path(&dir, &file_name);
    let part = part_path(&target);
    let mut file = std::fs::File::create(&part)
        .map_err(|err| (500, format!("创建接收文件 {} 失败：{err}", part.display())))?;
    {
        let mut guard = lock_inner(inner);
        if let Some(session) = guard.sessions.get_mut(&session_id) {
            if let Some(entry) = session.files.iter_mut().find(|file| file.id == file_id) {
                entry.part_path = Some(part.clone());
            }
        }
    }

    let copy_result = std::io::copy(&mut (&mut *reader).take(size), &mut file);
    drop(file);
    let written = match copy_result {
        Ok(written) => written,
        Err(err) => {
            let _ = std::fs::remove_file(&part);
            record_part_cleared(inner, &session_id, &file_id);
            return Err((400, format!("写入接收文件失败：{err}")));
        }
    };
    if written != size {
        let _ = std::fs::remove_file(&part);
        record_part_cleared(inner, &session_id, &file_id);
        return Err((400, "上传中断：文件大小与登记不一致".to_string()));
    }
    if let Err(err) = std::fs::rename(&part, &target) {
        let _ = std::fs::remove_file(&part);
        record_part_cleared(inner, &session_id, &file_id);
        return Err((
            500,
            format!("保存接收文件 {} 失败：{err}", target.display()),
        ));
    }

    let path = target.to_string_lossy().to_string();
    {
        let mut guard = lock_inner(inner);
        let complete = if let Some(session) = guard.sessions.get_mut(&session_id) {
            if let Some(entry) = session.files.iter_mut().find(|file| file.id == file_id) {
                entry.received = true;
                entry.part_path = None;
            }
            session.is_complete()
        } else {
            false
        };
        if complete {
            guard.remove_session(&session_id);
        }
    }
    emit_snapshot(inner, app);

    write_json(reader.get_mut(), 200, &UploadResponse { ok: true, path })
        .map_err(|err| (500, format!("写响应失败：{err}")))
}

fn handle_cancel(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    head: &RequestHead,
    reader: &mut BufReader<TcpStream>,
) -> Result<(), (u16, String)> {
    let session_id = required_query(head, "session_id")?;
    {
        let mut guard = lock_inner(inner);
        guard.remove_session(&session_id);
    }
    emit_snapshot(inner, app);
    write_json(reader.get_mut(), 200, &serde_json::json!({ "ok": true }))
        .map_err(|err| (500, format!("写响应失败：{err}")))
}

fn required_query(head: &RequestHead, name: &str) -> Result<String, (u16, String)> {
    head.query
        .get(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| (400, format!("缺少查询参数 {name}")))
}

fn remove_pending(inner: &InnerRef, offer_id: &str) {
    let mut guard = lock_inner(inner);
    guard.take_pending(offer_id);
}

fn record_part_cleared(inner: &InnerRef, session_id: &str, file_id: &str) {
    let mut guard = lock_inner(inner);
    if let Some(session) = guard.sessions.get_mut(session_id) {
        if let Some(file) = session.files.iter_mut().find(|file| file.id == file_id) {
            file.part_path = None;
        }
    }
}

fn part_path(target: &std::path::Path) -> PathBuf {
    let mut name = target.to_string_lossy().to_string();
    name.push_str(".part");
    PathBuf::from(name)
}

fn write_json<T: Serialize>(
    stream: &mut TcpStream,
    status: u16,
    payload: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());
    write_body(stream, status, "application/json; charset=utf-8", &body)
}

fn write_body(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// 测试用的 HTTP 客户端：发一个请求、读完整响应，返回状态码与响应体。
#[cfg(test)]
fn test_request(port: u16, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接测试服务失败");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    parse_response(&raw)
}

#[cfg(test)]
fn parse_response(raw: &[u8]) -> (u16, String) {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let body = String::from_utf8_lossy(&raw[(split + 4).min(raw.len())..]).to_string();
    (status, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lanshare::protocol::{
        encode_query, PrepareUploadRequest, PrepareUploadResponse, SenderInfo,
    };
    use crate::lanshare::{LanShareState, OfferFile};

    fn start_state() -> (tempfile::TempDir, LanShareState, HttpServer) {
        let dir = tempfile::tempdir().unwrap();
        let state = LanShareState::new(dir.path().join("recv"));
        let server = HttpServer::start(state.inner.clone(), None, 0, 1).unwrap();
        {
            let mut guard = state.inner.lock().unwrap();
            guard.running = true;
            guard.port = Some(server.port);
        }
        (dir, state, server)
    }

    fn prepare_body(files: &[(&str, &str, u64)]) -> Vec<u8> {
        let request = PrepareUploadRequest {
            sender: SenderInfo {
                alias: "测试发送方".to_string(),
                fingerprint: "sender-fp".to_string(),
            },
            files: files
                .iter()
                .enumerate()
                .map(|(index, (id, name, size))| OfferFile {
                    id: if id.is_empty() {
                        format!("f{index}")
                    } else {
                        id.to_string()
                    },
                    file_name: name.to_string(),
                    size: *size,
                })
                .collect(),
        };
        serde_json::to_vec(&request).unwrap()
    }

    fn wait_for_offer(state: &LanShareState) -> String {
        for _ in 0..400 {
            if let Some(entry) = state.inner.lock().unwrap().pending.first() {
                return entry.offer.id.clone();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("等待待接收请求超时");
    }

    fn accept_prepare(state: &LanShareState, port: u16, body: Vec<u8>) -> (u16, String) {
        let handle = std::thread::spawn(move || test_request(port, "POST", PREPARE_PATH, &body));
        let offer_id = wait_for_offer(state);
        state.respond_offer_inner(&offer_id, true).unwrap();
        handle.join().unwrap()
    }

    fn upload_path(response: &PrepareUploadResponse, index: usize) -> String {
        let file = &response.files[index];
        format!(
            "{UPLOAD_PATH}?{}",
            encode_query(&[
                ("session_id", response.session_id.as_str()),
                ("file_id", file.id.as_str()),
                ("token", file.token.as_str()),
            ])
        )
    }

    #[test]
    fn info_returns_identity() {
        let (_dir, state, server) = start_state();
        let (status, body) = test_request(server.port, "GET", INFO_PATH, b"");
        assert_eq!(status, 200);
        let info: InfoResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(info.fingerprint, state.snapshot().fingerprint);
        assert_eq!(info.alias, state.snapshot().alias);
        assert_eq!(info.port, server.port);
        assert_eq!(info.version, PROTOCOL_VERSION);
    }

    #[test]
    fn unknown_path_returns_404_and_wrong_method_405() {
        let (_dir, _state, server) = start_state();
        assert_eq!(test_request(server.port, "GET", "/nothing", b"").0, 404);
        assert_eq!(test_request(server.port, "GET", PREPARE_PATH, b"").0, 405);
    }

    #[test]
    fn reject_returns_403_and_clears_offer() {
        let (_dir, state, server) = start_state();
        let body = prepare_body(&[("f0", "hello.txt", 5)]);
        let port = server.port;
        let handle = std::thread::spawn(move || test_request(port, "POST", PREPARE_PATH, &body));
        let offer_id = wait_for_offer(&state);
        state.respond_offer_inner(&offer_id, false).unwrap();
        let (status, text) = handle.join().unwrap();
        assert_eq!(status, 403);
        assert!(text.contains("拒绝"), "响应体：{text}");
        assert!(state.snapshot().pending_offers.is_empty());
    }

    #[test]
    fn timeout_returns_408() {
        let (_dir, state, server) = start_state();
        {
            let mut guard = state.inner.lock().unwrap();
            guard.offer_timeout = Duration::from_millis(200);
        }
        let body = prepare_body(&[("f0", "hello.txt", 5)]);
        let (status, text) = test_request(server.port, "POST", PREPARE_PATH, &body);
        assert_eq!(status, 408);
        assert!(text.contains("超时"), "响应体：{text}");
        assert!(state.snapshot().pending_offers.is_empty());
    }

    #[test]
    fn accepted_upload_writes_file_and_completes_session() {
        let (dir, state, server) = start_state();
        let body = prepare_body(&[("f0", "hello.txt", 5)]);
        let (status, text) = accept_prepare(&state, server.port, body);
        assert_eq!(status, 200);
        let response: PrepareUploadResponse = serde_json::from_str(&text).unwrap();
        assert_eq!(response.files.len(), 1);

        let (status, text) =
            test_request(server.port, "POST", &upload_path(&response, 0), b"hello");
        assert_eq!(status, 200, "响应体：{text}");
        let upload: UploadResponse = serde_json::from_str(&text).unwrap();
        assert!(upload.ok);
        let saved = dir.path().join("recv").join("hello.txt");
        assert_eq!(std::fs::read(&saved).unwrap(), b"hello");
        assert_eq!(upload.path, saved.to_string_lossy());

        // 全部文件完成后会话被移除
        assert!(state.inner.lock().unwrap().sessions.is_empty());
    }

    #[test]
    fn wrong_token_and_duplicate_upload_are_conflicts() {
        let (_dir, state, server) = start_state();
        let body = prepare_body(&[("f0", "a.txt", 3)]);
        let (status, text) = accept_prepare(&state, server.port, body);
        assert_eq!(status, 200);
        let response: PrepareUploadResponse = serde_json::from_str(&text).unwrap();

        let bad = format!(
            "{UPLOAD_PATH}?{}",
            encode_query(&[
                ("session_id", response.session_id.as_str()),
                ("file_id", "f0"),
                ("token", "wrong-token"),
            ])
        );
        assert_eq!(test_request(server.port, "POST", &bad, b"abc").0, 409);

        assert_eq!(
            test_request(server.port, "POST", &upload_path(&response, 0), b"abc").0,
            200
        );
        // 会话完成后再次上传：会话已不存在
        assert_eq!(
            test_request(server.port, "POST", &upload_path(&response, 0), b"abc").0,
            409
        );
    }

    #[test]
    fn path_traversal_file_name_is_sanitized() {
        let (dir, state, server) = start_state();
        let body = prepare_body(&[("f0", "../../evil.txt", 3)]);
        let (status, text) = accept_prepare(&state, server.port, body);
        assert_eq!(status, 200);
        let response: PrepareUploadResponse = serde_json::from_str(&text).unwrap();
        // 登记到会话里的名字已经被清洗
        {
            let guard = state.inner.lock().unwrap();
            let session = guard.sessions.get(&response.session_id).unwrap();
            assert_eq!(session.files[0].file_name, "evil.txt");
        }
        assert_eq!(
            test_request(server.port, "POST", &upload_path(&response, 0), b"abc").0,
            200
        );
        assert_eq!(
            std::fs::read(dir.path().join("recv").join("evil.txt")).unwrap(),
            b"abc"
        );
        assert!(!dir.path().join("evil.txt").exists());
    }

    #[test]
    fn duplicate_file_name_gets_suffix() {
        let (dir, state, server) = start_state();
        std::fs::create_dir_all(dir.path().join("recv")).unwrap();
        std::fs::write(dir.path().join("recv").join("hello.txt"), b"old").unwrap();
        let body = prepare_body(&[("f0", "hello.txt", 3)]);
        let (status, text) = accept_prepare(&state, server.port, body);
        assert_eq!(status, 200);
        let response: PrepareUploadResponse = serde_json::from_str(&text).unwrap();
        assert_eq!(
            test_request(server.port, "POST", &upload_path(&response, 0), b"new").0,
            200
        );
        assert_eq!(
            std::fs::read(dir.path().join("recv").join("hello.txt")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(dir.path().join("recv").join("hello (1).txt")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn cancel_removes_session() {
        let (_dir, state, server) = start_state();
        let body = prepare_body(&[("f0", "a.txt", 3)]);
        let (status, text) = accept_prepare(&state, server.port, body);
        assert_eq!(status, 200);
        let response: PrepareUploadResponse = serde_json::from_str(&text).unwrap();
        let path = format!(
            "{CANCEL_PATH}?{}",
            encode_query(&[("session_id", response.session_id.as_str())])
        );
        assert_eq!(test_request(server.port, "POST", &path, b"").0, 200);
        assert!(state.inner.lock().unwrap().sessions.is_empty());
        assert_eq!(
            test_request(server.port, "POST", &upload_path(&response, 0), b"abc").0,
            409
        );
    }

    #[test]
    fn oversized_prepare_is_rejected() {
        let (_dir, _state, server) = start_state();
        let request = PrepareUploadRequest {
            sender: SenderInfo {
                alias: "a".into(),
                fingerprint: "fp".into(),
            },
            files: vec![OfferFile {
                id: "f0".into(),
                file_name: "x".into(),
                size: MAX_FILE_SIZE + 1,
            }],
        };
        let body = serde_json::to_vec(&request).unwrap();
        let (status, text) = test_request(server.port, "POST", PREPARE_PATH, &body);
        assert_eq!(status, 400);
        assert!(text.contains("20GB"), "响应体：{text}");
    }

    #[test]
    fn port_fallback_tries_next_when_occupied() {
        // 第一个端口固定失败，第二个成功：应返回 base+1 而不是报错
        let (listener, port) = bind_in_range(40000, 3, |port| {
            if port == 40000 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "occupied in test",
                ))
            } else {
                // 测试只看「选了哪个端口」，实际监听地址用随机端口即可
                TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            }
        })
        .unwrap();
        assert_eq!(port, 40001);
        drop(listener);

        // 全被占用时给出带端口段的错误
        let error = bind_in_range(41000, 2, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "occupied in test",
            ))
        })
        .unwrap_err();
        assert!(error.contains("41000..41001"), "错误信息：{error}");
    }
}
