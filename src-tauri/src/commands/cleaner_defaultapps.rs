//! 「默认应用」页签的 Tauri 命令层（设计 4.4）：只做参数 / 错误翻译。
//!
//! 读 LaunchServices / 注册表、扫 `/Applications`、起系统对话框都是阻塞操作，
//! 统一 `spawn_blocking`，避免卡住 WebView 主线程（与 `commands/cleaner.rs` 一致）。

use crate::cleaner::defaultapps::{self, DefaultAppCandidate, DefaultAppEntry};

/// 列出首批扩展名的当前默认应用
#[tauri::command]
pub async fn defaultapps_list() -> Result<Vec<DefaultAppEntry>, String> {
    tauri::async_runtime::spawn_blocking(defaultapps::list)
        .await
        .map_err(|error| format!("读取默认应用失败: {}", error))?
}

/// 列出能打开该扩展名的候选应用（macOS 真实返回；Windows 为空表）
#[tauri::command]
pub async fn defaultapps_candidates(
    extension: String,
) -> Result<Vec<DefaultAppCandidate>, String> {
    tauri::async_runtime::spawn_blocking(move || defaultapps::candidates(&extension))
        .await
        .map_err(|error| format!("读取候选应用失败: {}", error))?
}

/// 修改默认应用（macOS LaunchServices；Windows 返回「需在系统设置里确认」的错误）
#[tauri::command]
pub async fn defaultapps_set(extension: String, app_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || defaultapps::set(&extension, &app_id))
        .await
        .map_err(|error| format!("修改默认应用失败: {}", error))?
}

/// 打开系统的「默认应用」设置页
#[tauri::command]
pub async fn defaultapps_reveal_settings() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(defaultapps::reveal_settings)
        .await
        .map_err(|error| format!("打开系统设置失败: {}", error))?
}

/// 调起系统的「打开方式」对话框（Windows 专用；macOS 返回提示）
#[tauri::command]
pub async fn defaultapps_open_with(extension: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || defaultapps::open_with(&extension))
        .await
        .map_err(|error| format!("打开方式对话框失败: {}", error))?
}
