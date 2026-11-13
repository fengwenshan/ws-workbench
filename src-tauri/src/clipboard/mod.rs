//! 剪贴板历史（macOS / Windows）：文本 / 图片 / 图文采集。
//!
//! 关键约束：
//! - 历史落 SQLite（`clipboard_items`），正文与 HTML 为密文信封，图片 WST1 密文；
//! - 采集用系统剪贴板的变化序列号驱动（macOS `NSPasteboard.changeCount`、
//!   Windows `GetClipboardSequenceNumber`），变了才读 text / html / image；
//! - 面板是独立窗口（label = [`PANEL_LABEL`]，页面 `clipboard.html`），全局快捷键
//!   （默认 [`DEFAULT_ACCELERATOR`]，管理页可改）只负责打开/聚焦，连按去抖、不收起；
//! - 选中历史只写回系统剪贴板，**不自动模拟 ⌘V / Ctrl+V**（不申请辅助功能授权）；
//! - 采集失败只记错误类型，绝不打印剪贴板内容。
//!
//! Linux 维持不启用：不采集、不注册热键、不建面板。
//!
//! 平台实现按 target 拆到 `macos.rs` / `windows.rs`，其它平台走 `unsupported.rs`；
//! 本文件只放平台无关的纯逻辑与门面分派，两个平台文件互不引用。

pub(crate) mod rich;
pub(crate) mod store;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_store::StoreExt;

/// 默认呼出面板的全局快捷键：mac 上是 ⌘⇧V，Windows 上是 Ctrl+Shift+V
pub const DEFAULT_ACCELERATOR: &str = "CommandOrControl+Shift+V";

/// 快捷键存在 config.json（store）里的键名
pub const SHORTCUT_STORE_KEY: &str = "clipboard_shortcut";
/// 和设置页共用的 store 文件名
const CONFIG_STORE_FILE: &str = "config.json";

/// 面板窗口 label，与 `lib.rs` 创建窗口时一致
pub const PANEL_LABEL: &str = "clipboard-panel";

/// 呼出面板前最前面的窗口（macOS 存 pid、Windows 存 HWND 原始值，排除自己）：
/// 复制完把焦点还回去，也用来判断「这次是从别的 App 呼出的」，
/// 避免激活应用时把主窗口一并带到前台。
#[cfg(any(target_os = "macos", target_os = "windows"))]
static PANEL_PREVIOUS_APP: Mutex<Option<isize>> = Mutex::new(None);

/// 历史变化时推给面板 / 管理页的事件
pub const EVENT_CHANGED: &str = "clipboard-changed";
/// 面板显示时推给前端的事件（用来聚焦搜索框、刷新列表）
pub const EVENT_SHOWN: &str = "clipboard-panel-shown";

/// 快捷键去抖窗口：连按多次只处理第一次
pub const HOTKEY_DEBOUNCE_MS: u64 = 300;
/// 单条文本字符上限；超出的复制不收录
pub const MAX_TEXT_CHARS: usize = 200_000;
/// 单条 HTML 字节上限；超出则只按纯文本记录
pub const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
/// 单张图片字节上限
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// 单条记录最多图片数
pub const MAX_IMAGES_PER_ITEM: usize = 50;
/// 单条记录（文本 + HTML + 图片）总量上限
pub const MAX_ITEM_BYTES: usize = 24 * 1024 * 1024;
/// 缩略图最长边
pub const THUMB_MAX_EDGE: u32 = 320;
/// 面板 / 管理页分页大小
pub const PAGE_SIZE: usize = 20;
/// 搜索结果上限
pub const SEARCH_LIMIT: usize = 100;
/// 轮询间隔：太快没必要，太慢会漏掉快速连续复制
const POLL_INTERVAL: Duration = Duration::from_millis(600);

/// 同一份内容在这么短时间内重复复制（按住 ⌘C 连按）就不再重复入库 / 置顶 / 推刷新
const RECOPY_DEDUPE_MS: i64 = 2_000;

/// 面板逻辑尺寸（与 `lib.rs` 创建窗口时的 inner_size 保持一致）
pub const PANEL_WIDTH: f64 = 380.0;
pub const PANEL_HEIGHT: f64 = 480.0;
/// 面板与鼠标之间的间隙、与屏幕边缘的留白（逻辑点）
const PANEL_GAP: i32 = 12;
const PANEL_MARGIN: i32 = 8;

