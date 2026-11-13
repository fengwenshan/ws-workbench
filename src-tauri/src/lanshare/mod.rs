//! 局域网分享（LocalSend 风格）的运行时状态与入口。
//!
//! 设计文档：`docs/superpowers/specs/2026-11-13-lan-share-design.md`
//!
//! 模块分工：
//! - `protocol`：报文结构、文件名清洗、HTTP 头解析
//! - `settings`：`lan_share.json` 持久化
//! - `discovery`：UDP 组播收发与 peer 表维护
//! - `server`：TCP HTTP 接收服务（prepare-upload 等待用户确认、upload 流式落盘）
//! - `client`：发送端（探活、prepare-upload、逐文件上传、进度事件）
//!
//! 事件发送统一走 `Option<AppHandle>`：命令和后台线程传 `Some`，测试传 `None`，
//! 这样测试不依赖 Tauri 运行时。

pub(crate) mod client;
pub(crate) mod discovery;
pub(crate) mod protocol;
pub(crate) mod server;
pub(crate) mod settings;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use protocol::Announce;

/// 组播报文里的应用标识：只认真实本应用，别的进程发的一律忽略。
pub const APP_TAG: &str = "ws-workbench-lanshare";
/// 协议版本（announce 的 version 字段）
pub const PROTOCOL_VERSION: u32 = 1;
/// 设备类型（本应用只有桌面端）
pub const DEVICE_TYPE: &str = "desktop";

/// 组播组与发现端口（LocalSend 同款地址，仅借思路不做官方互通）
pub const MULTICAST_GROUP: [u8; 4] = [224, 0, 0, 167];
pub const DISCOVERY_PORT: u16 = 53317;
/// HTTP 服务端口：默认 53317，被占用时依次试到 53326
pub const PORT_BASE: u16 = 53317;
pub const PORT_COUNT: u16 = 10;

/// announce 间隔与 peer 过期时间
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3);
pub const PEER_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次准备接收的确认等待上限（设计文档 3.3）
pub const OFFER_TIMEOUT: Duration = Duration::from_secs(60);
/// 传输会话语义上的有效期（设计文档 4.2/6）
pub const SESSION_TTL: Duration = Duration::from_secs(300);

/// 单次最多文件数 / 单文件大小上限（防误传）
pub const MAX_FILES: usize = 100;
pub const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024 * 1024;
/// prepare-upload 请求体上限
pub const MAX_PREPARE_BODY: usize = 2 * 1024 * 1024;

/// 事件名
pub const EVENT_SNAPSHOT: &str = "lan-share://snapshot";
pub const EVENT_SEND_PROGRESS: &str = "lan-share://send-progress";

/// 设置持久化的 store 文件与键
pub(crate) const STORE_FILE: &str = "lan_share.json";
pub(crate) const STORE_KEY: &str = "settings";

/// 共享的运行时状态句柄：命令线程、发现线程、HTTP 线程、发送线程都拿这个 Arc。
pub type InnerRef = Arc<Mutex<Inner>>;

/// 持久化设置（设计文档 4.2）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanShareSettings {
    #[serde(default)]
    pub enabled: bool,
    /// 设备名；空串表示用主机名
    #[serde(default)]
    pub alias: String,
    /// 用户指定的接收目录；None / 空串表示用默认目录
    #[serde(default)]
    pub download_dir: Option<String>,
    /// 稳定指纹：首次开启时生成，之后一直复用
    #[serde(default)]
    pub fingerprint: String,
}

impl LanShareSettings {
    pub fn new_with_fingerprint() -> Self {
        Self {
            enabled: false,
            alias: String::new(),
            download_dir: None,
            fingerprint: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// 兼容老配置：指纹为空时补一个（调用方负责回写）
    pub fn ensure_fingerprint(&mut self) -> bool {
        if self.fingerprint.trim().is_empty() {
            self.fingerprint = uuid::Uuid::new_v4().to_string();
            true
        } else {
            false
        }
    }
}

impl Default for LanShareSettings {
    fn default() -> Self {
        Self::new_with_fingerprint()
    }
}

/// 发现到的一台设备（设计文档 4.2）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanPeer {
    pub fingerprint: String,
    pub alias: String,
    pub ip: String,
    pub port: u16,
    pub device_type: String,
    /// 最近一次收到 announce 的 Unix 毫秒时间，面板用来显示「最近在线」
    pub last_seen_ms: i64,
}

/// 发送方提交的单个文件清单项
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfferFile {
    pub id: String,
    pub file_name: String,
    pub size: u64,
}

/// 等待用户确认的接收请求（设计文档 4.2）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingOffer {
    pub id: String,
    pub sender_alias: String,
    pub sender_fingerprint: String,
    pub sender_ip: String,
    pub total_size: u64,
    pub files: Vec<OfferFile>,
    pub created_at_ms: i64,
    pub expires_in_ms: i64,
}

