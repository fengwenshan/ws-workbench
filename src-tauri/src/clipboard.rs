//! 剪贴板历史（macOS 一期）。
//!
//! 关键约束：
//! - 历史只存**内存**，最多 [`MAX_ITEMS`] 条，进程退出即清空（不落盘、不进日志）；
//! - 采集用官方 `tauri-plugin-clipboard-manager`，后台线程每 [`POLL_INTERVAL`] 读一次文本；
//! - 面板是独立窗口（label = [`PANEL_LABEL`]，页面 `clipboard.html`），由全局快捷键
//!   `Command+Shift+V` 呼出。定位在鼠标附近，失焦或再按一次收起；
//! - 选中历史只写回系统剪贴板，**不自动模拟 ⌘V**（不申请辅助功能授权）。
//!
//! Windows 系统自带 Win+V，本期不启用：不采集、不注册热键、不建面板。

mod rich;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

/// 呼出面板的全局快捷键（`lib.rs` 注册、单测校验可解析；面板徽标写 `⌘⇧V`）
pub const HOTKEY_ACCELERATOR: &str = "Command+Shift+V";

/// 面板窗口 label，与 `lib.rs` 创建窗口时一致
pub const PANEL_LABEL: &str = "clipboard-panel";
/// 历史变化时推给面板的事件
pub const EVENT_CHANGED: &str = "clipboard-changed";
/// 面板显示时推给前端的事件（用来聚焦搜索框、刷新列表）
pub const EVENT_SHOWN: &str = "clipboard-panel-shown";

/// 最多保留的条数（内存中，超出丢最旧）
pub const MAX_ITEMS: usize = 20;
/// 单条文本字符上限；超出的复制不收录，避免剪贴板大文本把内存吃掉
pub const MAX_TEXT_CHARS: usize = 200_000;
/// 轮询间隔：太快没必要，太慢会漏掉快速连续复制
const POLL_INTERVAL: Duration = Duration::from_millis(600);

/// 面板逻辑尺寸（与 `lib.rs` 创建窗口时的 inner_size 保持一致）
pub const PANEL_WIDTH: f64 = 380.0;
pub const PANEL_HEIGHT: f64 = 480.0;
/// 面板与鼠标之间的间隙、与屏幕边缘的留白（逻辑点）
const PANEL_GAP: i32 = 12;
const PANEL_MARGIN: i32 = 8;

/// 一条剪贴板历史
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardItem {
    /// uuid v4，前端列表的 key
    pub id: String,
    /// 原始文本（只做 CRLF → LF 归一，不 trim，保留用户复制的缩进）
    pub text: String,
    /// 粗分类：`url` / `code` / `text`
    pub kind: String,
    /// 收录时间（unix 毫秒）
    pub created_at: i64,
}

#[derive(Default)]
struct Inner {
    items: VecDeque<ClipboardItem>,
    /// 上一次轮询读到的文本；和自己写回的内容比较，避免每 600ms 重复入栈
    last_seen: Option<String>,
}

/// 剪贴板历史状态，注册给 Tauri 托管。
#[derive(Default)]
pub struct ClipboardHistory {
    inner: Mutex<Inner>,
}

impl ClipboardHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<ClipboardItem> {
        match self.inner.lock() {
            Ok(inner) => inner.items.iter().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn clear(&self) -> Vec<ClipboardItem> {
        if let Ok(mut inner) = self.inner.lock() {
            inner.items.clear();
        }
        Vec::new()
    }

    /// 收到一段新文本：去重（相同文本移到最前）、裁到上限、跳过空白/超长。
    /// 返回历史是否发生变化（调用方据此决定要不要发事件）。
    pub fn push(&self, text: &str) -> bool {
        let normalized = normalize(text);
        if normalized.trim().is_empty() {
            return false;
        }

        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return false,
        };

        // 超长文本不收录，但也记成「已看过」，否则每轮都会再判断一次
        if normalized.chars().count() > MAX_TEXT_CHARS {
            inner.last_seen = Some(normalized);
            return false;
        }
        if inner.last_seen.as_deref() == Some(normalized.as_str()) {
            return false;
        }
        inner.last_seen = Some(normalized.clone());

        // 去重：相同文本已存在就移出，统一按「新条目」放到最前
        if let Some(index) = inner.items.iter().position(|item| item.text == normalized) {
            inner.items.remove(index);
        }

        inner.items.push_front(ClipboardItem {
            id: uuid::Uuid::new_v4().to_string(),
            text: normalized,
            kind: classify(text).to_string(),
            created_at: now_millis(),
        });
        while inner.items.len() > MAX_ITEMS {
            inner.items.pop_back();
        }
        true
    }
}

