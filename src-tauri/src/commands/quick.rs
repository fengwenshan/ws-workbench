//! 托盘快捷面板的窗口控制。
//!
//! 面板是独立的无边框窗口（label = `quick-panel`），由 `lib.rs` 在启动时创建并隐藏。
//! 前端只调这里的命令，不直接碰窗口 API —— 这样不必为了 hide/show 去开一堆
//! `core:window` 权限，面板与主窗口的行为也集中在一处。

use tauri::{AppHandle, Manager};

use crate::{
    panel_origin_at, QUICK_PANEL_ANCHOR, QUICK_PANEL_LABEL, QUICK_PANEL_MAX_HEIGHT,
    QUICK_PANEL_MIN_HEIGHT, QUICK_PANEL_WIDTH,
};

/// 隐藏面板（点面板外部失焦、再点一次托盘图标时都会走到这里）
#[tauri::command]
pub fn quick_panel_hide(app: AppHandle) {
    if let Some(window) = app.get_webview_window(QUICK_PANEL_LABEL) {
        let _ = window.hide();
    }
}

/// 按当前页签的内容高度调整面板窗口。
///
/// 面板打开时 VPN / 休眠两个页签的高度不同（提示条出现/消失也会变），窗口
/// 跟着缩到合适高度，避免固定高度下面板底部大片空白。
///
/// 菜单栏 / 顶部任务栏下面板朝下展开：`set_size` 本身就会保住左上角，顶边
/// 不用动（用户拖动过面板也保持原地）；只有桌面底部任务栏那种朝上展开的
/// 情况，才按上次弹出时的锚点重新算 y，把底边固定在任务栏上方。
#[tauri::command]
pub fn quick_panel_resize(app: AppHandle, height: f64) {
    let Some(window) = app.get_webview_window(QUICK_PANEL_LABEL) else {
        return;
    };
    let height = height.clamp(QUICK_PANEL_MIN_HEIGHT, QUICK_PANEL_MAX_HEIGHT);
    #[cfg(debug_assertions)]
    eprintln!("[quick-panel] resize height={height}");
    if window
        .set_size(tauri::LogicalSize::new(QUICK_PANEL_WIDTH, height))
        .is_err()
    {
        return;
    }

    // 面板还没弹出（启动时前端会先量一次高度）就没有锚点，只需改大小；
    // 朝下展开时也只需要改大小，顶边自然不动。
    let anchor = QUICK_PANEL_ANCHOR.lock().ok().and_then(|guard| *guard);
    let Some(anchor) = anchor else {
        return;
    };
    if anchor.expands_below() {
        return;
    }

    let panel = if anchor.logical {
        // macOS：panel_origin_at 用逻辑点
        (QUICK_PANEL_WIDTH.round() as u32, height.round() as u32)
    } else {
        // Windows / 兜底：panel_origin_at 用物理像素
        let scale = window.scale_factor().unwrap_or(1.0);
        (
            (QUICK_PANEL_WIDTH * scale).round() as u32,
            (height * scale).round() as u32,
        )
    };
    let (x, y) = panel_origin_at(anchor.anchor, panel, anchor.screen);
    if anchor.logical {
        let _ = window.set_position(tauri::LogicalPosition::new(x as f64, y as f64));
    } else {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}
