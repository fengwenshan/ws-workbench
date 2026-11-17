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

pub(crate) mod chat_store;
pub(crate) mod client;
pub(crate) mod discovery;
pub(crate) mod identity;
pub(crate) mod outbox;
pub(crate) mod protocol;
pub(crate) mod server;
pub(crate) mod settings;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use chat_store::ChatStore;
use protocol::Announce;

pub use chat_store::ChatMessage;

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
/// 聊天相关变化：新消息 / 已读 / 会话删除。前端收到后按需重拉会话与消息。
pub const EVENT_CHAT: &str = "lan-share://chat";
/// 未读总数变化：主窗口 rail 红点 / 托盘快捷面板角标共用这一份订阅。
pub const EVENT_UNREAD: &str = "lan-share://unread";
/// 托盘消息列表点了某条会话：主窗口切到局域网分享并定位该会话。
pub const EVENT_OPEN_CHAT: &str = "lan-share://open-chat";

/// 未读事件载荷：唯一事实源是 SQLite 里的未读列，前端不再自己求和。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UnreadPayload {
    pub total: u32,
}

/// `/chat` 请求体上限与正文长度上限（防误发 / 防大报文）
pub const MAX_CHAT_BODY: usize = 16 * 1024;
pub const MAX_CHAT_CHARS: usize = 4000;
/// 打开会话时最多返回多少条历史消息
pub const CHAT_MESSAGE_LIMIT: usize = 200;
/// 在线判定窗口：announce / 心跳 3 秒一次，10 秒没续上就算离线（前端据此把点判灰）
pub const PEER_ONLINE_WINDOW: Duration = Duration::from_secs(10);

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
    /// 接收文件前是否需要本机确认；默认 false：直接接收，不弹接受框
    #[serde(default)]
    pub require_accept: bool,
    /// 本机给其它设备起的备注：fingerprint -> 备注（只在本机显示，不广播给对端）
    #[serde(default)]
    pub peer_remarks: HashMap<String, String>,
}

impl LanShareSettings {
    pub fn new_with_fingerprint() -> Self {
        Self {
            // 首次运行（没有设置文件）默认开启：装完就能互相发现，不用先去点开关；
            // 用户主动关掉后会持久化为 false，之后冷启动保持关闭。
            enabled: true,
            alias: String::new(),
            download_dir: None,
            fingerprint: uuid::Uuid::new_v4().to_string(),
            require_accept: false,
            peer_remarks: HashMap::new(),
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
    /// 当前选定、也确实连得上的地址（探测成功后会被换成 VPN / 其它网卡的地址）
    pub ip: String,
    pub port: u16,
    pub device_type: String,
    /// 对端自报的全部 IPv4，第一项是 announce 的来源地址；面板上显示的是 `ip`
    #[serde(default)]
    pub addresses: Vec<String>,
    /// 最近一次收到 announce 的 Unix 毫秒时间，面板用来显示「最近在线」
    pub last_seen_ms: i64,
}

/// 聊天列表里的一行：持久化会话 + 运行期在线状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanShareConversation {
    /// 对端设备身份（`fingerprint`，即主网卡 MAC）
    pub id: String,
    pub alias: String,
    pub online: bool,
    pub last_activity_ms: i64,
    pub last_preview: String,
    pub unread: i64,
    pub created_at_ms: i64,
}

/// 聊天变化事件：前端收到后重拉会话列表，正在看的会话再拉消息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatEvent {
    pub peer_id: String,
    /// `message` / `read` / `deleted` / `updated`（附件被删除等状态变化）
    pub kind: String,
    /// 消息方向：`in` / `out`；非消息变化（已读 / 删除 / 状态更新）为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

/// 托盘消息列表点选会话事件载荷
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenChatEvent {
    pub peer_id: String,
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
    /// 致命错误：只有「服务没起来」才写这里，界面据此显示「启动失败」
    pub last_error: Option<String>,
    /// 非致命问题：服务正常运行，只是设置没能写进磁盘之类。
    /// 和 `last_error` 分开，避免再出现「运行中 + 启动失败」同时成立的矛盾状态
    /// （现场：保存设置瞬时被拒，功能明明已经起来了，界面却红字报启动失败）。
    #[serde(default)]
    pub warning: Option<String>,
    /// 接收文件前是否需要本机确认（默认 false：直接接收）
    #[serde(default)]
    pub require_accept: bool,
    /// 本机给设备起的备注（fingerprint -> 备注），只影响本机显示
    #[serde(default)]
    pub peer_remarks: HashMap<String, String>,
}

/// peer 表条目：展示字段 + 连接地址 + 过期判断用的本地时钟
#[derive(Debug, Clone)]
pub(crate) struct PeerEntry {
    pub peer: LanPeer,
    pub addr: SocketAddr,
    pub last_seen: Instant,
}

/// `upsert_peer` 的结果：
/// - `changed`：设备名 / 地址 / 端口等展示信息变了，需要探测地址并立即回包；
/// - `seen`：只是心跳把 `last_seen` 续上了。它也要推快照，否则面板里的
///   `last_seen_ms` 永远停在首次发现那一刻，10 秒后在线点就会变灰
///   （现场问题：两台机器都在线，附近设备却一直是灰点）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PeerUpdate {
    pub changed: bool,
    pub seen: bool,
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
    /// 发送方身份：接收完成后要按这个给聊天记录补一条「收到的文件」消息
    pub peer_id: String,
    pub peer_alias: String,
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
    /// 非致命问题（目前只有「设置没能写进磁盘」）：服务照常运行，只是偏好没存住
    pub warning: Option<String>,
    pub local_addresses: Vec<String>,
    pub peers: HashMap<String, PeerEntry>,
    pub pending: Vec<PendingEntry>,
    pub sessions: HashMap<String, Session>,
    /// 聊天记录存储；钥匙串 / 数据库打不开时是 None，功能其余部分照常运行
    pub chat: Option<ChatStore>,
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
            warning: None,
            local_addresses: Vec::new(),
            peers: HashMap::new(),
            pending: Vec::new(),
            sessions: HashMap::new(),
            chat: None,
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

