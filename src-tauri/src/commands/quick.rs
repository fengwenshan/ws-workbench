//! 托盘快捷面板的窗口控制。
//!
//! 面板是独立的无边框窗口（label = `quick-panel`），由 `lib.rs` 在启动时创建并隐藏。
//! 前端只调这里的命令，不直接碰窗口 API —— 这样不必为了 hide/show 去开一堆
//! `core:window` 权限，面板与主窗口的行为也集中在一处。

use tauri::{AppHandle, Emitter, Manager};

use crate::EVENT_NAV_TRAY;

/// 隐藏面板（点「隐藏」、按 Esc、点面板外部时都会走到这里）
#[tauri::command]
pub fn quick_panel_hide(app: AppHandle) {
    if let Some(window) = app.get_webview_window(crate::QUICK_PANEL_LABEL) {
        let _ = window.hide();
    }
}

/// 打开主窗口并切到指定页面。
///
/// `target` 取 `cleaner` / `proxy` / `settings`，复用托盘菜单的 `nav-*` 事件，
/// 主窗口的监听逻辑不用为面板再写一份。
#[tauri::command]
pub fn quick_panel_open_main(app: AppHandle, target: Option<String>) {
    crate::show_main_window(&app);
    if let Some(target) = target {
        let _ = app.emit(EVENT_NAV_TRAY, format!("nav-{}", target));
    }
}