/// 发送进度事件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SendProgress {
    pub transfer_id: String,
    pub peer_alias: String,
    pub file_name: String,
    pub file_index: usize,
    pub file_total: usize,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub done: bool,
    pub error: Option<String>,
}

/// 面板初始化 / 刷新用的完整快照
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanShareSnapshot {
    pub enabled: bool,
    pub running: bool,
    pub alias: String,
    pub fingerprint: String,
    pub port: Option<u16>,
    pub download_dir: String,
    pub download_dir_custom: bool,
    pub local_addresses: Vec<String>,
    pub peers: Vec<LanPeer>,
    pub pending_offers: Vec<PendingOffer>,
    pub last_error: Option<String>,
}

/// peer 表条目：展示字段 + 连接地址 + 过期判断用的本地时钟
#[derive(Debug, Clone)]
pub(crate) struct PeerEntry {
    pub peer: LanPeer,
    pub addr: SocketAddr,
    pub last_seen: Instant,
}

/// 待确认请求 + 把用户决定送回 HTTP 线程的通道
pub(crate) struct PendingEntry {
    pub offer: PendingOffer,
    pub responder: mpsc::Sender<bool>,
}

/// 会话里一个文件的上传状态
#[derive(Debug, Clone)]
pub(crate) struct SessionFile {
    pub id: String,
    pub file_name: String,
    pub size: u64,
    pub token: String,
    pub received: bool,
    /// 正在写的 `.part` 路径（会话过期 / 取消时清理）
    pub part_path: Option<PathBuf>,
}

/// 一次接收会话（一次 prepare-upload 对应一个）
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub dir: PathBuf,
    pub files: Vec<SessionFile>,
    pub created: Instant,
}

impl Session {
    pub fn is_complete(&self) -> bool {
        self.files.iter().all(|file| file.received)
    }
}

/// 运行期可变状态。锁内只做字段读写，不在持锁时等待网络/磁盘。
pub(crate) struct Inner {
    pub settings: LanShareSettings,
    /// 默认接收目录（系统下载目录下）
    pub default_dir: PathBuf,
    /// 当前实际使用的接收目录
    pub download_dir: PathBuf,
    pub download_dir_custom: bool,
    pub running: bool,
    pub port: Option<u16>,
    pub last_error: Option<String>,
    pub local_addresses: Vec<String>,
    pub peers: HashMap<String, PeerEntry>,
    pub pending: Vec<PendingEntry>,
    pub sessions: HashMap<String, Session>,
    /// 测试用：app 为 None 时把进度事件记在这里，测试断言事件内容
    pub recorded_progress: Vec<SendProgress>,
    pub offer_timeout: Duration,
    pub session_ttl: Duration,
}

