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
    self, read_json_body, read_request_head, ChatRequest, ChatResponse, ErrorResponse, FileToken,
    InfoResponse, PrepareUploadRequest, PrepareUploadResponse, RequestHead, SenderInfo,
    UploadResponse,
};
use super::{
    emit_chat, emit_chat_with_direction, emit_snapshot, lock_inner, unix_millis, ChatMessage, InnerRef,
    OfferFile,
    PendingEntry, PendingOffer, Session, SessionFile, MAX_CHAT_BODY, MAX_CHAT_CHARS, MAX_FILES,
    MAX_FILE_SIZE, MAX_PREPARE_BODY, PORT_BASE, PROTOCOL_VERSION,
};

const INFO_PATH: &str = "/api/lanshare/v1/info";
const PREPARE_PATH: &str = "/api/lanshare/v1/prepare-upload";
const UPLOAD_PATH: &str = "/api/lanshare/v1/upload";
const CANCEL_PATH: &str = "/api/lanshare/v1/cancel";
const CHAT_PATH: &str = "/api/lanshare/v1/chat";

/// 收上传正文时允许的单次读等待。请求头仍然只给 30 秒（正常客户端都是秒回），
/// 但正文要跟发送端的写超时（120 秒）对齐：现场发大文件到 24% 就报
/// 「远程主机强迫关闭了一个现有的连接」，多半是链路上卡了三十秒、接收端
/// 先按超时把连接关了，发送端紧接着写就吃到 RST。
const UPLOAD_READ_TIMEOUT: Duration = Duration::from_secs(120);

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
                        // 监听 socket 为了轮询停止标记设了非阻塞；accept 出来的连接会把
                        // 这个标志带进来（macOS/BSD 一定继承，Windows 实测同样会），
                        // 于是正文还没到齐时 read 立刻返回 EAGAIN / WSAEWOULDBLOCK(10035)，
                        // 表现就是「对方未接收：读取请求体失败」或传到一半被 RST。
                        // 这里显式改回阻塞，后面的 SO_RCVTIMEO 才有意义。
                        let _ = stream.set_nonblocking(false);
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
        ("POST", CHAT_PATH) => handle_chat(&inner, app.as_ref(), &mut reader, &head),
        (_, path)
            if path == INFO_PATH
                || path == PREPARE_PATH
                || path == UPLOAD_PATH
                || path == CANCEL_PATH
                || path == CHAT_PATH =>
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