    /// 更新 / 插入 peer，告诉调用方「展示信息是否变化」以及「心跳是否续上」。
    ///
    /// 两个标记分开的原因：`changed` 会触发地址探测和立即回包（重操作，必须限速）；
    /// `seen` 只是时间推进，但也要推快照，否则面板 `last_seen_ms` 不刷新、在线点变灰。
    pub(crate) fn upsert_peer(
        &mut self,
        announce: &Announce,
        addr: SocketAddr,
        now: Instant,
        now_ms: i64,
    ) -> PeerUpdate {
        let fingerprint = announce.fingerprint.trim().to_string();
        if fingerprint.is_empty() {
            return PeerUpdate::default();
        }
        let addresses = peer_addresses(addr, &announce.addresses);
        // 已经是探测过的地址（比如 VPN 地址）就不要被下一条 announce 盖回 LAN 地址，
        // 否则每 3 秒都要重新探测一次，面板上的地址也会闪。
        let kept_ip = self
            .peers
            .get(&fingerprint)
            .map(|entry| entry.peer.ip.clone())
            .filter(|ip| addresses.iter().any(|item| item == ip));
        let ip = kept_ip
            .or_else(|| addresses.first().cloned())
            .unwrap_or_default();
        let port = announce.port;
        let peer_addr = ip
            .parse::<IpAddr>()
            .map(|parsed| SocketAddr::new(parsed, port))
            .unwrap_or(addr);
        let peer = LanPeer {
            fingerprint: fingerprint.clone(),
            alias: announce.alias.trim().to_string(),
            ip,
            port,
            device_type: announce.device_type.clone(),
            addresses: addresses.clone(),
            last_seen_ms: now_ms,
        };
        match self.peers.get_mut(&fingerprint) {
            Some(entry) => {
                let changed = entry.peer.alias != peer.alias
                    || entry.peer.ip != peer.ip
                    || entry.peer.port != peer.port
                    || entry.peer.device_type != peer.device_type
                    || entry.peer.addresses != peer.addresses;
                entry.peer = peer;
                entry.addr = peer_addr;
                entry.last_seen = now;
                PeerUpdate {
                    changed,
                    seen: true,
                }
            }
            None => {
                self.peers.insert(
                    fingerprint,
                    PeerEntry {
                        peer,
                        addr: peer_addr,
                        last_seen: now,
                    },
                );
                PeerUpdate {
                    changed: true,
                    seen: true,
                }
            }
        }
    }

    /// 探测结果写回：把 `ip` 作为这台设备的可用地址。返回是否真的改了。
    pub(crate) fn set_peer_ip(&mut self, fingerprint: &str, ip: &str) -> bool {
        let Ok(parsed) = ip.parse::<IpAddr>() else {
            return false;
        };
        let Some(entry) = self.peers.get_mut(fingerprint) else {
            return false;
        };
        if entry.peer.ip == ip {
            return false;
        }
        let port = entry.peer.port;
        entry.peer.ip = ip.to_string();
        entry.addr = SocketAddr::new(parsed, port);
        true
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

    /// 设备现在是否在线（在线点用）：10 秒内收到过 announce / 心跳。
    pub(crate) fn is_peer_online(&self, fingerprint: &str, now: Instant) -> bool {
        self.peers
            .get(fingerprint)
            .map(|entry| now.duration_since(entry.last_seen) < PEER_ONLINE_WINDOW)
            .unwrap_or(false)
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

        // 身份优先用主物理网卡 MAC：换 IP / 重启都不会变。
        // 取不到 MAC（没有物理网卡、受限权限）时沿用持久化指纹，身份不会变成空串。
        if let Some(mac) = identity::primary_mac() {
            if loaded.fingerprint != mac {
                loaded.fingerprint = mac;
                let _ = settings::save(app, &loaded);
            }
        }

        // 端口常开：不再提供关闭入口；老配置里的 enabled=false / require_accept=true
        // 一并纠正并回写（接收文件前确认功能已移除，始终直接接收）。
        let mut save_warning: Option<String> = None;
        if enforce_always_on(&mut loaded) {
            if let Err(err) = settings::save(app, &loaded) {
                // 纠正没能落盘不影响本次运行（内存里已是常开），但要提示用户
                save_warning = Some(format!("常开设置没能写入磁盘：{err}"));
            }
        }

        let default_dir = default_download_dir(app);
        let state = Self::with_settings(loaded, default_dir);
        if let Some(message) = save_warning {
            lock_inner(&state.inner).warning = Some(message);
        }

        // 聊天记录存储打不开不影响文件收发：只提示，命令层返回明确错误
        match ChatStore::open(app) {
            Ok(store) => lock_inner(&state.inner).chat = Some(store),
            Err(err) => {
                let mut inner = lock_inner(&state.inner);
                let message = format!("聊天记录不可用：{err}");
                inner.warning = Some(match inner.warning.take() {
                    Some(prev) => format!("{prev}；{message}"),
                    None => message,
                });
            }
        }

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

    /// 聊天存储句柄；不可用时给出能直接展示给用户的错误。
    fn chat_store(&self) -> Result<ChatStore, String> {
        lock_inner(&self.inner)
            .chat
            .clone()
            .ok_or_else(|| "聊天记录不可用（本地数据库或密钥无法访问）".to_string())
    }

    /// 聊天列表：持久化会话 + 运行期在线状态 / 最新设备名。
    ///
    /// 在线判定用 `last_seen_ms`（本地时钟生成）而不是 `Instant`：
    /// 返回值要给前端排序和画在线点，时间戳更直观。
    pub fn conversations(&self) -> Result<Vec<LanShareConversation>, String> {
        let store = self.chat_store()?;
        let rows = store.list_conversations()?;
        let (aliases, online_flags) = {
            let inner = lock_inner(&self.inner);
            let now = Instant::now();
            let mut aliases: HashMap<String, String> = HashMap::new();
            let mut online_flags: HashMap<String, bool> = HashMap::new();
            for (id, entry) in &inner.peers {
                aliases.insert(id.clone(), entry.peer.alias.trim().to_string());
                online_flags.insert(id.clone(), inner.is_peer_online(id, now));
            }
            (aliases, online_flags)
        };
        Ok(rows
            .into_iter()
            .map(|row| {
                let alias = aliases
                    .get(&row.id)
                    .filter(|alias| !alias.is_empty())
                    .cloned()
                    .unwrap_or(row.alias);
                let online = online_flags.get(&row.id).copied().unwrap_or(false);
                LanShareConversation {
                    id: row.id,
                    alias,
                    online,
                    last_activity_ms: row.last_activity_ms,
                    last_preview: row.last_preview,
                    unread: row.unread,
                    created_at_ms: row.created_at_ms,
                }
            })
            .collect())
    }

    /// 某个会话的历史消息（时间正序，最多 `CHAT_MESSAGE_LIMIT` 条）
    pub fn messages(&self, peer_id: &str) -> Result<Vec<ChatMessage>, String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        self.chat_store()?.list_messages(peer_id, CHAT_MESSAGE_LIMIT)
    }

    /// 清空某个会话的未读
    pub fn mark_read(&self, app: &AppHandle, peer_id: &str) -> Result<(), String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        let changed = self.chat_store()?.mark_read(peer_id)?;
        // 只有真的清掉了未读才推事件：否则前端「收到消息→标已读→收到 read 事件→
        // 再标已读」会来回触发，形成事件循环
        if changed {
            emit_chat(&self.inner, Some(app), peer_id, "read");
        }
        Ok(())
    }