/// 剪贴板运行时状态：已见变化序列号、最近一次热键时间、采集锁、当前快捷键。
pub struct ClipboardRuntime {
    pub(crate) last_change_count: AtomicI64,
    pub(crate) last_hotkey_ms: AtomicU64,
    pub(crate) capture_lock: Mutex<()>,
    /// 当前生效的全局快捷键（`CommandOrControl+Shift+V` 这类加速键字符串）
    pub(crate) shortcut: Mutex<String>,
    /// 面板是否「贴到屏幕上」：置顶后失焦、复制都不收起，关闭面板即失效
    pub(crate) pinned: AtomicBool,
    /// 最近一次采集到的内容哈希与时间：连按 ⌘C 时用来去重
    pub(crate) last_capture: Mutex<Option<(String, i64)>>,
}

impl ClipboardRuntime {
    pub fn new() -> Self {
        Self {
            last_change_count: AtomicI64::new(-1),
            last_hotkey_ms: AtomicU64::new(0),
            capture_lock: Mutex::new(()),
            shortcut: Mutex::new(DEFAULT_ACCELERATOR.to_string()),
            pinned: AtomicBool::new(false),
            last_capture: Mutex::new(None),
        }
    }
}

impl Default for ClipboardRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// 快捷键去抖：首次（last = 0）或间隔足够才放行。
pub fn allow_hotkey(last_ms: u64, now_ms: u64) -> bool {
    last_ms == 0 || now_ms.saturating_sub(last_ms) >= HOTKEY_DEBOUNCE_MS
}

/// 打开剪贴板用数据库连接（路径沿用 `db::db_path`）
pub(crate) fn db_conn(app: &AppHandle) -> Result<rusqlite::Connection, String> {
    let path = crate::db::db_path(app)?;
    crate::db::open(&path)
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn sm3_hex(bytes: &[u8]) -> String {
    use sm3::{Digest, Sm3};
    let mut hasher = Sm3::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn cleanup_blobs(dir: &Path, attachments: &[store::NewAttachment]) {
    for att in attachments {
        let _ = std::fs::remove_file(store::blob_path(dir, &att.id));
        let _ = std::fs::remove_file(store::thumb_path(dir, &att.id));
    }
}

/// 记录采集失败的类型（绝不包含剪贴板内容）
fn log_capture_err(stage: &str, err: &str) {
    eprintln!("[clipboard] capture failed ({stage}): {err}");
}

/// CRLF / CR 统一成 LF，其它内容保持原样
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 粗分类，只用于面板上的标签与视觉区分：
/// - 单行、不带空白、以 http(s):// 或 www. 开头 → 链接
/// - 命中常见代码特征（多行 + 代码符号 / 缩进）→ 代码
/// - 其它 → 文本
pub fn classify(text: &str) -> &'static str {
    let trimmed = text.trim();
    if !trimmed.is_empty()
        && !trimmed.contains(char::is_whitespace)
        && (trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("www.")
            || trimmed.starts_with("ftp://"))
    {
        return "url";
    }
    if looks_like_code(trimmed) {
        return "code";
    }
    "text"
}

fn looks_like_code(text: &str) -> bool {
    let has_newline = text.contains('\n');
    let indented = text
        .lines()
        .any(|line| line.starts_with("  ") || line.starts_with('\t'));
    let mut hits = 0usize;
    for needle in [
        "=>", "->", "()", "{}", "};", "</", "/>", "#!/", "<?", "function ", "const ", "let ",
        "var ", "def ", "class ", "import ", "return ", "#include", "SELECT ", "INSERT ",
        "UPDATE ", "printf(", "console.", "public ", "private ", "async ", "await ", "fn ",
        "impl ",
    ] {
        if text.contains(needle) {
            hits += 1;
            if hits >= 2 {
                return true;
            }
        }
    }
    if has_newline {
        // 多行文本里只要出现分号 / 花括号 / 缩进，基本就是代码或配置
        return hits >= 1 || indented;
    }
    false
}

// ============================== 额外表示 ==============================

/// 来源应用的富文本私有表示（例如钉钉的 `com.dingtalk.richTextAttributedString`）。
///
/// 只写 HTML 时，钉钉这类应用粘贴「文字+图片」只会贴出文字：它们的输入框认自己的
/// 私有格式（里面带图片路径 / 样式），所以把除文本、HTML、图片之外的表示原样留着，
/// 粘贴时再写回去。
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredFlavor {
    t: String,
    #[serde(default)]
    d: String,
}

fn encode_extra_flavors(flavors: &[(String, Vec<u8>)]) -> String {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    if flavors.is_empty() {
        return String::new();
    }
    let stored: Vec<StoredFlavor> = flavors
        .iter()
        .map(|(name, bytes)| StoredFlavor {
            t: name.clone(),
            d: B64.encode(bytes),
        })
        .collect();
    serde_json::to_string(&stored).unwrap_or_default()
}

fn decode_extra_flavors(json: &str) -> Vec<(String, Vec<u8>)> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    if json.is_empty() {
        return Vec::new();
    }
    let Ok(stored) = serde_json::from_str::<Vec<StoredFlavor>>(json) else {
        return Vec::new();
    };
    stored
        .into_iter()
        .filter_map(|flavor| B64.decode(flavor.d).ok().map(|bytes| (flavor.t, bytes)))
        .collect()
}

