//! 剪贴板历史（macOS）：文本 / 图片 / 图文采集。
//!
//! 关键约束：
//! - 历史落 SQLite（`clipboard_items`），正文与 HTML 为密文信封，图片 WST1 密文；
//! - 采集用 `NSPasteboard.changeCount` 驱动：变了才读 text / html / image；
//! - 面板是独立窗口（label = [`PANEL_LABEL`]，页面 `clipboard.html`），全局快捷键
//!   [`HOTKEY_ACCELERATOR`] 只负责打开/聚焦，连按去抖、不收起；
//! - 选中历史只写回系统剪贴板，**不自动模拟 ⌘V**（不申请辅助功能授权）；
//! - 采集失败只记错误类型，绝不打印剪贴板内容。
//!
//! Windows 系统自带 Win+V，本期不启用：不采集、不注册热键、不建面板。

pub(crate) mod rich;
pub(crate) mod store;

use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

/// 呼出面板的全局快捷键（`lib.rs` 注册、单测校验可解析；面板徽标写 `⌘⇧V`）
pub const HOTKEY_ACCELERATOR: &str = "Command+Shift+V";

/// 面板窗口 label，与 `lib.rs` 创建窗口时一致
pub const PANEL_LABEL: &str = "clipboard-panel";
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

/// 面板逻辑尺寸（与 `lib.rs` 创建窗口时的 inner_size 保持一致）
pub const PANEL_WIDTH: f64 = 380.0;
pub const PANEL_HEIGHT: f64 = 480.0;
/// 面板与鼠标之间的间隙、与屏幕边缘的留白（逻辑点）
const PANEL_GAP: i32 = 12;
const PANEL_MARGIN: i32 = 8;

/// 剪贴板运行时状态：已见 changeCount、最近一次热键时间、采集锁。
pub struct ClipboardRuntime {
    pub(crate) last_change_count: AtomicI64,
    pub(crate) last_hotkey_ms: AtomicU64,
    pub(crate) capture_lock: Mutex<()>,
}