fn lock_inner(inner: &InnerRef) -> MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Inner {
    pub(crate) fn new(settings: LanShareSettings, default_dir: PathBuf) -> Self {
        let custom = settings
            .download_dir
            .as_deref()
            .map(|dir| !dir.trim().is_empty())
            .unwrap_or(false);
        let download_dir = if custom {
            PathBuf::from(settings.download_dir.clone().unwrap_or_default())
        } else {
            default_dir.clone()
        };
        Self {
            settings,
            default_dir,
            download_dir,
            download_dir_custom: custom,
            running: false,
            port: None,
            last_error: None,
            local_addresses: Vec::new(),
            peers: HashMap::new(),
            pending: Vec::new(),
            sessions: HashMap::new(),
            recorded_progress: Vec::new(),
            offer_timeout: OFFER_TIMEOUT,
            session_ttl: SESSION_TTL,
        }
    }

    /// 当前生效的设备名：设置里的非空值，否则主机名（开发构建带 (dev)）
    pub(crate) fn effective_alias(&self) -> String {
        let alias = self.settings.alias.trim();
        if alias.is_empty() {
            default_alias()
        } else {
            alias.to_string()
        }
    }

    /// 更新 / 插入 peer；返回「列表是否发生变化」。
    pub(crate) fn upsert_peer(
        &mut self,
        announce: &Announce,
        addr: SocketAddr,
        now: Instant,
        now_ms: i64,
    ) -> bool {
        let fingerprint = announce.fingerprint.trim().to_string();
        if fingerprint.is_empty() {
            return false;
        }
        let peer = LanPeer {
            fingerprint: fingerprint.clone(),
            alias: announce.alias.trim().to_string(),
            ip: addr.ip().to_string(),
            port: announce.port,
            device_type: announce.device_type.clone(),
            last_seen_ms: now_ms,
        };
        match self.peers.get_mut(&fingerprint) {
            Some(entry) => {
                let changed = entry.peer.alias != peer.alias
                    || entry.peer.ip != peer.ip
                    || entry.peer.port != peer.port
                    || entry.peer.device_type != peer.device_type;
                entry.peer = peer;
                entry.addr = addr;
                entry.last_seen = now;
                changed
            }
            None => {
                self.peers.insert(
                    fingerprint,
                    PeerEntry {
                        peer,
                        addr,
                        last_seen: now,
                    },
                );
                true
            }
        }
    }

    /// 掉线的 peer 从列表移除；返回是否真的有变化。
    pub(crate) fn prune_peers(&mut self, now: Instant) -> bool {
        let before = self.peers.len();
        self.peers
            .retain(|_, entry| now.duration_since(entry.last_seen) < PEER_TIMEOUT);
        self.peers.len() != before
    }

    pub(crate) fn peer_by_fingerprint(&self, fingerprint: &str) -> Option<LanPeer> {
        self.peers.get(fingerprint).map(|entry| entry.peer.clone())
    }

    /// 待确认请求列表（按创建时间倒序，最新在前）
    pub(crate) fn pending_offers(&self) -> Vec<PendingOffer> {
        let mut offers: Vec<PendingOffer> = self
            .pending
            .iter()
            .map(|entry| entry.offer.clone())
            .collect();
        offers.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
        offers
    }

    /// 会话过期清理：删掉过期会话，顺带删掉没写完的 `.part`
    pub(crate) fn prune_sessions(&mut self) {
        let ttl = self.session_ttl;
        self.sessions.retain(|_, session| {
            if session.created.elapsed() >= ttl {
                for file in &session.files {
                    if let Some(part) = &file.part_path {
                        let _ = std::fs::remove_file(part);
                    }
                }
                false
            } else {
                true
            }
        });
    }

    /// 把等待中的请求删掉（拒绝 / 超时 / 取消），返回被删掉的条目
    pub(crate) fn take_pending(&mut self, offer_id: &str) -> Option<PendingEntry> {
        match self
            .pending
            .iter()
            .position(|entry| entry.offer.id == offer_id)
        {
            Some(index) => Some(self.pending.remove(index)),
            None => None,
        }
    }

    pub(crate) fn remove_session(&mut self, session_id: &str) -> Option<Session> {
        let session = self.sessions.remove(session_id)?;
        for file in &session.files {
            if let Some(part) = &file.part_path {
                let _ = std::fs::remove_file(part);
            }
        }
        Some(session)
    }
}

/// 对外暴露给命令 / 启动接线的运行时状态
pub struct LanShareState {
    pub(crate) inner: InnerRef,
    runtime: Mutex<Option<Runtime>>,
}

/// 一次运行中的发现 + HTTP 服务
struct Runtime {
    // 字段本身不被读取，靠 Drop 在停止时释放端口 / join 线程
    _http: server::HttpServer,
    _discovery: discovery::DiscoveryHandle,
    announce_now: Arc<AtomicBool>,
}

impl LanShareState {
    /// 测试用：给定默认接收目录，其他设置走默认值
    #[cfg(test)]
    pub fn new(default_dir: PathBuf) -> Self {
        let mut settings = LanShareSettings::default();
        // 测试里不需要落盘，指纹固定生成为随机值即可
        settings.ensure_fingerprint();
        Self::with_settings(settings, default_dir)
    }

    /// 命令 / setup 入口：从 store 读设置、解析默认目录
    pub fn from_app(app: &AppHandle) -> Self {
        let mut note: Option<String> = None;
        let mut loaded = match settings::load(app) {
            Ok(settings) => settings,
            Err(err) => {
                note = Some(format!("读取局域网分享设置失败，已使用默认值：{err}"));
                LanShareSettings::default()
            }
        };
        if loaded.ensure_fingerprint() {
            // 老配置没有指纹：补一个并回写，保证重启后设备身份不变
            let _ = settings::save(app, &loaded);
        }
        let default_dir = default_download_dir(app);
        let state = Self::with_settings(loaded, default_dir);
        if let Some(err) = note {
            lock_inner(&state.inner).last_error = Some(err);
        }
        state
    }