/// 接收一条聊天消息：校验 → 落库（同时更新会话 / 未读）→ 推事件。
fn handle_chat(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    reader: &mut BufReader<TcpStream>,
    head: &RequestHead,
) -> Result<(), (u16, String)> {
    if head.content_length == 0 || head.content_length > MAX_CHAT_BODY {
        return Err((400, "消息体为空或过大".to_string()));
    }
    let request: ChatRequest =
        read_json_body(reader, head.content_length).map_err(|err| (400, err))?;

    let sender = SenderInfo {
        alias: request.sender.alias.trim().to_string(),
        fingerprint: request.sender.fingerprint.trim().to_string(),
    };
    if sender.fingerprint.is_empty() {
        return Err((400, "发送方设备标识为空".to_string()));
    }
    let text = request.text.trim().to_string();
    if text.is_empty() {
        return Err((400, "消息不能为空".to_string()));
    }
    if text.chars().count() > MAX_CHAT_CHARS {
        return Err((400, format!("消息太长（最多 {MAX_CHAT_CHARS} 个字符）")));
    }
    let message_id = if request.message_id.trim().is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        request.message_id.trim().to_string()
    };
    // alias 统一按会话展示规则收敛（trim + 最长 64 字符），避免对端塞超长明文
    let alias = super::normalize_alias(&sender.alias);
    let alias = if alias.is_empty() {
        "未知设备".to_string()
    } else {
        alias
    };
    // 先取 store 再写库：避免 MutexGuard 临时量被 if-let 延长成「持锁做 DB IO」
    let chat_store = lock_inner(inner).chat.clone();
    if let Some(store) = chat_store {
        let mut message = ChatMessage::text(
            message_id,
            &sender.fingerprint,
            "in",
            text,
            unix_millis(),
        );
        message.status = "received".to_string();
        store
            .append_message(&sender.fingerprint, &alias, &message)
            .map_err(|err| (500, format!("保存消息失败：{err}")))?;
        emit_chat_with_direction(inner, app, &sender.fingerprint, "in");
    }

    write_json(reader.get_mut(), 200, &ChatResponse { ok: true })
        .map_err(|err| (500, format!("写响应失败：{err}")))
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
        sender_alias: {
            let alias = super::normalize_alias(&sender.alias);
            if alias.is_empty() {
                "未知设备".to_string()
            } else {
                alias
            }
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
    // 会话侧身份：有人发文件过来也算「交互过」，先把会话建出来
    let chat_peer_id = sender.fingerprint.clone();
    let chat_alias = {
        let alias = super::normalize_alias(&sender.alias);
        if alias.is_empty() {
            "未知设备".to_string()
        } else {
            alias
        }
    };

    // 文件请求到达就先落一个会话：左侧列表要立刻能看到这台设备；写失败只打日志。
    // 先取 store 再写库：MutexGuard 不能跨 if-let body 存活（否则等于持锁做 DB IO）。
    let chat_store = lock_inner(inner).chat.clone();
    if let Some(store) = chat_store {
        match store.upsert_conversation(&chat_peer_id, &chat_alias, unix_millis()) {
            // kind 沿用契约里的 "message"：前端对聊天数据统一按「重拉会话 / 消息」处理
            Ok(()) => emit_chat(inner, app, &chat_peer_id, "message"),
            Err(err) => eprintln!("写入文件会话失败：{err}"),
        }
    }

    // 默认直接接收：不等用户点「接受」，直接建 session 回 token，发送方立即开始上传。
    let require_accept = lock_inner(inner).settings.require_accept;
    if !require_accept {
        let (session_id, response_files) =
            attach_session(inner, &offer_files, chat_peer_id, chat_alias)?;
        emit_snapshot(inner, app);
        let response = PrepareUploadResponse {
            session_id,
            files: response_files,
        };
        return write_json(reader.get_mut(), 200, &response)
            .map_err(|err| (500, format!("写响应失败：{err}")));
    }

    // 需要确认：先建好通道再登记，命令层拿到的一定是能送回决定的那一半；
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
            return Err((408, "接收请求等待超时，请重新发送".to_string()));
        }
        Err(RecvTimeoutError::Disconnected) => {
            remove_pending(inner, &offer_id);
            emit_snapshot(inner, app);
            return Err((403, "接收已取消".to_string()));
        }
    }

    // 用户接受：建目录、生成会话与每个文件的一次性 token。
    // 接受与建会话之间功能可能被关闭，attach_session 内部会再确认一次。
    let (session_id, response_files) = match attach_session(inner, &offer_files, chat_peer_id, chat_alias)
    {
        Ok(result) => result,
        Err(err) => {
            remove_pending(inner, &offer_id);
            emit_snapshot(inner, app);
            return Err(err);
        }
    };
    remove_pending(inner, &offer_id);
    emit_snapshot(inner, app);

    let response = PrepareUploadResponse {
        session_id,
        files: response_files,
    };
    write_json(reader.get_mut(), 200, &response).map_err(|err| (500, format!("写响应失败：{err}")))
}