impl ClipboardRuntime {
    pub fn new() -> Self {
        Self {
            last_change_count: AtomicI64::new(-1),
            last_hotkey_ms: AtomicU64::new(0),
            capture_lock: Mutex::new(()),
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

// ============================== 采集线程 ==============================

/// 起后台采集线程：先把当前剪贴板收进来，之后每 [`POLL_INTERVAL`] 检查一次。
pub fn start_poller(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
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
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

/// 当前系统剪贴板的 changeCount（macOS 专用；其它平台返回 None）
#[cfg(target_os = "macos")]
fn change_count() -> Option<i64> {
    use objc2_app_kit::NSPasteboard;
    Some(NSPasteboard::generalPasteboard().changeCount() as i64)
}

#[cfg(not(target_os = "macos"))]
fn change_count() -> Option<i64> {
    None
}

/// 检查并在有变化时采集一次。`force` 用于启动 / 面板唤出。
///
/// 返回是否写入了新记录。
#[cfg(target_os = "macos")]
pub fn capture_once(app: &AppHandle, force: bool) -> bool {
    let Some(runtime) = app.try_state::<ClipboardRuntime>() else {
        return false;
    };
    let _guard = runtime
        .capture_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(count) = change_count() else {
        return false;
    };
    if !force && count == runtime.last_change_count.load(Ordering::Relaxed) {
        return false;
    }
    runtime.last_change_count.store(count, Ordering::Relaxed);
    read_and_store(app)
}

#[cfg(not(target_os = "macos"))]
pub fn capture_once(_app: &AppHandle, _force: bool) -> bool {
    false
}

#[cfg(target_os = "macos")]
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

    // 单条总量上限
    let total = text.len()
        + stored_html.len()
        + all_images
            .iter()
            .map(|img| img.bytes.len())
            .sum::<usize>();
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
    let hash = rich::content_hash(&text, &stored_html, &hashes);

    let conn = match db_conn(app) {
        Ok(conn) => conn,
        Err(err) => {
            log_capture_err("db", &err);
            cleanup_blobs(&dir, &attachments);
            return false;
        }
    };
    if let Ok(Some(existing)) = store::find_by_hash(&conn, &hash) {
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
    };
    if let Err(err) = store::insert_item(&conn, &new_item, &attachments) {
        log_capture_err("insert", &err);
        cleanup_blobs(&dir, &attachments);
        return false;
    }
    let _ = app.emit(EVENT_CHANGED, ());
    true
}

/// 自己写回剪贴板后调用：把已见 changeCount 同步到当前值，避免把自己写的内容再收一遍。
pub fn note_self_write(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let (Some(runtime), Some(count)) = (app.try_state::<ClipboardRuntime>(), change_count()) {
        runtime.last_change_count.store(count, Ordering::Relaxed);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
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
    let Some(window) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };

    // 应用可能在后台：先激活，否则窗口显示了也拿不到键盘焦点
    activate_app(app);

    if let Some((x, y, logical)) = panel_position(app, &window) {
        if logical {
            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
        } else {
            let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
        }
    }
    let _ = window.show();
    let _ = window.set_focus();
    let _ = app.emit(EVENT_SHOWN, ());
}

/// 收起面板（Esc / 失焦 / 复制后由前端调用）
pub fn hide_panel(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(PANEL_LABEL) {
        let _ = window.hide();
    }
}

/// 面板显示位置：返回 `(x, y, 是否逻辑点)`。
///
/// macOS 上统一用 AppKit 逻辑点（和 `lib.rs` 里托盘面板同一套坐标处理），
/// 其它平台用物理像素兜底；拿不到光标时返回 None，窗口保持原位置。
#[cfg(target_os = "macos")]
fn panel_position(_app: &AppHandle, _window: &tauri::WebviewWindow) -> Option<(f64, f64, bool)> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSEvent;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
        fn CGGetActiveDisplayList(max_displays: u32, displays: *mut u32, count: *mut u32) -> i32;
    }

    // 全局快捷键回调本来就在主线程；面板里的再按一次也由主线程窗口事件触发
    MainThreadMarker::new()?;

    let mouse = NSEvent::mouseLocation();
    let primary = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    let cursor = (
        mouse.x.round() as i32,
        (primary.origin.y + primary.size.height - mouse.y).round() as i32,
    );

    let mut ids = [0u32; 16];
    let mut count = 0u32;
    let status = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if status != 0 {
        return None;
    }
    let screen = ids[..count as usize].iter().find_map(|display| {
        let bounds = unsafe { CGDisplayBounds(*display) };
        let (x, y) = (
            bounds.origin.x.round() as i32,
            bounds.origin.y.round() as i32,
        );
        let (w, h) = (
            bounds.size.width.round() as i32,
            bounds.size.height.round() as i32,
        );
        (cursor.0 >= x && cursor.0 < x + w && cursor.1 >= y && cursor.1 < y + h)
            .then_some((x, y, w, h))
    })?;

    let (x, y) = panel_origin_at(cursor, (PANEL_WIDTH as u32, PANEL_HEIGHT as u32), screen);
    Some((x as f64, y as f64, true))
}

#[cfg(not(target_os = "macos"))]
fn panel_position(app: &AppHandle, window: &tauri::WebviewWindow) -> Option<(f64, f64, bool)> {
    let cursor = app.cursor_position().ok()?;
    let anchor = (cursor.x as i32, cursor.y as i32);
    let monitor = app.monitor_from_point(cursor.x, cursor.y).ok().flatten()?;
    let position = monitor.position();
    let size = monitor.size();
    let screen = (
        position.x,
        position.y,
        size.width as i32,
        size.height as i32,
    );
    let scale = window.scale_factor().ok()?;
    let panel = (
        (PANEL_WIDTH * scale).round() as u32,
        (PANEL_HEIGHT * scale).round() as u32,
    );
    let (x, y) = panel_origin_at(anchor, panel, screen);
    Some((x as f64, y as f64, false))
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

/// macOS：把应用带到前台。全局热键触发时应用通常不在前台，
/// 只 `show()` + `set_focus()` 窗口可能拿不到键盘焦点。
#[cfg(target_os = "macos")]
fn activate_app(app: &AppHandle) {
    // 全局热键回调本来就在主线程；不在时再转一次。不能无条件 run_on_main_thread，
    // 否则「激活」会排在 show/focus 之后，窗口还是拿不到焦点。
    fn activate_now() {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        if let Some(mtm) = MainThreadMarker::new() {
            let ns_app = NSApplication::sharedApplication(mtm);
            // 兼容 macOS 10.15：新 API activate() 要求 macOS 14+，不能直接换
            #[allow(deprecated)]
            ns_app.activateIgnoringOtherApps(true);
        }
    }
    if objc2::MainThreadMarker::new().is_some() {
        activate_now();
    } else {
        let _ = app.run_on_main_thread(activate_now);
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_app(_app: &AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn hotkey_accelerator_is_command_shift_v() {
        use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
        let shortcut: Shortcut = HOTKEY_ACCELERATOR
            .parse()
            .expect("accelerator 应能被 global-hotkey 解析");
        assert!(shortcut.mods.contains(Modifiers::SUPER), "应为 Command");
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
    fn panel_handles_screen_smaller_than_panel() {
        let (x, y) = panel_origin_at((50, 50), (800, 600), (0, 0, 400, 300));
        assert_eq!((x, y), (8, 8));
    }
}