/// 解出某条记录的额外表示（密文 → `(类型名, 原始字节)`）
pub fn stored_extra_flavors(app: &AppHandle, extra_enc: &str) -> Vec<(String, Vec<u8>)> {
    if extra_enc.is_empty() {
        return Vec::new();
    }
    match crate::crypto::decrypt_str(app, extra_enc) {
        Ok(json) => decode_extra_flavors(&json),
        Err(err) => {
            log_capture_err("extra", &err);
            Vec::new()
        }
    }
}

// ============================== 采集线程 ==============================

/// 起后台采集线程：先把当前剪贴板收进来，之后每 [`POLL_INTERVAL`] 检查一次。
pub fn start_poller(app: AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        // 开发构建不采集：共享数据库里不会因为两边都在跑而出现重复历史
        if clipboard_background_disabled_in_dev() {
            eprintln!("[clipboard] 开发构建不采集剪贴板历史（发布版才启用）");
            return;
        }
        std::thread::Builder::new()
            .name("ws-clipboard-poller".to_string())
            .spawn(move || {
                // 启动时把当前剪贴板内容也收进来：它通常就是用户最近一次复制的东西
                let _ = capture_once(&app, true);

                loop {
                    std::thread::sleep(POLL_INTERVAL);
                    // 应用正在退出时 try_state 返回 None
                    if app.try_state::<ClipboardRuntime>().is_none() {
                        return;
                    }
                    let _ = capture_once(&app, false);
                }
            })
            .ok();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app;
    }
}

/// 检查并在有变化时采集一次。`force` 用于启动 / 面板唤出。
///
/// 返回是否写入了新记录。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn capture_once(app: &AppHandle, force: bool) -> bool {
    let Some(runtime) = app.try_state::<ClipboardRuntime>() else {
        return false;
    };
    let _guard = runtime
        .capture_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(count) = platform::change_count() else {
        return false;
    };
    if !force && count == runtime.last_change_count.load(Ordering::Relaxed) {
        return false;
    }
    runtime.last_change_count.store(count, Ordering::Relaxed);
    read_and_store(app)
}