    fn with_settings(settings: LanShareSettings, default_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new(settings, default_dir))),
            runtime: Mutex::new(None),
        }
    }

    pub fn snapshot(&self) -> LanShareSnapshot {
        snapshot_of(&lock_inner(&self.inner))
    }

    /// 启停功能。启动失败时把原因写进 `last_error` 并把开关回退到关闭。
    pub fn set_enabled(&self, app: &AppHandle, enabled: bool) -> LanShareSnapshot {
        if enabled {
            match self.start(app) {
                Ok(()) => {
                    self.mutate_settings(app, |settings| settings.enabled = true);
                }
                Err(err) => {
                    self.stop_runtime();
                    {
                        let mut inner = lock_inner(&self.inner);
                        inner.last_error = Some(err);
                    }
                    self.mutate_settings(app, |settings| settings.enabled = false);
                }
            }
        } else {
            // stop_runtime 已清空待接收 / 会话，进行中的 HTTP 线程收到通道断开后返回 403
            self.stop_runtime();
            self.mutate_settings(app, |settings| settings.enabled = false);
            emit_snapshot(&self.inner, Some(app));
        }
        self.snapshot()
    }

    /// 冷启动恢复：设置里 enabled=true 时后台启动，失败只记录原因不阻断应用启动。
    pub fn restore_on_startup(&self, app: &AppHandle) {
        if !lock_inner(&self.inner).settings.enabled {
            return;
        }
        match self.start(app) {
            Ok(()) => {}
            Err(err) => {
                {
                    let mut inner = lock_inner(&self.inner);
                    // 启动失败：开关回到关闭，页面显示原因；不阻断应用启动
                    inner.settings.enabled = false;
                    inner.last_error = Some(err);
                }
                if let Err(save_err) = self.persist_settings(app) {
                    let mut inner = lock_inner(&self.inner);
                    inner.last_error = Some(format!(
                        "{}；保存设置失败：{}",
                        inner.last_error.clone().unwrap_or_default(),
                        save_err
                    ));
                }
                emit_snapshot(&self.inner, Some(app));
            }
        }
    }

    /// 启动发现 + HTTP 服务；重复调用会先停掉旧运行时（幂等）。
    pub fn start(&self, app: &AppHandle) -> Result<(), String> {
        self.stop_runtime();
        {
            let inner = lock_inner(&self.inner);
            // 启动前把目录准备好：失败不阻断启动，接收时还会再建一次
            let _ = std::fs::create_dir_all(&inner.download_dir);
        }

        let http = server::HttpServer::start(
            self.inner.clone(),
            Some(app.clone()),
            PORT_BASE,
            PORT_COUNT,
        )?;
        let port = http.port;
        let discovery =
            match discovery::DiscoveryHandle::start(self.inner.clone(), port, Some(app.clone())) {
                Ok(handle) => handle,
                Err(err) => {
                    // 发现起不来就别留一个只在监听的 HTTP 服务
                    drop(http);
                    return Err(err);
                }
            };
        let local_addresses = local_ipv4_addresses();
        {
            let mut inner = lock_inner(&self.inner);
            inner.running = true;
            inner.port = Some(port);
            inner.local_addresses = local_addresses;
            inner.last_error = None;
        }
        let announce_now = Arc::new(AtomicBool::new(false));
        *self.runtime.lock().unwrap_or_else(|err| err.into_inner()) = Some(Runtime {
            _http: http,
            _discovery: discovery,
            announce_now,
        });
        emit_snapshot(&self.inner, Some(app));
        Ok(())
    }

    fn stop_runtime(&self) {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
        if runtime.is_some() {
            // 运行时结构体 Drop 时会置停止标记并 join 线程
            drop(runtime);
        }
        let mut inner = lock_inner(&self.inner);
        // 关闭时把设备表、待接收、会话全部清掉；没写完的 .part 一并删除
        for session in inner.sessions.values() {
            for file in &session.files {
                if let Some(part) = &file.part_path {
                    let _ = std::fs::remove_file(part);
                }
            }
        }
        inner.sessions.clear();
        inner.pending.clear();
        inner.peers.clear();
        inner.running = false;
        inner.port = None;
        inner.local_addresses.clear();
    }

    /// 修改设备名；空串回退主机名。立即重新 announce。
    pub fn set_alias(&self, app: &AppHandle, alias: &str) -> Result<LanShareSnapshot, String> {
        let alias = normalize_alias(alias);
        {
            let mut inner = lock_inner(&self.inner);
            inner.settings.alias = alias;
        }
        self.persist_settings(app)?;
        self.request_announce();
        emit_snapshot(&self.inner, Some(app));
        Ok(self.snapshot())
    }

    /// 指定接收目录；`None` / 空串回退默认目录。目录不存在时自动创建并做写探测。
    pub fn set_download_dir(
        &self,
        app: &AppHandle,
        dir: Option<String>,
    ) -> Result<LanShareSnapshot, String> {
        let trimmed = dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        let (next_dir, custom) = match &trimmed {
            Some(path) => {
                let path = PathBuf::from(path);
                ensure_writable_dir(&path)?;
                (path, true)
            }
            None => (lock_inner(&self.inner).default_dir.clone(), false),
        };
        {
            let mut inner = lock_inner(&self.inner);
            inner.settings.download_dir = trimmed;
            inner.download_dir = next_dir;
            inner.download_dir_custom = custom;
        }
        self.persist_settings(app)?;
        emit_snapshot(&self.inner, Some(app));
        Ok(self.snapshot())
    }

    /// 接受 / 拒绝待接收请求。
    ///
    /// 拆成 `respond_offer_inner` 是为了让测试不依赖 AppHandle：
    /// 生产命令走 `respond_offer`，测试直接调 inner 版本。
    pub fn respond_offer(
        &self,
        app: &AppHandle,
        offer_id: &str,
        accept: bool,
    ) -> Result<(), String> {
        self.respond_offer_inner(offer_id, accept)?;
        emit_snapshot(&self.inner, Some(app));
        Ok(())
    }

    /// 找到待接收请求并把决定送回去；找不到 / 通道已关闭返回错误。
    pub(crate) fn respond_offer_inner(&self, offer_id: &str, accept: bool) -> Result<(), String> {
        let entry = {
            let mut inner = lock_inner(&self.inner);
            inner
                .take_pending(offer_id)
                .ok_or_else(|| "待接收请求不存在或已处理".to_string())?
        };
        // 通道另一端可能已超时退出，发送失败按「已处理」处理
        entry
            .responder
            .send(accept)
            .map_err(|_| "接收请求已超时".to_string())?;
        Ok(())
    }

    /// 立即返回 transfer_id，实际发送在线程里跑，进度走事件。
    pub fn send_files(
        &self,
        app: &AppHandle,
        peer_fingerprint: &str,
        paths: Vec<String>,
    ) -> Result<String, String> {
        if paths.is_empty() {
            return Err("请先选择要发送的文件".to_string());
        }
        if paths.len() > MAX_FILES {
            return Err(format!("一次最多发送 {} 个文件", MAX_FILES));
        }
        let peer = {
            let inner = lock_inner(&self.inner);
            inner
                .peer_by_fingerprint(peer_fingerprint)
                .ok_or_else(|| "设备不在线或已离线".to_string())?
        };
        let transfer_id = uuid::Uuid::new_v4().to_string();
        let send_peer = client::SendPeer {
            fingerprint: peer.fingerprint.clone(),
            alias: peer.alias.clone(),
            ip: peer.ip.clone(),
            port: peer.port,
        };
        let inner = self.inner.clone();
        let app_handle = app.clone();
        let task_id = transfer_id.clone();
        std::thread::spawn(move || {
            let _ = client::run_send(&inner, Some(&app_handle), &send_peer, &paths, &task_id);
        });
        Ok(transfer_id)
    }

    /// 把设置改动持久化；失败时只返回错误，不改内存里的值（调用方已经改过）。
    fn persist_settings(&self, app: &AppHandle) -> Result<(), String> {
        let settings = lock_inner(&self.inner).settings.clone();
        settings::save(app, &settings)
    }

    fn mutate_settings(&self, app: &AppHandle, mutate: impl FnOnce(&mut LanShareSettings)) {
        {
            let mut inner = lock_inner(&self.inner);
            mutate(&mut inner.settings);
        }
        if let Err(err) = self.persist_settings(app) {
            let mut inner = lock_inner(&self.inner);
            inner.last_error = Some(format!("保存设置失败：{err}"));
        }
    }

    fn request_announce(&self) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .as_ref()
        {
            runtime.announce_now.store(true, Ordering::SeqCst);
        }
    }
}