    /// 全部会话未读总数（托盘左键判定 + Dock 角标）
    pub fn total_unread(&self) -> u32 {
        let store = lock_inner(&self.inner).chat.clone();
        let Some(store) = store else {
            return 0;
        };
        store.total_unread().unwrap_or(0)
    }

    /// 启动时按历史未读刷一次 Dock 角标（macOS）
    pub fn sync_dock_badge(&self, app: &AppHandle) {
        refresh_dock_badge(&self.inner, app);
    }

    /// 删除本机会话与消息；对方不受影响，之后再来消息会重新建会话。
    ///
    /// `delete_files` 为 true 时，同时把本机收到的文件移入废纸篓（失败回退直接删除）。
    pub fn delete_conversation(
        &self,
        app: &AppHandle,
        peer_id: &str,
        delete_files: bool,
    ) -> Result<(), String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        let store = self.chat_store()?;
        delete_conversation_rows(&store, peer_id, delete_files)?;
        emit_chat(&self.inner, Some(app), peer_id, "deleted");
        Ok(())
    }

    /// 删除单条消息（仅本机聊天记录；对方不受影响，本机已收到的文件保留）。
    ///
    /// 删掉会话最后一条时后端会连带删掉空会话（等价「清除聊天记录」），
    /// 推 `updated` 事件让前端刷新消息和会话列表（不响提示音、不标已读）。
    pub fn delete_message(
        &self,
        app: &AppHandle,
        peer_id: &str,
        message_id: &str,
    ) -> Result<(), String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        let message_id = message_id.trim();
        if message_id.is_empty() {
            return Err("缺少消息标识".to_string());
        }
        // 锁里只取 store 克隆，DB IO 都在锁外做
        let store = self.chat_store()?;
        store.delete_message(peer_id, message_id)?;
        emit_chat(&self.inner, Some(app), peer_id, "updated");
        Ok(())
    }

    /// 该会话里本机收到的、带落盘路径的文件数
    pub fn attachment_count(&self, peer_id: &str) -> Result<u32, String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        self.chat_store()?.attachment_count(peer_id)
    }

    /// 删除本机收到的文件但保留聊天记录；返回实际删除的文件数。
    pub fn delete_attachments(&self, app: &AppHandle, peer_id: &str) -> Result<u32, String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        let store = self.chat_store()?;
        let deleted = delete_attachment_rows(&store, peer_id)?;
        // 附件状态变了但会话还在：用 updated，前端只刷新、不标已读、不响提示音
        emit_chat(&self.inner, Some(app), peer_id, "updated");
        Ok(deleted)
    }

    /// 发送一条文本消息：本地先落库（sending）并返回，投递在后台线程里做。
    pub fn send_chat(
        &self,
        app: &AppHandle,
        peer_id: &str,
        text: &str,
    ) -> Result<ChatMessage, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("消息不能为空".to_string());
        }
        if text.chars().count() > MAX_CHAT_CHARS {
            return Err(format!("消息太长（最多 {MAX_CHAT_CHARS} 个字符）"));
        }
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        let store = self.chat_store()?;
        let peer = {
            let inner = lock_inner(&self.inner);
            inner
                .peer_by_fingerprint(peer_id)
                .ok_or_else(|| "设备不在线或已离线".to_string())?
        };

        let message_id = uuid::Uuid::new_v4().to_string();
        let mut message = ChatMessage::text(&message_id, peer_id, "out", text, unix_millis());
        message.status = "sending".to_string();
        store.append_message(peer_id, &peer.alias, &message)?;
        emit_chat_with_direction(&self.inner, Some(app), peer_id, "out");

        let send_peer = client::SendPeer {
            fingerprint: peer.fingerprint.clone(),
            alias: peer.alias.clone(),
            ip: peer.ip.clone(),
            port: peer.port,
            addresses: peer.addresses.clone(),
        };
        let inner = self.inner.clone();
        let app_handle = app.clone();
        let text = text.to_string();
        std::thread::spawn(move || {
            // 失败 / 成功都由 run_send_chat 写回消息状态并推事件
            if let Err(err) = client::run_send_chat(&inner, Some(&app_handle), &send_peer, &message_id, &text) {
                eprintln!("局域网消息投递失败：{err}");
            }
        });
        Ok(message)
    }

    /// 冷启动恢复：端口常开，应用启动即监听；失败只记录原因并等下次冷启动重试，
    /// 不阻断应用启动（也没有「关闭功能」入口）。
    pub fn restore_on_startup(&self, app: &AppHandle) {
        match self.start(app) {
            Ok(()) => {}
            Err(err) => {
                {
                    let mut inner = lock_inner(&self.inner);
                    // enabled 保持 true：下次冷启动仍会尝试监听
                    inner.last_error = Some(err);
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
        // 停下来之后不该再挂着上一次的错误/提示：否则界面上「未开启」还红字写着
        // 「启动失败」，用户会以为关不掉。真正启动失败时调用方紧接着会重新写入。
        inner.last_error = None;
        inner.warning = None;
    }

    /// 修改设备名；空串回退主机名。立即重新 announce。
    pub fn set_alias(&self, app: &AppHandle, alias: &str) -> Result<LanShareSnapshot, String> {
        let alias = normalize_alias(alias);
        {
            let mut inner = lock_inner(&self.inner);
            inner.settings.alias = alias;
        }
        // 存不下来只提示，不当成失败：名字已经生效了
        self.persist_with_warning(app);
        self.request_announce();
        emit_snapshot(&self.inner, Some(app));
        Ok(self.snapshot())
    }

    /// 设置 / 清除本机给某台设备起的备注（只在本机显示，不广播给对端）。
    ///
    /// `remark` 会 trim 并截断到 32 字符；空串表示删除该备注。
    pub fn set_peer_remark(
        &self,
        app: &AppHandle,
        peer_id: &str,
        remark: &str,
    ) -> Result<LanShareSnapshot, String> {
        let peer_id = peer_id.trim();
        if peer_id.is_empty() {
            return Err("缺少设备标识".to_string());
        }
        let remark = normalize_remark(remark);
        let mut next = lock_inner(&self.inner).settings.clone();
        apply_peer_remark(&mut next, peer_id, &remark);
        settings::save(app, &next)?;
        {
            let mut inner = lock_inner(&self.inner);
            apply_peer_remark(&mut inner.settings, peer_id, &remark);
        }
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
        // 同上：写盘失败不阻断，接收目录已经切过去了
        self.persist_with_warning(app);
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
    ///
    /// 每个文件在会话里先生成一条 `sending` 文件消息，发送线程按文件结果更新状态；
    /// 聊天记录写失败只打日志，不影响真正的文件传输。
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
        // 先校验本地文件（失败就不建会话消息）
        let files = client::collect_files(&paths)?;
        let peer = {
            let inner = lock_inner(&self.inner);
            inner
                .peer_by_fingerprint(peer_fingerprint)
                .ok_or_else(|| "设备不在线或已离线".to_string())?
        };
        let transfer_id = uuid::Uuid::new_v4().to_string();
        let now = unix_millis();
        let message_ids: Vec<String> = files
            .iter()
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        // 注意：先把 store 从锁里取出来，再在锁外写库。edition 2021 的
        // `if let Some(store) = inner.lock().chat.clone()` 会把 MutexGuard 的临时量
        // 延长到整个 if-let 结束，等于持锁做 DB IO，会卡住心跳/快照。
        let chat_store = lock_inner(&self.inner).chat.clone();
        if let Some(store) = chat_store {
            for (index, file) in files.iter().enumerate() {
                let mut message = ChatMessage::file(
                    &message_ids[index],
                    peer_fingerprint,
                    "out",
                    &file.name,
                    file.size,
                    "sending",
                    now,
                );
                message.transfer_id = Some(transfer_id.clone());
                message.file_index = Some(index);
                message.file_total = Some(files.len());
                // 回填本机源路径：失败气泡「重试」时不用再让用户重新选文件
                message.file_path = Some(file.path.to_string_lossy().to_string());
                if let Err(err) = store.append_message(peer_fingerprint, &peer.alias, &message) {
                    eprintln!("写入发送文件消息失败：{err}");
                }
            }
            emit_chat_with_direction(&self.inner, Some(app), peer_fingerprint, "out");
        }
        let send_peer = client::SendPeer {
            fingerprint: peer.fingerprint.clone(),
            alias: peer.alias.clone(),
            ip: peer.ip.clone(),
            port: peer.port,
            addresses: peer.addresses.clone(),
        };
        let inner = self.inner.clone();
        let app_handle = app.clone();
        let task_id = transfer_id.clone();
        std::thread::spawn(move || {
            let _ = client::run_send(
                &inner,
                Some(&app_handle),
                &send_peer,
                &paths,
                &task_id,
                &message_ids,
            );
        });
        Ok(transfer_id)
    }

    /// 拖拽文件：先把字节落到暂存目录，再走常规文件发送（返回 transfer_id）。
    pub fn send_dropped_files(
        &self,
        app: &AppHandle,
        peer_fingerprint: &str,
        files: &[outbox::DroppedFileInput],
    ) -> Result<String, String> {
        let paths = outbox::save_dropped_files(app, files)?;
        self.send_files(app, peer_fingerprint, paths)
    }

    /// 落盘并处理失败：重试几次，仍失败只记 `warning`。
    ///
    /// 重试是因为实测这类「拒绝访问」多数是瞬时的（杀毒/索引服务在同一个文件上
    /// 抖一下），隔几十毫秒再来通常就过了；真存不下来也不该把功能说成失败的，
    /// 但必须告诉用户「重启后可能回到上次的状态」——否则关掉功能后重启又自己
    /// 开起来，用户只会觉得开关坏了。
    fn persist_with_warning(&self, app: &AppHandle) {
        let settings = lock_inner(&self.inner).settings.clone();
        let mut last = String::new();
        for attempt in 0..3 {
            match settings::save(app, &settings) {
                Ok(()) => {
                    lock_inner(&self.inner).warning = None;
                    return;
                }
                Err(err) => {
                    last = err;
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_millis(80));
                    }
                }
            }
        }
        let mut inner = lock_inner(&self.inner);
        inner.warning = Some(format!(
            "{last}。本次改动已在运行时生效，但重启后可能回到上次保存的状态"
        ));
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
        warning: inner.warning.clone(),
        require_accept: inner.settings.require_accept,
        peer_remarks: inner.settings.peer_remarks.clone(),
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

/// 推送聊天变化事件；测试里 app 为 None 时不做任何事。
/// 删除本机文件：优先移入废纸篓，失败回退直接删除；只处理普通文件，跳过目录/符号链接。
///
/// 返回真正删掉的文件数。单个文件失败只记日志，不阻断整体流程。
fn delete_local_files(paths: &[String]) -> usize {
    let mut deleted = 0;
    for raw in paths {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let path = Path::new(raw);
        // 只接受绝对路径：路径来自本应用写入的 DB，这里再防一层被篡改的值
        if !path.is_absolute() {
            continue;
        }
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let file_type = meta.file_type();
        if file_type.is_dir() || file_type.is_symlink() || !meta.is_file() {
            continue;
        }
        let ok = match trash::delete(path) {
            Ok(()) => true,
            Err(err) => {
                eprintln!(
                    "[lanshare] 附件移入废纸篓失败，改为直接删除: {} ({})",
                    path.display(),
                    err
                );
                std::fs::remove_file(path).is_ok()
            }
        };
        if ok {
            deleted += 1;
        } else {
            eprintln!("[lanshare] 删除附件失败: {}", path.display());
        }
    }
    deleted
}

/// 删会话（可选删本机收到的文件）的纯存储流程，便于单测。
fn delete_conversation_rows(
    store: &ChatStore,
    peer_id: &str,
    delete_files: bool,
) -> Result<usize, String> {
    let paths = if delete_files {
        store.received_file_paths(peer_id)?
    } else {
        Vec::new()
    };
    store.delete_conversation(peer_id)?;
    Ok(if delete_files {
        delete_local_files(&paths)
    } else {
        0
    })
}

/// 删本机收到的文件但保留聊天记录的纯存储流程，便于单测。
fn delete_attachment_rows(store: &ChatStore, peer_id: &str) -> Result<u32, String> {
    let paths = store.clear_received_file_paths(peer_id)?;
    Ok(delete_local_files(&paths) as u32)
}

pub(crate) fn emit_chat(inner: &InnerRef, app: Option<&AppHandle>, peer_id: &str, kind: &str) {
    emit_chat_with_direction_opt(inner, app, peer_id, kind, None);
}

/// 新消息事件：带 direction，前端据此只对「收到」播放提示音。
pub(crate) fn emit_chat_with_direction(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    peer_id: &str,
    direction: &str,
) {
    emit_chat_with_direction_opt(inner, app, peer_id, "message", Some(direction));
}

fn emit_chat_with_direction_opt(
    inner: &InnerRef,
    app: Option<&AppHandle>,
    peer_id: &str,
    kind: &str,
    direction: Option<&str>,
) {
    let Some(app) = app else {
        return;
    };
    let _ = app.emit(
        EVENT_CHAT,
        ChatEvent {
            peer_id: peer_id.to_string(),
            kind: kind.to_string(),
            direction: direction.map(str::to_string),
        },
    );
    // 任何会话变化（新消息 / 已读 / 删除）后刷新 Dock 角标、托盘红点与前端未读订阅
    refresh_dock_badge(inner, app);
    emit_unread(inner, app);
}

/// 未读总数唯一出口：DB 是事实源，前端与托盘都订阅这一次推送。
///
/// store 先从锁里 clone 再查询，禁止持锁做 SQLite IO。
fn emit_unread(inner: &InnerRef, app: &AppHandle) {
    let store = lock_inner(inner).chat.clone();
    let Some(store) = store else {
        return;
    };
    let total = store.total_unread().unwrap_or(0);
    let _ = app.emit(EVENT_UNREAD, UnreadPayload { total });
    // 复用刚查到的 total，托盘不再重复查一次 SQLite
    crate::refresh_tray_badge_with_total(app, total);
}

/// 全部会话未读总数 → macOS Dock 角标；0 时清掉，失败忽略。
///
/// 注意：先把 store 从 LAN 锁里 clone 出来再查询，禁止持锁做 SQLite IO。
#[cfg(target_os = "macos")]
fn refresh_dock_badge(inner: &InnerRef, app: &AppHandle) {
    let store = lock_inner(inner).chat.clone();
    let Some(store) = store else {
        return;
    };
    let total = store.total_unread().unwrap_or(0);
    let count = if total > 0 { Some(total as i64) } else { None };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_badge_count(count);
    }
}

#[cfg(not(target_os = "macos"))]
fn refresh_dock_badge(_inner: &InnerRef, _app: &AppHandle) {}

/// 端口常开策略：强制 enabled=true、require_accept=false。
///
/// 返回是否真的改过（调用方据此决定是否回写设置文件）。老版本可能存过
/// enabled=false / require_accept=true；新版本没有关闭入口，启动时必须纠正。
pub(crate) fn enforce_always_on(settings: &mut LanShareSettings) -> bool {
    let mut corrected = false;
    if !settings.enabled {
        settings.enabled = true;
        corrected = true;
    }
    if settings.require_accept {
        settings.require_accept = false;
        corrected = true;
    }
    corrected
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

/// 本机备注：trim + 最多 32 个字符；空串表示删除
pub(crate) fn normalize_remark(remark: &str) -> String {
    let remark = remark.trim();
    if remark.chars().count() > 32 {
        remark.chars().take(32).collect()
    } else {
        remark.to_string()
    }
}

/// 把（已 normalize 的）备注写到设置里；空串删除。纯函数，方便单测。
pub(crate) fn apply_peer_remark(settings: &mut LanShareSettings, peer_id: &str, remark: &str) {
    if remark.is_empty() {
        settings.peer_remarks.remove(peer_id);
    } else {
        settings
            .peer_remarks
            .insert(peer_id.to_string(), remark.to_string());
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

/// 对端地址候选：announce 的来源地址排第一，其余用对方自报的 IPv4（去重、只留合法 v4）。
///
/// 开着 VPN 时来源地址常常是「局域网地址」，而真正通的是 VPN 地址
/// （或者反过来），所以两边都留着，由发送端挨个探测。
fn peer_addresses(source: SocketAddr, advertised: &[String]) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    if let IpAddr::V4(ip) = source.ip() {
        list.push(ip.to_string());
    }
    for raw in advertised {
        let value = raw.trim();
        if value.parse::<Ipv4Addr>().is_err() || list.iter().any(|item| item == value) {
            continue;
        }
        list.push(value.to_string());
    }
    list
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

    /// 测试用固定密钥（不碰钥匙串 / DPAPI）
    fn test_keys() -> crate::crypto::VaultKeys {
        crate::crypto::VaultKeys {
            enc_key: [0x11; 32],
            mac_key: [0x22; 32],
        }
    }

    /// 给运行时挂一个临时聊天库（同时触发 schema 迁移），返回 store 供断言
    fn attach_chat(state: &LanShareState, path: &Path) -> ChatStore {
        crate::db::open(path).expect("创建聊天测试库失败");
        let store = ChatStore::new(path.to_path_buf(), test_keys());
        lock_inner(&state.inner).chat = Some(store.clone());
        store
    }

    const DELETE_PEER: &str = "A4:83:E7:12:34:56";

    /// 建一个带收到文件消息的临时库；返回 (目录, store, 收到文件路径)
    fn store_with_received_file() -> (tempfile::TempDir, ChatStore, String) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("chat.db");
        crate::db::open(&db).unwrap();
        let store = ChatStore::new(db, test_keys());
        store
            .append_message(
                DELETE_PEER,
                "对端",
                &ChatMessage::file("in1", DELETE_PEER, "in", "收到.pdf", 10, "received", 1_000),
            )
            .unwrap();
        let received = dir.path().join("收到.pdf");
        std::fs::write(&received, b"recv").unwrap();
        store
            .update_message_status(
                DELETE_PEER,
                "in1",
                "received",
                None,
                Some(received.to_string_lossy().as_ref()),
            )
            .unwrap();
        (dir, store, received.to_string_lossy().to_string())
    }

    #[test]
    fn delete_local_files_skips_directories_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let missing = dir.path().join("missing.txt");
        // 相对路径守卫：在 CWD 落一个真实文件，用相对文件名传入，断言未被删除
        let relative_name = format!("lanshare-relative-{}.txt", std::process::id());
        std::fs::write(&relative_name, b"x").unwrap();
        let deleted = delete_local_files(&[
            file.to_string_lossy().to_string(),
            sub.to_string_lossy().to_string(),
            missing.to_string_lossy().to_string(),
            relative_name.clone(),
        ]);
        assert_eq!(deleted, 1);
        assert!(!file.exists());
        assert!(sub.exists());
        assert!(
            Path::new(&relative_name).exists(),
            "相对路径应被跳过，不能被删除"
        );
        let _ = std::fs::remove_file(&relative_name);
    }

    #[test]
    fn delete_conversation_rows_with_files_removes_received_file() {
        let (_dir, store, received) = store_with_received_file();
        let sent = Path::new(&received).with_file_name("发出.pdf");
        std::fs::write(&sent, b"sent").unwrap();
        store
            .append_message(
                DELETE_PEER,
                "对端",
                &ChatMessage::file("out1", DELETE_PEER, "out", "发出.pdf", 10, "sent", 2_000),
            )
            .unwrap();
        store
            .update_message_status(DELETE_PEER, "out1", "sent", None, Some(sent.to_string_lossy().as_ref()))
            .unwrap();

        delete_conversation_rows(&store, DELETE_PEER, true).unwrap();
        assert!(store.get_conversation(DELETE_PEER).unwrap().is_none());
        assert!(!Path::new(&received).exists(), "收到的文件应被删除");
        assert!(sent.exists(), "自己发出的源文件不应被删除");
    }

    #[test]
    fn delete_conversation_rows_without_files_keeps_files() {
        let (_dir, store, received) = store_with_received_file();
        delete_conversation_rows(&store, DELETE_PEER, false).unwrap();
        assert!(store.get_conversation(DELETE_PEER).unwrap().is_none());
        assert!(Path::new(&received).exists(), "未勾选时不应删除文件");
    }

    #[test]
    fn delete_attachment_rows_marks_deleted_and_removes_file() {
        let (_dir, store, received) = store_with_received_file();
        let deleted = delete_attachment_rows(&store, DELETE_PEER).unwrap();
        assert_eq!(deleted, 1);
        assert!(!Path::new(&received).exists());
        let messages = store.list_messages(DELETE_PEER, 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].status, "deleted");
        assert_eq!(messages[0].file_path, None);
        assert_eq!(store.attachment_count(DELETE_PEER).unwrap(), 0);
    }

    #[test]
    fn chat_event_direction_is_optional() {
        let with = ChatEvent {
            peer_id: "peer".into(),
            kind: "message".into(),
            direction: Some("in".into()),
        };
        let json = serde_json::to_value(&with).unwrap();
        assert_eq!(json["direction"], "in");
        let without = ChatEvent {
            peer_id: "peer".into(),
            kind: "read".into(),
            direction: None,
        };
        let json = serde_json::to_value(&without).unwrap();
        assert!(json.get("direction").is_none());
        // 老版本事件（没有 direction）也能反序列化
        let parsed: ChatEvent =
            serde_json::from_value(serde_json::json!({"peer_id":"p","kind":"message"})).unwrap();
        assert!(parsed.direction.is_none());
    }

    #[test]
    fn open_chat_event_serializes_peer_id() {
        let json = serde_json::to_value(OpenChatEvent {
            peer_id: "peer".into(),
        })
        .unwrap();
        assert_eq!(json["peer_id"], "peer");
        let parsed: OpenChatEvent = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.peer_id, "peer");
    }

    #[test]
    fn snapshot_defaults() {
        let (_dir, state) = state_with_temp_dir();
        let snapshot = state.snapshot();
        // 首次运行（默认设置）功能默认开启
        assert!(snapshot.enabled);
        assert!(!snapshot.running);
        assert!(snapshot.port.is_none());
        assert!(!snapshot.alias.is_empty());
        assert!(!snapshot.fingerprint.is_empty());
        assert!(snapshot.peers.is_empty());
        assert!(snapshot.pending_offers.is_empty());
        assert!(snapshot.last_error.is_none());
        assert!(snapshot.warning.is_none());
        // 测试里默认目录传的是临时目录下的 recv，未指定接收目录时应回退到它
        assert!(snapshot.download_dir.ends_with("recv"));
        assert!(!snapshot.download_dir_custom);
        // 默认直接接收，不要求对方确认；备注表为空
        assert!(!snapshot.require_accept);
        assert!(snapshot.peer_remarks.is_empty());
    }

    /// 现场问题回归：保存设置瞬时被拒时，界面曾把「保存失败」当成「启动失败」，
    /// 于是「运行中」和「启动失败」同时成立。现在这类问题只走 warning，
    /// 不会污染 last_error。
    #[test]
    fn save_failure_is_a_warning_not_a_start_failure() {
        let (_dir, state) = state_with_temp_dir();
        {
            let mut inner = lock_inner(&state.inner);
            inner.running = true;
            inner.port = Some(53317);
            inner.warning = Some("保存设置失败：拒绝访问。(os error 5)".to_string());
        }
        let snapshot = state.snapshot();
        assert!(snapshot.running, "服务还在跑");
        assert_eq!(snapshot.port, Some(53317));
        assert!(snapshot.last_error.is_none(), "保存失败不能算启动失败");
        assert!(snapshot.warning.is_some(), "要让用户看到「设置没存住」");
    }

    #[test]
    fn unread_payload_serializes_contract() {
        // 前端 UnreadPayload 按 { total: number } 解包；改字段名会悄悄让红点失效
        let json = serde_json::to_string(&UnreadPayload { total: 3 }).unwrap();
        assert_eq!(json, r#"{"total":3}"#);
    }

    #[test]
    fn always_on_settings_are_enforced() {
        let mut settings = LanShareSettings::default();
        assert!(settings.enabled);
        assert!(!settings.require_accept);
        assert!(!enforce_always_on(&mut settings), "默认设置无需纠正");

        // 老配置：曾经关闭过 / 开过「接收前确认」→ 启动时纠正
        settings.enabled = false;
        settings.require_accept = true;
        assert!(enforce_always_on(&mut settings));
        assert!(settings.enabled);
        assert!(!settings.require_accept);
        assert!(!enforce_always_on(&mut settings), "纠正后幂等");

        // 纠正后的设置进入状态机，快照也应恒为开启
        let dir = tempfile::tempdir().unwrap();
        let state = LanShareState::with_settings(settings, dir.path().join("recv"));
        let snapshot = state.snapshot();
        assert!(snapshot.enabled);
        assert!(!snapshot.require_accept);
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
        let update = inner.upsert_peer(&announce("A", "fp-a", 53317), addr(), now, 1000);
        assert!(update.changed && update.seen);
        // 心跳续期：展示信息没变（changed=false），但 seen 必须为真，
        // 否则快照不刷新、前端 10 秒后把在线设备画成灰点。
        let update = inner.upsert_peer(&announce("A", "fp-a", 53317), addr(), now, 1000);
        assert!(!update.changed);
        assert!(update.seen);
        assert_eq!(inner.peers.len(), 1);
        assert_eq!(inner.peer_by_fingerprint("fp-a").unwrap().alias, "A");

        // 改名算变化
        let update = inner.upsert_peer(&announce("B", "fp-a", 53317), addr(), now, 2000);
        assert!(update.changed && update.seen);
        assert_eq!(inner.peer_by_fingerprint("fp-a").unwrap().alias, "B");

        // 15 秒内还在
        assert!(!inner.prune_peers(now + Duration::from_secs(14)));
        // 超过 15 秒被移除
        assert!(inner.prune_peers(now + PEER_TIMEOUT + Duration::from_secs(1)));
        assert!(inner.peers.is_empty());
    }

    /// 现场回归：两台设备都在线，附近设备的点却一直是灰的。
    /// 根因是快照只在「peer 展示信息变化」时推，last_seen_ms 停在首次发现；
    /// 现在心跳也会推快照，且在线判定按 last_seen 推进。
    #[test]
    fn heartbeat_refreshes_last_seen_in_snapshot() {
        let (_dir, state) = state_with_temp_dir();
        let now = Instant::now();
        let mut inner = lock_inner(&state.inner);
        inner.upsert_peer(&announce("Mac", "fp-mac", 53317), addr(), now, 1_000);
        assert_eq!(snapshot_of(&inner).peers[0].last_seen_ms, 1_000);

        let update = inner.upsert_peer(&announce("Mac", "fp-mac", 53317), addr(), now, 4_000);
        assert!(!update.changed);
        assert!(update.seen);
        assert_eq!(
            snapshot_of(&inner).peers[0].last_seen_ms,
            4_000,
            "心跳后快照里的 last_seen_ms 必须跟着刷新"
        );
        assert!(inner.is_peer_online("fp-mac", now));
        assert!(!inner.is_peer_online(
            "fp-mac",
            now + PEER_ONLINE_WINDOW + Duration::from_secs(1)
        ));
    }

    /// 现场问题回归：两台机器都在 192.168.3.0/24 又都开着 VPN，announce 从 LAN 网卡发出，
    /// 但真正连得上的只有 VPN 地址。peer 要把全部地址留着，探测成功后记住 VPN 地址，
    /// 后面的 announce 不能把它盖回 LAN 地址（否则每 3 秒重探测一次、面板地址乱跳）。
    #[test]
    fn peer_addresses_keep_probed_one() {
        let (_dir, state) = state_with_temp_dir();
        let now = Instant::now();
        let lan = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 3, 11)), 53317);
        let mut announce = announce("Mac", "fp-mac", 53317);
        announce.addresses = vec!["192.168.3.11".to_string(), "192.168.136.98".to_string()];

        let mut inner = lock_inner(&state.inner);
        let update = inner.upsert_peer(&announce, lan, now, 1000);
        assert!(update.changed);
        let peer = inner.peer_by_fingerprint("fp-mac").unwrap();
        assert_eq!(peer.ip, "192.168.3.11", "首选还是 announce 的来源地址");
        assert_eq!(peer.addresses.len(), 2);

        // 探测到 VPN 地址能连，写回
        assert!(inner.set_peer_ip("fp-mac", "192.168.136.98"));
        assert_eq!(
            inner.peer_by_fingerprint("fp-mac").unwrap().ip,
            "192.168.136.98"
        );

        // 下一条 announce 仍然从 LAN 网卡来：地址不翻回去，也不算「有变化」
        let update = inner.upsert_peer(&announce, lan, now, 4000);
        assert!(!update.changed);
        assert!(update.seen);
        assert_eq!(
            inner.peer_by_fingerprint("fp-mac").unwrap().ip,
            "192.168.136.98"
        );
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

    /// 旧版本 `lan_share.json` 没有新增字段，反序列化必须照常走默认值。
    #[test]
    fn old_settings_json_gets_new_defaults() {
        let old =
            r#"{"enabled":true,"alias":"办公室 Mac","download_dir":null,"fingerprint":"fp-old"}"#;
        let settings: LanShareSettings = serde_json::from_str(old).unwrap();
        assert!(!settings.require_accept, "默认免确认直接接收");
        assert!(settings.peer_remarks.is_empty());

        let empty: LanShareSettings = serde_json::from_str("{}").unwrap();
        assert!(!empty.require_accept);
        assert!(empty.peer_remarks.is_empty());
    }

    #[test]
    fn peer_remark_trim_truncates_and_clears() {
        let mut settings = LanShareSettings::default();
        apply_peer_remark(&mut settings, "AA:BB", &normalize_remark("  书房 Mac  "));
        assert_eq!(settings.peer_remarks.get("AA:BB").unwrap(), "书房 Mac");

        let long = normalize_remark(&"设".repeat(60));
        assert_eq!(long.chars().count(), 32);
        apply_peer_remark(&mut settings, "AA:BB", &long);
        assert_eq!(
            settings.peer_remarks.get("AA:BB").unwrap().chars().count(),
            32
        );

        apply_peer_remark(&mut settings, "AA:BB", "");
        assert!(settings.peer_remarks.is_empty());
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
    /// 同时覆盖聊天接线：B 的两条 outgoing 文件消息变 sent，A 收到两条 received。
    #[test]
    fn end_to_end_two_runtime_instances() {
        // A：接收端（这个用例覆盖「需要确认」路径，先打开确认开关）
        let (dir_a, state_a) = state_with_temp_dir();
        lock_inner(&state_a.inner).settings.require_accept = true;
        let store_a = attach_chat(&state_a, &dir_a.path().join("chat-a.db"));
        let server = server::HttpServer::start(state_a.inner.clone(), None, 0, 1).unwrap();
        {
            let mut guard = lock_inner(&state_a.inner);
            guard.running = true;
            guard.port = Some(server.port);
        }

        // B：发送端（只需要 client，不启动网络服务）
        let (dir_b, state_b) = state_with_temp_dir();
        let store_b = attach_chat(&state_b, &dir_b.path().join("chat-b.db"));
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
            addresses: vec!["127.0.0.1".to_string()],
        };
        // 发送端先落两条 sending 文件消息（生产路径在 send_files 里做）
        let message_ids = vec!["file-msg-0".to_string(), "file-msg-1".to_string()];
        for (index, name) in ["报告.txt", "second.bin"].iter().enumerate() {
            let mut message = ChatMessage::file(
                &message_ids[index],
                &peer.fingerprint,
                "out",
                *name,
                if index == 0 { 15 } else { 1024 },
                "sending",
                1_000,
            );
            message.transfer_id = Some("e2e-transfer".to_string());
            message.file_index = Some(index);
            message.file_total = Some(2);
            store_b
                .append_message(&peer.fingerprint, &peer.alias, &message)
                .unwrap();
        }

        let inner_b = state_b.inner.clone();
        let paths = vec![
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ];
        let send_peer = peer.clone();
        let send_handle = std::thread::spawn(move || {
            client::run_send(
                &inner_b,
                None,
                &send_peer,
                &paths,
                "e2e-transfer",
                &message_ids,
            )
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

        // 发送端文件消息都变成 sent
        let outgoing = store_b.list_messages(&peer.fingerprint, 10).unwrap();
        assert_eq!(outgoing.len(), 2);
        assert!(outgoing
            .iter()
            .all(|message| message.kind == "file" && message.status == "sent"));

        // 接收端：两条 received 文件消息，且会话已建立
        let sender_id = state_b.snapshot().fingerprint;
        let incoming = store_a.list_messages(&sender_id, 10).unwrap();
        assert_eq!(incoming.len(), 2, "接收端文件消息：{incoming:?}");
        assert!(incoming.iter().all(|message| {
            message.kind == "file"
                && message.direction == "in"
                && message.status == "received"
                && message.file_path.is_some()
        }));
        let conversations = store_a.list_conversations().unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, sender_id);
        assert_eq!(conversations[0].unread, 2);
    }

    /// 端到端聊天：B POST `/chat` → A 落库；B 的消息状态变 sent。
    #[test]
    fn end_to_end_chat_between_two_instances() {
        let (dir_a, state_a) = state_with_temp_dir();
        let store_a = attach_chat(&state_a, &dir_a.path().join("chat-a.db"));
        let server = server::HttpServer::start(state_a.inner.clone(), None, 0, 1).unwrap();
        {
            let mut guard = lock_inner(&state_a.inner);
            guard.running = true;
            guard.port = Some(server.port);
        }

        let (dir_b, state_b) = state_with_temp_dir();
        let store_b = attach_chat(&state_b, &dir_b.path().join("chat-b.db"));
        let sender_id = state_b.snapshot().fingerprint;
        let receiver_id = state_a.snapshot().fingerprint;
        let peer = client::SendPeer {
            fingerprint: receiver_id.clone(),
            alias: "接收端".to_string(),
            ip: "127.0.0.1".to_string(),
            port: server.port,
            addresses: vec!["127.0.0.1".to_string()],
        };

        let message_id = "chat-msg-1".to_string();
        let mut message =
            ChatMessage::text(&message_id, &receiver_id, "out", "你好", 1_000);
        message.status = "sending".to_string();
        store_b
            .append_message(&receiver_id, &peer.alias, &message)
            .unwrap();

        client::run_send_chat(&state_b.inner, None, &peer, &message_id, "你好")
            .expect("投递聊天消息失败");

        let incoming = store_a.list_messages(&sender_id, 10).unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].text, "你好");
        assert_eq!(incoming[0].direction, "in");
        assert_eq!(incoming[0].status, "received");

        let outgoing = store_b.list_messages(&receiver_id, 10).unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].status, "sent");

        let conversations = store_a.list_conversations().unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, sender_id);
        assert_eq!(conversations[0].unread, 1);
        assert_eq!(conversations[0].last_preview, "你好");
    }

    /// 回归：文件在 send_files 建好消息之后、后台线程收集文件之前消失时，
    /// 文件消息必须回写 failed，不能永远停在「发送中」。
    #[test]
    fn send_collect_failure_marks_file_messages_failed() {
        let (dir, state) = state_with_temp_dir();
        let store = attach_chat(&state, &dir.path().join("chat.db"));
        let peer = client::SendPeer {
            fingerprint: "peer-1".to_string(),
            alias: "对端".to_string(),
            ip: "127.0.0.1".to_string(),
            port: 53317,
            addresses: vec!["127.0.0.1".to_string()],
        };
        let message_id = "file-msg-x".to_string();
        let mut message = ChatMessage::file(
            &message_id,
            &peer.fingerprint,
            "out",
            "gone.txt",
            3,
            "sending",
            1_000,
        );
        message.transfer_id = Some("transfer-x".to_string());
        message.file_index = Some(0);
        message.file_total = Some(1);
        store
            .append_message(&peer.fingerprint, &peer.alias, &message)
            .unwrap();

        let result = client::run_send(
            &state.inner,
            None,
            &peer,
            &["/definitely/not/here.txt".to_string()],
            "transfer-x",
            &[message_id],
        );
        assert!(result.is_err());

        let stored = store.list_messages(&peer.fingerprint, 10).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, "failed", "不能永久停在 sending");
        assert!(
            stored[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("无法读取文件"),
            "错误信息：{:?}",
            stored[0].error
        );
    }
}