/// 非 macOS / Windows：不采集（显式 stub 见 [`unsupported`]）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use unsupported::capture_once;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn read_and_store(app: &AppHandle) -> bool {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(err) => {
            log_capture_err("init", &err.to_string());
            return false;
        }
    };
    let raw_text = clipboard.get().text().unwrap_or_default();
    if raw_text.chars().count() > MAX_TEXT_CHARS {
        return false;
    }
    let mut text = normalize(&raw_text);
    let html = clipboard.get().html().unwrap_or_default();
    let image = clipboard.get().image().ok();
    if text.trim().is_empty() && html.is_empty() && image.is_none() {
        return false;
    }

    let mut images = Vec::<rich::ExtractedImage>::new();
    let mut stored_html = String::new();
    if !html.is_empty() && html.len() <= MAX_HTML_BYTES {
        let (rewritten, extracted) = rich::extract_inline_images(&html);
        if !extracted.is_empty() {
            stored_html = rewritten;
            images = extracted
                .into_iter()
                .take(MAX_IMAGES_PER_ITEM)
                .filter(|img| img.bytes.len() <= MAX_IMAGE_BYTES)
                .collect();
        }
    }

    let mut kind = if !images.is_empty() {
        "rich".to_string()
    } else if !text.trim().is_empty() {
        classify(&text).to_string()
    } else {
        "image".to_string()
    };

    // 没有内嵌图片但有位图：按「图片」记录（文本 / HTML 丢弃，按设计取舍）
    let mut image_item: Option<rich::ExtractedImage> = None;
    if images.is_empty() {
        if let Some(img) = image {
            match rich::encode_png_rgba(img.bytes.as_ref(), img.width as u32, img.height as u32) {
                Some(png) => {
                    if png.len() <= MAX_IMAGE_BYTES {
                        image_item = Some(rich::ExtractedImage {
                            mime: "image/png".into(),
                            width: img.width as u32,
                            height: img.height as u32,
                            bytes: png,
                            placeholder: String::new(),
                        });
                        kind = "image".into();
                        text.clear();
                        stored_html.clear();
                    }
                }
                None => {
                    log_capture_err("png", "encode failed");
                    return false;
                }
            }
        }
    }
    if kind == "image" && image_item.is_none() {
        return false;
    }
    if stored_html.is_empty() && text.trim().is_empty() && image_item.is_none() {
        return false;
    }

    // 图片列表（含纯图片条目）
    let mut all_images = images;
    if let Some(img) = image_item {
        all_images.push(img);
    }
    if all_images.is_empty() {
        kind = classify(&text).to_string();
    }

    // 来源应用的富文本私有表示（钉钉等）：粘贴时原样写回才能贴出图文
    let extras = platform::collect_extra_flavors();

    // 单条总量上限
    let total = text.len()
        + stored_html.len()
        + all_images
            .iter()
            .map(|img| img.bytes.len())
            .sum::<usize>()
        + extras.iter().map(|(name, bytes)| name.len() + bytes.len()).sum::<usize>();
    if total > MAX_ITEM_BYTES {
        return false;
    }

    let dir = match store::attachments_dir(app) {
        Ok(dir) => dir,
        Err(err) => {
            log_capture_err("dir", &err);
            return false;
        }
    };

    let mut attachments = Vec::new();
    let mut hashes = Vec::new();
    for (seq, image) in all_images.iter().enumerate() {
        let att_id = uuid::Uuid::new_v4().to_string();
        let blob = match crate::filecrypto::encrypt_bytes(app, &image.bytes) {
            Ok(blob) => blob,
            Err(err) => {
                log_capture_err("encrypt", &err);
                cleanup_blobs(&dir, &attachments);
                return false;
            }
        };
        let Some(thumb) = rich::png_thumbnail(&image.bytes, THUMB_MAX_EDGE) else {
            log_capture_err("thumb", "生成缩略图失败");
            cleanup_blobs(&dir, &attachments);
            return false;
        };
        let thumb_enc = match crate::filecrypto::encrypt_bytes(app, &thumb) {
            Ok(enc) => enc,
            Err(err) => {
                log_capture_err("encrypt-thumb", &err);
                cleanup_blobs(&dir, &attachments);
                return false;
            }
        };
        if write_atomic(&store::blob_path(&dir, &att_id), &blob).is_err()
            || write_atomic(&store::thumb_path(&dir, &att_id), &thumb_enc).is_err()
        {
            log_capture_err("write", "写入图片附件失败");
            cleanup_blobs(&dir, &attachments);
            return false;
        }
        // 把抽取时的随机占位符换成真正的 attachment_id
        if !image.placeholder.is_empty() {
            stored_html = stored_html.replace(
                &image.placeholder,
                &format!("{}{att_id}", rich::PLACEHOLDER_PREFIX),
            );
        }
        attachments.push(store::NewAttachment {
            id: att_id,
            item_id: String::new(),
            seq: seq as i64,
            mime: image.mime.clone(),
            width: image.width as i64,
            height: image.height as i64,
            orig_size: image.bytes.len() as i64,
            thumb_size: thumb.len() as i64,
        });
        hashes.push(sm3_hex(&image.bytes));
    }

    // 文本 / HTML 加密
    let text_enc = if text.is_empty() {
        String::new()
    } else {
        match crate::crypto::encrypt_str(app, &text) {
            Ok(value) => value,
            Err(err) => {
                log_capture_err("encrypt-text", &err);
                cleanup_blobs(&dir, &attachments);
                return false;
            }
        }
    };
    let html_enc = if stored_html.is_empty() {
        String::new()
    } else {
        match crate::crypto::encrypt_str(app, &stored_html) {
            Ok(value) => value,
            Err(err) => {
                log_capture_err("encrypt-html", &err);
                cleanup_blobs(&dir, &attachments);
                return false;
            }
        }
    };
    // 额外表示是可选增强：存不下就退化成只写 HTML / 纯文本
    let extra_enc = if extras.is_empty() {
        String::new()
    } else {
        let encoded = encode_extra_flavors(&extras);
        match crate::crypto::encrypt_str(app, &encoded) {
            Ok(value) => value,
            Err(err) => {
                log_capture_err("encrypt-extra", &err);
                String::new()
            }
        }
    };
    let hash = rich::content_hash(&text, &stored_html, &hashes);

    // 按住 ⌘C 连按 / 反复复制同一份内容：直接跳过，不重复入库、不置顶、不推刷新事件
    if let Some(runtime) = app.try_state::<ClipboardRuntime>() {
        let mut last = runtime
            .last_capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = now_ms();
        if let Some((previous, at)) = last.as_ref() {
            if previous == &hash && now - *at < RECOPY_DEDUPE_MS {
                cleanup_blobs(&dir, &attachments);
                return true;
            }
        }
        *last = Some((hash.clone(), now));
    }

    let conn = match db_conn(app) {
        Ok(conn) => conn,
        Err(err) => {
            log_capture_err("db", &err);
            cleanup_blobs(&dir, &attachments);
            return false;
        }
    };
    if let Ok(Some(existing)) = store::find_by_hash(&conn, &hash) {
        // 同一内容再复制一次时可能才带来来源应用的富文本表示（例如先复制文字、
        // 再复制图文），补到已有记录上，粘贴时就能用上
        if existing.extra_enc.is_empty() && !extra_enc.is_empty() {
            let _ = store::set_extra_enc(&conn, &existing.id, &extra_enc);
        }
        let _ = store::touch(&conn, &existing.id, now_ms());
        cleanup_blobs(&dir, &attachments);
        let _ = app.emit(EVENT_CHANGED, ());
        return true;
    }

    let item_id = uuid::Uuid::new_v4().to_string();
    for att in &mut attachments {
        att.item_id = item_id.clone();
    }
    let new_item = store::NewItem {
        id: item_id,
        kind,
        created_at: now_ms(),
        content_hash: hash,
        text_enc,
        html_enc,
        image_count: attachments.len() as i64,
        byte_size: (text.len() + stored_html.len()) as i64
            + attachments.iter().map(|att| att.orig_size).sum::<i64>(),
        extra_enc,
    };
    if let Err(err) = store::insert_item(&conn, &new_item, &attachments) {
        log_capture_err("insert", &err);
        cleanup_blobs(&dir, &attachments);
        return false;
    }
    let _ = app.emit(EVENT_CHANGED, ());
    true
}