/// 建接收目录 + 会话 + 每个文件的一次性 token，并登记到 `inner.sessions`。
///
/// 免确认（自动接收）与用户点接受两条路径共用，避免复制出错。
fn attach_session(
    inner: &InnerRef,
    files: &[OfferFile],
    peer_id: String,
    peer_alias: String,
) -> Result<(String, Vec<FileToken>), (u16, String)> {
    // 接受与建会话之间运行时可能被重启（stop_runtime 已清空会话），这里再确认一次，
    // 避免在运行时未就绪时留下一个没人管理的会话。
    if !lock_inner(inner).running {
        return Err((403, "LANLink 服务尚未就绪".to_string()));
    }
    let dir = { lock_inner(inner).download_dir.clone() };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return Err((500, format!("无法创建接收目录 {}：{err}", dir.display())));
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut session_files = Vec::with_capacity(files.len());
    let mut response_files = Vec::with_capacity(files.len());
    for file in files {
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
        // 免确认路径不经过「登记 pending」那段，这里也要顺手清掉过期会话，
        // 否则中断上传遗留的 .part 只能等下一次 upload 才被回收。
        guard.prune_sessions();
        guard.sessions.insert(
            session_id.clone(),
            Session {
                dir,
                files: session_files,
                created: Instant::now(),
                peer_id,
                peer_alias,
            },
        );
    }
    Ok((session_id, response_files))
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

    let copy_result = {
        // 正文读取放宽到和发送端写超时一致，避免链路上稍微卡一下就被接收端先判死
        let _ = reader.get_mut().set_read_timeout(Some(UPLOAD_READ_TIMEOUT));
        std::io::copy(&mut (&mut *reader).take(size), &mut file)
    };
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
    // 落盘成功 → 聊天里记一条「收到的文件」；写失败只打日志，不影响文件本身
    let peer_info = {
        let guard = lock_inner(inner);
        guard
            .sessions
            .get(&session_id)
            .map(|session| (session.peer_id.clone(), session.peer_alias.clone()))
    };
    let chat_store = lock_inner(inner).chat.clone();
    if let (Some(store), Some((peer_id, peer_alias))) = (chat_store, peer_info) {
        let mut message = ChatMessage::file(
            uuid::Uuid::new_v4().to_string(),
            &peer_id,
            "in",
            file_name.clone(),
            size,
            "received",
            unix_millis(),
        );
        message.file_path = Some(path.clone());
        match store.append_message(&peer_id, &peer_alias, &message) {
            Ok(()) => emit_chat_with_direction(inner, app, &peer_id, "in"),
            Err(err) => eprintln!("写入接收文件消息失败：{err}"),
        }
    }
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

/// 分两次发：先请求头，隔一会儿再发正文。
///
/// 局域网里这俩通常挤在同一个报文里，所以「正文没到齐时 read 会怎样」一直没被测到。
/// 一旦走 VPN / 跨网段（MSS 更小、分段更碎），正文就会晚到几十毫秒；如果 accept
/// 出来的连接被监听 socket 的非阻塞标志带进来（BSD/macOS 一定继承，Windows 上实测
/// 也会），read 会立刻返回 EAGAIN/10035，现场表现就是「对方未接收：读取请求体失败」
/// 或传到一半被 RST。
#[cfg(test)]
fn test_request_split(port: u16, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接测试服务失败");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.flush().unwrap();
    std::thread::sleep(Duration::from_millis(300));
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

    /// 默认（require_accept=true）测试状态：文件请求会进「待接收」等待前端确认。
    fn start_state() -> (tempfile::TempDir, LanShareState, HttpServer) {
        let (dir, state) = base_state();
        // 本文件里的用例大多覆盖「待接收」流程，显式打开确认；
        // 免确认自动接收由 auto_accept_* 用例覆盖。
        state.inner.lock().unwrap().settings.require_accept = true;
        let server = HttpServer::start(state.inner.clone(), None, 0, 1).unwrap();
        {
            let mut guard = state.inner.lock().unwrap();
            guard.running = true;
            guard.port = Some(server.port);
        }
        (dir, state, server)
    }

    fn base_state() -> (tempfile::TempDir, LanShareState) {
        let dir = tempfile::tempdir().unwrap();
        let state = LanShareState::new(dir.path().join("recv"));
        (dir, state)
    }

    /// 免确认（默认设置）测试状态：文件请求直接接收，不产生 pending。
    fn start_state_auto() -> (tempfile::TempDir, LanShareState, HttpServer) {
        let (dir, state) = base_state();
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

    /// 回归：正文晚到（分两个报文）时，服务端必须等，而不是立刻报 EAGAIN。
    #[test]
    fn body_in_a_second_segment_is_read() {
        let (_dir, _state, server) = start_state();
        let body = prepare_body(&[]);
        let (status, message) = test_request_split(server.port, "POST", PREPARE_PATH, &body);
        assert_eq!(status, 400);
        assert!(
            message.contains("没有要接收的文件"),
            "正文应该被正常读到；拿到这个说明 read 在正文到达前就返回了：{message}"
        );
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
        assert_eq!(test_request(server.port, "GET", CHAT_PATH, b"").0, 405);
    }

    /// `/chat` 校验：空消息 / 超长消息返回 400，消息体为空也返回 400
    #[test]
    fn chat_request_validation() {
        let (_dir, _state, server) = start_state();
        let empty = br#"{"sender":{"alias":"B","fingerprint":"fp-b"},"message_id":"m","text":"   "}"#;
        let (status, body) = test_request(server.port, "POST", CHAT_PATH, empty);
        assert_eq!(status, 400, "响应：{body}");
        assert!(body.contains("不能为空"), "响应：{body}");

        let too_long = "a".repeat(crate::lanshare::MAX_CHAT_CHARS + 1);
        let request = crate::lanshare::protocol::ChatRequest {
            sender: SenderInfo {
                alias: "B".to_string(),
                fingerprint: "fp-b".to_string(),
            },
            message_id: "m".to_string(),
            text: too_long,
            sent_at_ms: 1,
        };
        let body = serde_json::to_vec(&request).unwrap();
        let (status, body) = test_request(server.port, "POST", CHAT_PATH, &body);
        assert_eq!(status, 400, "响应：{body}");
        assert!(body.contains("太长"), "响应：{body}");

        let missing_sender =
            br#"{"sender":{"alias":"B","fingerprint":""},"message_id":"m","text":"hi"}"#;
        let (status, body) = test_request(server.port, "POST", CHAT_PATH, missing_sender);
        assert_eq!(status, 400, "响应：{body}");
    }

    /// 聊天库不可用（测试默认 None）时 `/chat` 仍返回 ok：文件通道不能因为聊天记录挂掉
    #[test]
    fn chat_without_store_still_returns_ok() {
        let (_dir, _state, server) = start_state();
        let body = br#"{"sender":{"alias":"B","fingerprint":"fp-b"},"text":"hi","sent_at_ms":1}"#;
        let (status, response) = test_request(server.port, "POST", CHAT_PATH, body);
        assert_eq!(status, 200, "响应：{response}");
        assert!(response.contains("\"ok\":true"), "响应：{response}");
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

    /// 默认设置（require_accept=false）：prepare 直接返回 token，不产生待接收请求，
    /// 随后 upload 正常落盘。
    #[test]
    fn auto_accept_prepare_returns_tokens_without_pending() {
        let (dir, state, server) = start_state_auto();
        assert!(
            !state.snapshot().require_accept,
            "新状态默认免确认，设置页勾选默认不勾"
        );
        let body = prepare_body(&[("f0", "auto.txt", 5)]);
        let (status, text) = test_request(server.port, "POST", PREPARE_PATH, &body);
        assert_eq!(status, 200, "响应体：{text}");
        let response: PrepareUploadResponse = serde_json::from_str(&text).unwrap();
        assert_eq!(response.files.len(), 1);
        assert_eq!(response.files[0].id, "f0");
        assert!(!response.files[0].token.is_empty());
        // 不产生待接收请求：左栏不会出现「待接收」
        assert!(state.snapshot().pending_offers.is_empty());
        // 会话已登记，可以直接上传
        assert_eq!(state.inner.lock().unwrap().sessions.len(), 1);

        let (status, text) =
            test_request(server.port, "POST", &upload_path(&response, 0), b"hello");
        assert_eq!(status, 200, "响应体：{text}");
        let saved = dir.path().join("recv").join("auto.txt");
        assert_eq!(std::fs::read(&saved).unwrap(), b"hello");
        assert!(state.inner.lock().unwrap().sessions.is_empty());
    }

    /// 免确认路径不经过 pending 登记，也要顺手清理过期会话（否则中断上传的
    /// `.part` 要等下一次 upload 才回收）。
    #[test]
    fn auto_accept_prunes_expired_sessions() {
        let (dir, state, server) = start_state_auto();
        {
            let mut guard = state.inner.lock().unwrap();
            // 把 TTL 调到 1ms 再注入一个「建完就过期」的会话。不用
            // `Instant::now() - ttl`：机器刚启动不到 TTL 时长时减法会溢出 panic。
            guard.session_ttl = Duration::from_millis(1);
            guard.sessions.insert(
                "stale".to_string(),
                Session {
                    dir: dir.path().join("stale"),
                    files: Vec::new(),
                    created: Instant::now(),
                    peer_id: "peer".to_string(),
                    peer_alias: "peer".to_string(),
                },
            );
        }
        std::thread::sleep(Duration::from_millis(10));
        let body = prepare_body(&[("f0", "prune.txt", 1)]);
        let (status, text) = test_request(server.port, "POST", PREPARE_PATH, &body);
        assert_eq!(status, 200, "响应体：{text}");
        let guard = state.inner.lock().unwrap();
        assert!(
            !guard.sessions.contains_key("stale"),
            "自动接收也要清掉过期会话"
        );
        assert_eq!(guard.sessions.len(), 1, "只留下本次新建的会话");
    }

    /// 开启确认后仍走原来的待接收流程（此处只验证 403 拒绝路径没被改坏）。
    #[test]
    fn require_accept_keeps_pending_flow() {
        let (_dir, state, server) = start_state();
        assert!(state.snapshot().require_accept);
        let body = prepare_body(&[("f0", "confirm.txt", 1)]);
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
