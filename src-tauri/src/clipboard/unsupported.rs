//! 非 macOS / Windows 平台的占位实现：剪贴板历史维持不启用（不采集、不注册热键、
//! 不建面板），只保留门面接口和平台无关的兜底实现。
//!
//! 平台实现文件：只在非 macOS / Windows target 编译，CI 不覆盖。

use tauri::{AppHandle, Manager};

use super::{panel_origin_at, PANEL_HEIGHT, PANEL_WIDTH};

/// 其它平台不采集剪贴板历史（本平台也没有 arboard 依赖）。
pub(super) fn capture_once(_app: &AppHandle, _force: bool) -> bool {
    false
}

/// 其它平台不读来源应用的私有表示。
pub(super) fn collect_extra_flavors() -> Vec<(String, Vec<u8>)> {
    Vec::new()
}

/// 当前系统剪贴板的变化序列号：其它平台一律返回 `None`，采集流程因此直接跳过。
pub(super) fn change_count() -> Option<i64> {
    None
}

/// 面板显示位置：返回 `(x, y, 是否逻辑点)`。
///
/// 其它平台用物理像素兜底；拿不到光标时返回 None，窗口保持原位置。
pub(super) fn panel_position(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
) -> Option<(f64, f64, bool)> {
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

/// 其它平台不需要激活应用（保持门面接口）。
pub(super) fn activate_app<R: tauri::Runtime>(_app: &AppHandle<R>) {}

/// 写回系统剪贴板：HTML（可选）+ 纯文本 + 图片（可选）。
///
/// 与 Windows 相同的 tauri 插件兜底：剪贴板格式是数字 ID，私有表示没法按名字还原，
/// 只用 HTML / 图片 / 文本。
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