/// 自己写回剪贴板后调用：把已见序列号同步到当前值，避免把自己写的内容再收一遍。
pub fn note_self_write(app: &AppHandle) {
    if let (Some(runtime), Some(count)) =
        (app.try_state::<ClipboardRuntime>(), platform::change_count())
    {
        runtime.last_change_count.store(count, Ordering::Relaxed);
    }
}

// ============================== 全局快捷键 ==============================

/// 两个加速键字符串是否指向同一个热键（`Command` / `CommandOrControl` 这类别名）。
///
/// 字符串不同但解析结果相同时不能再注册一次，系统会报「已被占用」。
fn parses_to_same_hotkey(a: &str, b: &str) -> bool {
    let parse = |value: &str| value.parse::<tauri_plugin_global_shortcut::Shortcut>().ok();
    match (parse(a), parse(b)) {
        (Some(next), Some(prev)) => next == prev,
        _ => false,
    }
}

/// 当前生效的加速键字符串（管理页 / 面板徽标读这个）。
pub fn current_accelerator(app: &AppHandle) -> String {
    app.try_state::<ClipboardRuntime>()
        .map(|runtime| runtime.shortcut.lock().unwrap().clone())
        .unwrap_or_else(|| DEFAULT_ACCELERATOR.to_string())
}

fn set_current_accelerator(app: &AppHandle, accelerator: &str) {
    if let Some(runtime) = app.try_state::<ClipboardRuntime>() {
        *runtime.shortcut.lock().unwrap() = accelerator.to_string();
    }
}

/// 从 config.json 读用户保存过的快捷键
/// 读用户保存的快捷键。
///
/// `None` = 从没设置过（用默认组合）；`Some("")` = 用户主动清空过，不注册任何快捷键。
fn stored_accelerator(app: &AppHandle) -> Option<String> {
    let store = app.store(CONFIG_STORE_FILE).ok()?;
    Some(store.get(SHORTCUT_STORE_KEY)?.as_str()?.trim().to_string())
}

fn save_accelerator(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let store = app
        .store(CONFIG_STORE_FILE)
        .map_err(|err| format!("保存快捷键失败: {err}"))?;
    store.set(SHORTCUT_STORE_KEY, accelerator.to_string());
    store.save().map_err(|err| format!("保存快捷键失败: {err}"))
}

/// 开发构建（`pnpm exec tauri dev`）不跑剪贴板的后台部分：不注册全局快捷键、不采集历史。
///
/// 两个原因：
/// 1. 全局快捷键是系统级独占：开发版和安装版同时跑时，谁先注册谁拿到，
///    开发版抢走就会让生产版的面板呼不出来；
/// 2. 开发版和安装版共用同一个数据库，采集线程两边都跑的话，用户每复制一次
///    历史里就会出现两条（各自插一条）。剪贴板面板已经开发完，日常开发不需要它。
///
/// 发布构建（release）照常注册快捷键、照常采集。
fn clipboard_background_disabled_in_dev() -> bool {
    cfg!(debug_assertions)
}

