//! Windows 剪贴板平台实现：`GetClipboardSequenceNumber` 变化序列号、Win32 面板
//! 定位与焦点（`GetForegroundWindow` / `SetForegroundWindow` / `GetWindowRect`）、
//! 写回剪贴板。
//!
//! 平台实现文件：只在 Windows target 编译，禁止引用 `clipboard::macos`。
//! 平台验证状态：本文件只在 Windows 上编译，Windows CI（test-windows）验证。

use tauri::{AppHandle, Manager};

use super::{panel_origin_at, PANEL_HEIGHT, PANEL_PREVIOUS_APP, PANEL_WIDTH};

// ============================== 额外表示 ==============================

/// Windows 不保留来源应用的私有表示：剪贴板格式是数字 ID，没法按名字还原，
/// 写回时只用 HTML / 图片 / 文本（见 `write_pasteboard`）。
pub(super) fn collect_extra_flavors() -> Vec<(String, Vec<u8>)> {
    Vec::new()
}

/// Windows：`GetClipboardSequenceNumber` 每次内容变化都会递增；
/// 返回 0 表示当前拿不到剪贴板（例如被别的进程独占），本次跳过。
pub(super) fn change_count() -> Option<i64> {
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    let sequence = unsafe { GetClipboardSequenceNumber() };
    (sequence != 0).then_some(sequence as i64)
}

/// 光标是否还停在面板窗口里（Windows）。
///
/// 拖标题栏走的是 Windows 的 SC_MOVE 模态循环，循环开始时会先给窗口发一次
/// `WM_ACTIVATE(WA_INACTIVE)`；失焦处理器如果无脑 `hide()`，拖拽会被当场掐断，
/// 用户看到的就是「点一下标题栏面板就没了」。鼠标还在面板里就当作「在拖自己」，
/// 真正点外面时鼠标已经不在面板矩形内。
// `clipboard/mod.rs` 把它按 `pub(crate)` 再导出给 `lib.rs` 的失焦处理器，
// 所以这里必须也是 `pub(crate)`：只写 `pub(super)` 会在 Windows 上编译不过
// （E0364：private item cannot be re-exported）。
pub(crate) fn cursor_inside_hwnd(raw: isize) -> bool {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};

    let mut point = POINT::default();
    let mut rect = RECT::default();
    let ok = unsafe {
        GetCursorPos(&mut point).is_ok() && GetWindowRect(HWND(raw as *mut _), &mut rect).is_ok()
    };
    ok && point.x >= rect.left
        && point.x < rect.right
        && point.y >= rect.top
        && point.y < rect.bottom
}

/// 面板显示位置：返回 `(x, y, 是否逻辑点)`。
///
/// Windows 用物理像素；优先用系统算好的工作区（已扣掉任务栏），否则在屏幕底部
/// 呼出时面板底边会落到任务栏后面被压住；拿不到光标时返回 None，窗口保持原位置。
pub(super) fn panel_position(app: &AppHandle, window: &tauri::WebviewWindow) -> Option<(f64, f64, bool)> {
    let cursor = app.cursor_position().ok()?;
    let anchor = (cursor.x as i32, cursor.y as i32);
    // Windows：先用系统算好的工作区（已扣掉任务栏），否则在屏幕底部呼出时
    // 面板底边会落到任务栏后面被压住；拿不到工作区再退回整块显示器。
    let work_area = crate::work_area_at(anchor.0, anchor.1);
    let screen = match work_area {
        Some(area) => area,
        None => {
            let monitor = app.monitor_from_point(cursor.x, cursor.y).ok().flatten()?;
            let position = monitor.position();
            let size = monitor.size();
            (
                position.x,
                position.y,
                size.width as i32,
                size.height as i32,
            )
        }
    };
    let scale = window.scale_factor().ok()?;
    let panel = (
        (PANEL_WIDTH * scale).round() as u32,
        (PANEL_HEIGHT * scale).round() as u32,
    );
    let (x, y) = panel_origin_at(anchor, panel, screen);
    Some((x as f64, y as f64, false))
}

/// Windows：呼出面板时由系统激活应用，不需要额外处理（保持门面接口）。
pub(super) fn activate_app<R: tauri::Runtime>(_app: &AppHandle<R>) {}

/// Windows：记录呼出面板前最前面的窗口（HWND），自己家的窗口不算。
pub(super) fn remember_frontmost_app() {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        *PANEL_PREVIOUS_APP.lock().unwrap() = None;
        return;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    *PANEL_PREVIOUS_APP.lock().unwrap() = (pid != std::process::id()).then_some(hwnd.0 as isize);
}

/// Windows：把前台窗口还给呼出面板前的应用，用户直接按 Ctrl+V 就能粘贴。
pub(super) fn restore_frontmost_app(_app: &AppHandle) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

    let Some(raw) = PANEL_PREVIOUS_APP.lock().unwrap().take() else {
        return;
    };
    // 前台窗口句柄只是拿来还原焦点，不是借用，转成裸指针交回系统
    unsafe {
        let _ = SetForegroundWindow(HWND(raw as *mut core::ffi::c_void));
    }
}

/// Windows：呼出面板会把整个应用带到前台，主窗口可能跟着顶上来。
/// 和 macOS 的 `orderBack` 对应：从别的应用呼出时把主窗口压到所有窗口后面。
pub(super) fn send_main_window_to_back(app: &AppHandle) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    // 从主窗口自己呼出时保持原样，只有「从别的 App 呼出」才压下去
    if PANEL_PREVIOUS_APP.lock().unwrap().is_none() {
        return;
    }
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    if !main.is_visible().unwrap_or(false) {
        return;
    }
    let Ok(hwnd) = main.hwnd() else {
        return;
    };
    // tauri 用的是另一个 minor 版本的 windows crate，HWND 都是裸指针包一层，
    // 取指针转成本 crate 依赖的句柄类型即可（ABI 一致）
    let hwnd = windows::Win32::Foundation::HWND(hwnd.0 as *mut core::ffi::c_void);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

/// 写回系统剪贴板：HTML（可选）+ 纯文本 + 图片（可选）。
///
/// Windows 的剪贴板格式是数字 ID，私有表示没法按名字还原，只用 HTML / 图片 / 文本。
pub(super) fn write_pasteboard(
    app: &AppHandle,
    html: Option<&str>,
    text: &str,
    png: Option<Vec<u8>>,
    extras: &[(String, Vec<u8>)],
) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    // Windows 的剪贴板格式是数字 ID，私有表示没法按名字还原，只用 HTML / 图片 / 文本
    let _ = extras;
    if let Some(html) = html {
        return app
            .clipboard()
            .write_html(html.to_string(), Some(text.to_string()))
            .map_err(|e| format!("写回剪贴板失败: {e}"));
    }
    if let Some(png) = png {
        let image = image::load_from_memory(&png).map_err(|e| format!("图片解码失败: {e}"))?;
        let rgba = image.to_rgba8();
        let image =
            tauri::image::Image::new_owned(rgba.as_raw().clone(), rgba.width(), rgba.height());
        return app
            .clipboard()
            .write_image(&image)
            .map_err(|e| format!("写回剪贴板失败: {e}"));
    }
    app.clipboard()
        .write_text(text.to_string())
        .map_err(|e| format!("写回剪贴板失败: {e}"))
}