/// 从 `Inner` 组装快照（纯函数，方便测试；锁由调用方处理）
pub(crate) fn snapshot_of(inner: &Inner) -> LanShareSnapshot {
    let mut peers: Vec<LanPeer> = inner
        .peers
        .values()
        .map(|entry| entry.peer.clone())
        .collect();
    peers.sort_by(|a, b| a.alias.to_lowercase().cmp(&b.alias.to_lowercase()));
    LanShareSnapshot {
        enabled: inner.settings.enabled,
        running: inner.running,
        alias: inner.effective_alias(),
        fingerprint: inner.settings.fingerprint.clone(),
        port: inner.port,
        download_dir: inner.download_dir.to_string_lossy().to_string(),
        download_dir_custom: inner.download_dir_custom,
        local_addresses: inner.local_addresses.clone(),
        peers,
        pending_offers: inner.pending_offers(),
        last_error: inner.last_error.clone(),
    }
}

/// 推送快照事件；app 为 None（测试）时不做任何事。
pub(crate) fn emit_snapshot(inner: &InnerRef, app: Option<&AppHandle>) {
    let Some(app) = app else {
        return;
    };
    let snapshot = snapshot_of(&lock_inner(inner));
    let _ = app.emit(EVENT_SNAPSHOT, snapshot);
}