/// 启动时注册快捷键：优先用户保存的组合，不可用再退回默认值。
pub fn register_saved_shortcut(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    if clipboard_background_disabled_in_dev() {
        eprintln!("[clipboard] 开发构建不注册全局快捷键（发布版才启用）");
        return;
    }

    let saved = match stored_accelerator(app) {
        // 用户主动清空过：不注册快捷键，面板只能从主窗口改回组合后再用
        Some(accelerator) if accelerator.is_empty() => {
            set_current_accelerator(app, "");
            eprintln!("[clipboard] 未设置全局快捷键，跳过注册");
            return;
        }
        Some(accelerator) => accelerator,
        None => DEFAULT_ACCELERATOR.to_string(),
    };
    let shortcuts = app.global_shortcut();
    match shortcuts.register(saved.as_str()) {
        Ok(()) => {
            set_current_accelerator(app, &saved);
            eprintln!("[clipboard] 全局快捷键 {saved} 已注册");
        }
        Err(err) => {
            eprintln!("[clipboard] 全局快捷键 {saved} 注册失败: {err}");
            if saved != DEFAULT_ACCELERATOR && shortcuts.register(DEFAULT_ACCELERATOR).is_ok() {
                set_current_accelerator(app, DEFAULT_ACCELERATOR);
                eprintln!("[clipboard] 已回退到默认快捷键 {DEFAULT_ACCELERATOR}");
            }
        }
    }
}

/// 修改快捷键：先注册新的（被占用 / 格式不对就直接报错，旧快捷键保持可用），
/// 再注销旧的、写回 config.json。
pub fn apply_shortcut(app: &AppHandle, accelerator: &str) -> Result<String, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    if clipboard_background_disabled_in_dev() {
        return Err("开发构建不注册全局快捷键，请在发布版里修改".to_string());
    }

    let accelerator = accelerator.trim();
    let old = current_accelerator(app);

    // 清空快捷键：先落盘，成功后再注销当前组合；失败时原组合继续可用
    if accelerator.is_empty() {
        save_accelerator(app, "")?;
        if !old.is_empty() {
            let _ = app.global_shortcut().unregister(old.as_str());
        }
        set_current_accelerator(app, "");
        eprintln!("[clipboard] 全局快捷键已清空");
        return Ok(String::new());
    }
    if accelerator == old || parses_to_same_hotkey(accelerator, &old) {
        save_accelerator(app, accelerator)?;
        set_current_accelerator(app, accelerator);
        return Ok(accelerator.to_string());
    }

    let shortcuts = app.global_shortcut();
    shortcuts
        .register(accelerator)
        .map_err(|err| format!("快捷键不可用: {err}"))?;
    if let Err(err) = save_accelerator(app, accelerator) {
        // 存不下就回滚，避免「这次能用、重启就丢」
        let _ = shortcuts.unregister(accelerator);
        return Err(err);
    }
    let _ = shortcuts.unregister(old.as_str());
    set_current_accelerator(app, accelerator);
    Ok(accelerator.to_string())
}

/// 面板是否被置顶（贴屏幕）。置顶后失焦、复制都不收起。
pub fn is_pinned(app: &AppHandle) -> bool {
    app.try_state::<ClipboardRuntime>()
        .map(|runtime| runtime.pinned.load(Ordering::Relaxed))
        .unwrap_or(false)
}

/// 设置面板置顶状态，返回设置后的值。
pub fn set_pinned(app: &AppHandle, pinned: bool) -> bool {
    if let Some(runtime) = app.try_state::<ClipboardRuntime>() {
        runtime.pinned.store(pinned, Ordering::Relaxed);
        return pinned;
    }
    false
}

/// 光标是否还停在面板窗口里（Windows）。
///
/// 拖标题栏走的是 Windows 的 SC_MOVE 模态循环，循环开始时会先给窗口发一次
/// `WM_ACTIVATE(WA_INACTIVE)`；失焦处理器如果无脑 `hide()`，拖拽会被当场掐断，
/// 用户看到的就是「点一下标题栏面板就没了」。鼠标还在面板里就当作「在拖自己」，
/// 真正点外面时鼠标已经不在面板矩形内。
///
/// macOS / 其它平台沿用原来的「失焦即收起」行为，没有 SC_MOVE，直接返回 false。
#[cfg(target_os = "windows")]
pub(crate) use windows::cursor_inside_hwnd;

#[cfg(not(target_os = "windows"))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn cursor_inside_hwnd(_raw: isize) -> bool {
    false
}