/// CRLF / CR 统一成 LF，其它内容保持原样
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
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
    let indented = text.lines().any(|line| line.starts_with("  ") || line.starts_with('\t'));
    let mut hits = 0usize;
    for needle in [
        "=>", "->", "()", "{}", "};", "</", "/>", "#!/", "<?", "function ", "const ",
        "let ", "var ", "def ", "class ", "import ", "return ", "#include", "SELECT ",
        "INSERT ", "UPDATE ", "printf(", "console.", "public ", "private ", "async ",
        "await ", "fn ", "impl ",
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
    std::thread::Builder::new()
        .name("ws-clipboard-poller".to_string())
        .spawn(move || {
            // 启动时把当前剪贴板内容也收进来：它通常就是用户最近一次复制的东西
            if let Ok(text) = app.clipboard().read_text() {
                if let Some(history) = app.try_state::<ClipboardHistory>() {
                    if history.push(&text) {
                        let items = history.snapshot();
                        drop(history);
                        let _ = app.emit(EVENT_CHANGED, items);
                    }
                }
            }

            loop {
                std::thread::sleep(POLL_INTERVAL);
                // 应用正在退出时 try_state 返回 None
                let Some(history) = app.try_state::<ClipboardHistory>() else {
                    return;
                };
                let Ok(text) = app.clipboard().read_text() else {
                    continue;
                };
                if history.push(&text) {
                    let items = history.snapshot();
                    drop(history);
                    let _ = app.emit(EVENT_CHANGED, items);
                }
            }
        })
        .ok();
}

// ============================== 面板显示 ==============================

/// 呼出 / 收起面板。全局快捷键、面板里再按一次都走这里。
pub fn toggle_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

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

    let (x, y) = panel_origin_at(
        cursor,
        (PANEL_WIDTH as u32, PANEL_HEIGHT as u32),
        screen,
    );
    Some((x as f64, y as f64, true))
}

#[cfg(not(target_os = "macos"))]
fn panel_position(app: &AppHandle, window: &tauri::WebviewWindow) -> Option<(f64, f64, bool)> {
    let cursor = app.cursor_position().ok()?;
    let anchor = (cursor.x as i32, cursor.y as i32);
    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()?;
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

    fn history() -> ClipboardHistory {
        ClipboardHistory::new()
    }

    #[test]
    fn keeps_newest_first_and_dedupes() {
        let history = history();
        assert!(history.push("one"));
        assert!(history.push("two"));
        assert!(history.push("one"));
        let items = history.snapshot();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "one");
        assert_eq!(items[1].text, "two");
        // 重复内容不应被当成新事件反复推送
        assert!(!history.push("one"));
    }

    #[test]
    fn caps_at_twenty_items() {
        let history = history();
        for index in 0..(MAX_ITEMS + 5) {
            assert!(history.push(&format!("item-{index}")));
        }
        let items = history.snapshot();
        assert_eq!(items.len(), MAX_ITEMS);
        assert_eq!(items[0].text, format!("item-{}", MAX_ITEMS + 4));
        assert_eq!(items[MAX_ITEMS - 1].text, "item-5");
    }

    #[test]
    fn skips_blank_and_oversized_text() {
        let history = history();
        assert!(!history.push(""));
        assert!(!history.push("   \n\t "));
        let oversized = "x".repeat(MAX_TEXT_CHARS + 1);
        assert!(!history.push(&oversized));
        assert!(history.snapshot().is_empty());
    }

    #[test]
    fn normalizes_crlf() {
        let history = history();
        assert!(history.push("a\r\nb\rc"));
        assert_eq!(history.snapshot()[0].text, "a\nb\nc");
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
    fn panel_centers_on_cursor_and_clamps() {
        let panel = (400, 300);
        let screen = (0, 0, 1920, 1080);
        assert_eq!(panel_origin_at((800, 400), panel, screen), (600, 412));
        // 左边缘：夹到 margin
        assert_eq!(panel_origin_at((10, 400), panel, screen).0, 8);
        // 右边缘：夹进屏幕
        assert_eq!(panel_origin_at((1900, 400), panel, screen).0, 1920 - 400 - 8);
        // 底部：夹进屏幕
        assert_eq!(panel_origin_at((800, 1070), panel, screen).1, 1080 - 300 - 8);
        // 多显示器负坐标
        let left_screen = (-1920, 0, 1920, 1080);
        assert_eq!(panel_origin_at((-1000, 300), panel, left_screen), (-1200, 312));
    }

    #[test]
    fn panel_handles_screen_smaller_than_panel() {
        let (x, y) = panel_origin_at((50, 50), (800, 600), (0, 0, 400, 300));
        assert_eq!((x, y), (8, 8));
    }
}