/// 推送发送进度；测试里 app 为 None，记到 `Inner::recorded_progress` 供断言。
pub(crate) fn emit_progress(inner: &InnerRef, app: Option<&AppHandle>, progress: SendProgress) {
    match app {
        Some(app) => {
            let _ = app.emit(EVENT_SEND_PROGRESS, progress);
        }
        None => {
            let mut guard = lock_inner(inner);
            // 测试场景事件量很小；加个上限防止异常路径无限增长
            if guard.recorded_progress.len() < 4096 {
                guard.recorded_progress.push(progress);
            }
        }
    }
}

/// 设备名清洗：去首尾空白；超过 64 个字符截断（announce 报文别太长）。
pub(crate) fn normalize_alias(alias: &str) -> String {
    let alias = alias.trim();
    if alias.chars().count() > 64 {
        alias.chars().take(64).collect()
    } else {
        alias.to_string()
    }
}

/// 当前 Unix 毫秒时间（面板展示、排序用；不参与过期判断）
pub(crate) fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 默认接收目录：直接使用系统下载目录（macOS 的 ~/Downloads、Windows 的
/// 用户目录\Downloads），取不到时依次回退应用数据目录 / 临时目录。
pub fn default_download_dir(app: &AppHandle) -> PathBuf {
    let download = app.path().download_dir().ok();
    let app_data = app.path().app_data_dir().ok();
    pick_default_dir(download, app_data, std::env::temp_dir())
}

/// 纯函数：默认目录的选择与回退（便于单测，不读真实 HOME）
fn pick_default_dir(
    download: Option<PathBuf>,
    app_data: Option<PathBuf>,
    temp: PathBuf,
) -> PathBuf {
    download.or(app_data).unwrap_or(temp)
}

/// 设备名默认值：主机名；开发构建追加 `(dev)`，同机跑两个版本时能区分。
pub fn default_alias() -> String {
    let host = hostname().unwrap_or_else(|| "ws-workbench".to_string());
    if crate::is_dev_build() {
        format!("{host} (dev)")
    } else {
        host
    }
}

fn hostname() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("COMPUTERNAME").ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut buf = [0u8; 256];
        // SAFETY: gethostname 只写入 buf 的容量范围内；失败时返回值非 0。
        let code = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if code != 0 {
            return None;
        }
        let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
        String::from_utf8(buf[..end].to_vec()).ok()
    }
}

/// 本机可用 IPv4 地址（去重、过滤回环）；给面板展示与组播加入用。
pub fn local_ipv4_addresses() -> Vec<String> {
    let Ok(info) = crate::netinfo::NetworkInfo::current() else {
        return Vec::new();
    };
    let mut addresses: Vec<String> = Vec::new();
    for interface in info.interfaces {
        for address in interface.addresses {
            if !address.family.eq_ignore_ascii_case("ipv4") {
                continue;
            }
            if address.ip.parse::<std::net::Ipv4Addr>().is_ok() && !addresses.contains(&address.ip)
            {
                addresses.push(address.ip);
            }
        }
    }
    addresses
}