// ============================== 面板显示 ==============================

/// 快捷键处理：去抖 + 只负责显示/聚焦 + 异步补采一次剪贴板。
///
/// 面板已显示时再按也不收起（用户怕没复制上会连按）。
pub fn on_hotkey(app: &AppHandle) {
    let Some(runtime) = app.try_state::<ClipboardRuntime>() else {
        return;
    };
    let now = now_ms() as u64;
    let last = runtime.last_hotkey_ms.load(Ordering::Relaxed);
    if !allow_hotkey(last, now) {
        return;
    }
    runtime.last_hotkey_ms.store(now, Ordering::Relaxed);

    show_panel(app);

    // 采集可能较慢（加密大图），异步进行；采集完成会发 EVENT_CHANGED 让面板刷新
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = capture_once(&app, true);
    });
}

/// 显示并聚焦面板（不切换隐藏）
pub fn show_panel(app: &AppHandle) {
    // 面板是懒创建的：启动时不再建，主窗口就绪后预热；预热没跑完就按了快捷键时现场建。
    let Some(window) = crate::ensure_clipboard_panel(app) else {
        return;
    };

    // 只记第一次呼出时最前面的窗口：连按热键时前台已经变成自己，再记就丢了目标
    let already_open = window.is_visible().unwrap_or(false);
    // 上次关掉时置顶已经失效，重新打开一律从「不置顶」开始
    if !already_open {
        set_pinned(app, false);
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if !already_open {
        platform::remember_frontmost_app();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = already_open;

    // 应用可能在后台：先激活，否则窗口显示了也拿不到键盘焦点
    activate_app(app);

    if let Some((x, y, logical)) = platform::panel_position(app, &window) {
        if logical {
            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
        } else {
            let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
        }
    }
    let _ = window.show();
    let _ = window.set_focus();

    // 激活会把主窗口一起带到前台：从别的 App 呼出时把主窗口压回后面，
    // 别让面板收起后主窗口顶掉用户正在用的 App
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if !already_open {
        platform::send_main_window_to_back(app);
    }

    let _ = app.emit(EVENT_SHOWN, is_pinned(app));
}

/// 收起面板（Esc / 失焦 / 复制后由前端调用）
pub fn hide_panel(app: &AppHandle) {
    // 收起面板就清掉置顶，下次呼出重新点「置顶」才生效
    set_pinned(app, false);
    // Windows：必须在面板还握着前台时把前台让给目标应用；先隐藏再抢前台会被系统拒绝
    #[cfg(target_os = "windows")]
    platform::restore_frontmost_app(app);
    if let Some(window) = app.get_webview_window(PANEL_LABEL) {
        let _ = window.hide();
    }
    // 复制 / 取消后把焦点还给呼出面板前的 App，用户直接按 ⌘V / Ctrl+V 就能粘贴。
    // 失焦收起走 lib.rs 里的窗口事件，那条路径不抢焦点。
    #[cfg(target_os = "macos")]
    platform::restore_frontmost_app(app);
}

/// 计算面板左上角（逻辑点）：水平居中于鼠标、向下偏移 [`PANEL_GAP`]，
/// 最后夹进显示器范围，避免贴边时超出屏幕。
///
/// 纯函数，单测覆盖主屏中间 / 左右边缘 / 底部边缘 / 多显示器负坐标。
pub fn panel_origin_at(
    cursor: (i32, i32),
    panel: (u32, u32),
    screen: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (cursor_x, cursor_y) = cursor;
    let (panel_w, panel_h) = (panel.0 as i32, panel.1 as i32);
    let (screen_x, screen_y, screen_w, screen_h) = screen;

    let mut x = cursor_x - panel_w / 2;
    let mut y = cursor_y + PANEL_GAP;

    // 屏幕比面板还小时上限会小于下限，先撑住再夹
    let max_x = (screen_x + screen_w - panel_w - PANEL_MARGIN).max(screen_x + PANEL_MARGIN);
    x = x.min(max_x).max(screen_x + PANEL_MARGIN);
    let max_y = (screen_y + screen_h - panel_h - PANEL_MARGIN).max(screen_y + PANEL_MARGIN);
    y = y.min(max_y).max(screen_y + PANEL_MARGIN);
    (x, y)
}

/// 把应用带到前台（macOS 需要；Windows 呼出面板时由系统激活，调用无副作用）。
pub(crate) fn activate_app<R: tauri::Runtime>(app: &AppHandle<R>) {
    platform::activate_app(app);
}

/// 写回系统剪贴板：HTML（可选）+ 纯文本 + 图片（可选）+ 来源应用的额外表示。
///
/// - 图文混排：HTML + 纯文本 + 额外表示（钉钉的私有富文本等，能一次贴出文字+图片）；
///   聊天输入框没有富文本表示时会回落到文字，图片用大图里的「复制图片」单独贴
/// - 纯图片：PNG + TIFF
pub fn write_pasteboard(
    app: &AppHandle,
    html: Option<&str>,
    text: &str,
    png: Option<Vec<u8>>,
    extras: &[(String, Vec<u8>)],
) -> Result<(), String> {
    platform::write_pasteboard(app, html, text, png, extras)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes_extra_flavors() {
        let flavors = vec![(
            "com.dingtalk.richTextAttributedString".to_string(),
            br#"{"contents":[]}"#.to_vec(),
        )];
        let encoded = encode_extra_flavors(&flavors);
        assert!(encoded.starts_with('['));
        assert_eq!(decode_extra_flavors(&encoded), flavors);
        assert!(encode_extra_flavors(&[]).is_empty());
        assert!(decode_extra_flavors("").is_empty());
        assert!(decode_extra_flavors("not json").is_empty());
    }

    #[test]
    fn classifies_url_code_and_text() {
        assert_eq!(classify("https://example.com/a?b=1"), "url");
        assert_eq!(classify("www.example.com"), "url");
        assert_eq!(classify("const x = 1;\nfunction f() { return x }"), "code");
        assert_eq!(classify("SELECT * FROM users WHERE id = 1"), "text");
        assert_eq!(classify("今天下午三点开会"), "text");
        assert_eq!(
            classify("  if (ok) {\n    console.log(ok)\n  }"),
            "code",
            "缩进 + 代码符号应识别为代码"
        );
    }

    #[test]
    fn normalizes_crlf() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn default_accelerator_is_parseable() {
        use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
        let shortcut: Shortcut = DEFAULT_ACCELERATOR
            .parse()
            .expect("accelerator 应能被 global-hotkey 解析");
        // macOS 上 CommandOrControl 落到 SUPER，Windows 上落到 CONTROL
        #[cfg(target_os = "macos")]
        assert!(shortcut.mods.contains(Modifiers::SUPER), "应为 Command");
        #[cfg(not(target_os = "macos"))]
        assert!(shortcut.mods.contains(Modifiers::CONTROL), "应为 Control");
        assert!(shortcut.mods.contains(Modifiers::SHIFT), "应为 Shift");
        assert_eq!(shortcut.key, Code::KeyV);
    }

    #[test]
    fn hotkey_debounce_allows_first_and_blocks_repeats() {
        assert!(allow_hotkey(0, 1_000));
        assert!(allow_hotkey(1_000, 1_400));
        assert!(!allow_hotkey(1_000, 1_100));
        assert!(!allow_hotkey(1_000, 1_000));
    }

    #[test]
    fn panel_centers_on_cursor_and_clamps() {
        let panel = (400, 300);
        let screen = (0, 0, 1920, 1080);
        assert_eq!(panel_origin_at((800, 400), panel, screen), (600, 412));
        // 左边缘：夹到 margin
        assert_eq!(panel_origin_at((10, 400), panel, screen).0, 8);
        // 右边缘：夹进屏幕
        assert_eq!(
            panel_origin_at((1900, 400), panel, screen).0,
            1920 - 400 - 8
        );
        // 底部：夹进屏幕
        assert_eq!(
            panel_origin_at((800, 1070), panel, screen).1,
            1080 - 300 - 8
        );
        // 多显示器负坐标
        let left_screen = (-1920, 0, 1920, 1080);
        assert_eq!(
            panel_origin_at((-1000, 300), panel, left_screen),
            (-1200, 312)
        );
    }

    #[test]
    fn parses_aliases_to_same_hotkey() {
        #[cfg(target_os = "macos")]
        assert!(parses_to_same_hotkey(
            "CommandOrControl+Shift+V",
            "Command+Shift+V"
        ));
        #[cfg(target_os = "windows")]
        assert!(parses_to_same_hotkey(
            "CommandOrControl+Shift+V",
            "Control+Shift+V"
        ));
        assert!(parses_to_same_hotkey("Alt+Shift+V", "Alt+Shift+V"));
        assert!(!parses_to_same_hotkey("Alt+Shift+V", "Shift+V"));
        assert!(!parses_to_same_hotkey("Alt+Shift+V", "not-a-key"));
    }

    #[test]
    fn panel_handles_screen_smaller_than_panel() {
        let (x, y) = panel_origin_at((50, 50), (800, 600), (0, 0, 400, 300));
        assert_eq!((x, y), (8, 8));
    }
}