/// 创建目录并做一次写探测，确认用户选的目录真能写。
fn ensure_writable_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("无法创建接收目录 {}：{err}", dir.display()))?;
    let probe = dir.join(format!(".ws-lanshare-probe-{}", uuid::Uuid::new_v4()));
    std::fs::write(&probe, b"probe")
        .map_err(|err| format!("接收目录不可写 {}：{err}", dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn announce(alias: &str, fingerprint: &str, port: u16) -> Announce {
        Announce::new(alias, fingerprint, port)
    }

    fn addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 8)), 53317)
    }

    fn state_with_temp_dir() -> (tempfile::TempDir, LanShareState) {
        let dir = tempfile::tempdir().unwrap();
        let state = LanShareState::new(dir.path().join("recv"));
        (dir, state)
    }

    #[test]
    fn snapshot_defaults() {
        let (_dir, state) = state_with_temp_dir();
        let snapshot = state.snapshot();
        assert!(!snapshot.enabled);
        assert!(!snapshot.running);
        assert!(snapshot.port.is_none());
        assert!(!snapshot.alias.is_empty());
        assert!(!snapshot.fingerprint.is_empty());
        assert!(snapshot.peers.is_empty());
        assert!(snapshot.pending_offers.is_empty());
        assert!(snapshot.last_error.is_none());
        // 测试里默认目录传的是临时目录下的 recv，未指定接收目录时应回退到它
        assert!(snapshot.download_dir.ends_with("recv"));
        assert!(!snapshot.download_dir_custom);
    }

    #[test]
    fn effective_alias_falls_back_to_hostname() {
        let (_dir, state) = state_with_temp_dir();
        let alias = state.snapshot().alias;
        assert!(!alias.trim().is_empty());
        {
            let mut inner = lock_inner(&state.inner);
            inner.settings.alias = "  办公室 Mac  ".to_string();
        }
        assert_eq!(state.snapshot().alias, "办公室 Mac");
    }

    #[test]
    fn peer_upsert_and_expiry() {
        let (_dir, state) = state_with_temp_dir();
        let now = Instant::now();
        let mut inner = lock_inner(&state.inner);
        assert!(inner.upsert_peer(&announce("A", "fp-a", 53317), addr(), now, 1000));
        assert!(!inner.upsert_peer(&announce("A", "fp-a", 53317), addr(), now, 1000));
        assert_eq!(inner.peers.len(), 1);
        assert_eq!(inner.peer_by_fingerprint("fp-a").unwrap().alias, "A");

        // 改名算变化
        assert!(inner.upsert_peer(&announce("B", "fp-a", 53317), addr(), now, 2000));
        assert_eq!(inner.peer_by_fingerprint("fp-a").unwrap().alias, "B");

        // 15 秒内还在
        assert!(!inner.prune_peers(now + Duration::from_secs(14)));
        // 超过 15 秒被移除
        assert!(inner.prune_peers(now + PEER_TIMEOUT + Duration::from_secs(1)));
        assert!(inner.peers.is_empty());
    }

    #[test]
    fn pending_offers_sorted_newest_first() {
        let (_dir, state) = state_with_temp_dir();
        let mut inner = lock_inner(&state.inner);
        for (index, created_at) in [100i64, 300, 200].iter().enumerate() {
            let (tx, _rx) = mpsc::channel();
            inner.pending.push(PendingEntry {
                offer: PendingOffer {
                    id: format!("offer-{index}"),
                    sender_alias: "sender".into(),
                    sender_fingerprint: "fp".into(),
                    sender_ip: "192.168.1.9".into(),
                    total_size: 10,
                    files: vec![],
                    created_at_ms: *created_at,
                    expires_in_ms: 60_000,
                },
                responder: tx,
            });
        }
        let ids: Vec<String> = inner
            .pending_offers()
            .into_iter()
            .map(|offer| offer.id)
            .collect();
        assert_eq!(ids, vec!["offer-1", "offer-2", "offer-0"]);
    }

    #[test]
    fn normalize_alias_trims_and_truncates() {
        assert_eq!(normalize_alias("  办公室 Mac  "), "办公室 Mac");
        assert_eq!(normalize_alias(""), "");
        let long = "a".repeat(100);
        assert_eq!(normalize_alias(&long).chars().count(), 64);
        let multibyte = "设".repeat(80);
        assert_eq!(normalize_alias(&multibyte).chars().count(), 64);
    }

    #[test]
    fn download_dir_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = LanShareSettings::default();
        settings.download_dir = Some(dir.path().join("custom").to_string_lossy().to_string());
        let inner = Inner::new(settings, dir.path().join("default"));
        assert!(inner.download_dir_custom);
        assert!(inner.download_dir.ends_with("custom"));

        let mut settings = LanShareSettings::default();
        settings.download_dir = Some("   ".to_string());
        let inner = Inner::new(settings, dir.path().join("default"));
        assert!(!inner.download_dir_custom);
        assert_eq!(inner.download_dir, dir.path().join("default"));
    }

    #[test]
    fn default_dir_is_system_download_dir_without_subfolder() {
        // 直接落在下载目录本身，不再拼「ws-workbench 局域网分享」子目录
        let download = PathBuf::from("/home/someone/Downloads");
        assert_eq!(
            pick_default_dir(
                Some(download.clone()),
                Some(PathBuf::from("/data")),
                PathBuf::from("/tmp")
            ),
            download
        );
        assert!(!pick_default_dir(
            Some(PathBuf::from("/home/someone/Downloads")),
            None,
            PathBuf::from("/tmp")
        )
        .to_string_lossy()
        .contains("ws-workbench"));

        // 下载目录拿不到时回退应用数据目录，再回退临时目录
        assert_eq!(
            pick_default_dir(None, Some(PathBuf::from("/data")), PathBuf::from("/tmp")),
            PathBuf::from("/data")
        );
        assert_eq!(
            pick_default_dir(None, None, PathBuf::from("/tmp")),
            PathBuf::from("/tmp")
        );
    }

    /// 单进程双运行时端到端：A 启动 HTTP 接收服务，B 用 client 发两个文件。
    #[test]
    fn end_to_end_two_runtime_instances() {
        // A：接收端
        let (dir_a, state_a) = state_with_temp_dir();
        let server = server::HttpServer::start(state_a.inner.clone(), None, 0, 1).unwrap();
        {
            let mut guard = lock_inner(&state_a.inner);
            guard.running = true;
            guard.port = Some(server.port);
        }

        // B：发送端（只需要 client，不启动网络服务）
        let (_dir_b, state_b) = state_with_temp_dir();
        let files_dir = tempfile::tempdir().unwrap();
        let first = files_dir.path().join("报告.txt");
        std::fs::write(&first, b"first file body").unwrap();
        let second = files_dir.path().join("second.bin");
        let second_bytes = vec![9u8; 1024];
        std::fs::write(&second, &second_bytes).unwrap();

        let peer = client::SendPeer {
            fingerprint: state_a.snapshot().fingerprint,
            alias: state_a.snapshot().alias,
            ip: "127.0.0.1".to_string(),
            port: server.port,
        };
        let inner_b = state_b.inner.clone();
        let paths = vec![
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ];
        let send_handle = std::thread::spawn(move || {
            client::run_send(&inner_b, None, &peer, &paths, "e2e-transfer")
        });

        // 等 A 的待接收请求出现后接受
        let offer_id = {
            let mut found = None;
            for _ in 0..500 {
                if let Some(entry) = state_a.inner.lock().unwrap().pending.first() {
                    found = Some(entry.offer.id.clone());
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            found.expect("A 没有收到待接收请求")
        };
        state_a.respond_offer_inner(&offer_id, true).unwrap();

        send_handle.join().unwrap().expect("B 发送失败");

        // 断言落盘内容
        let recv_dir = dir_a.path().join("recv");
        assert_eq!(
            std::fs::read(recv_dir.join("报告.txt")).unwrap(),
            b"first file body"
        );
        assert_eq!(
            std::fs::read(recv_dir.join("second.bin")).unwrap(),
            second_bytes
        );
        let snapshot_a = state_a.snapshot();
        assert!(snapshot_a.pending_offers.is_empty());

        // 发送端进度：两个文件都 done 且没有错误
        let progress = state_b.inner.lock().unwrap().recorded_progress.clone();
        let done: Vec<&SendProgress> = progress
            .iter()
            .filter(|item| item.done && item.error.is_none())
            .collect();
        assert_eq!(done.len(), 2, "进度事件：{progress:?}");
        assert!(done.iter().all(|item| item.sent_bytes == item.total_bytes));
    }
}
