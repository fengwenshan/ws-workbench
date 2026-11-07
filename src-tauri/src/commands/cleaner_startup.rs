//! 「开机自启」页签的命令入口（设计 4.2）。
//!
//! 跟 `cleaner.rs` 同一套约定：枚举注册表 / 跑 `launchctl` / `osascript` 都是阻塞
//! 操作，统一 `spawn_blocking`，避免卡住 WebView 主线程。`startup_set_enabled`
//! 需要 `AppHandle` 只是为了解析备份目录（Windows 写状态位前要备份原值），
//! 不参与「能不能改」的判断 —— 那由 `cleaner::startup` 统一校验。

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::cleaner::startup::{self, StartupItem};

/// 可写的应用数据目录（Windows 启动项备份用）。
///
/// 顺序与 config/db 的存储目录一致：`app_data_dir` → `app_config_dir` →
/// `~/.ws-workbench`。返回 None 时 Windows 端会拒绝写状态位（宁可不改，
/// 也不留下回滚不了的状态）。
fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = app.path().app_data_dir() {
        candidates.push(dir);
    }
    if let Ok(dir) = app.path().app_config_dir() {
        candidates.push(dir);
    }
    if let Some(home) = crate::cleaner::home_dir() {
        candidates.push(home.join(".ws-workbench"));
    }
    candidates.into_iter().find(|dir| {
        if dir.is_dir() {
            return true;
        }
        crate::util::fs::create_dir_all(dir).is_ok()
    })
}

/// 列出开机启动项（Windows：注册表 + 启动文件夹；macOS：LaunchAgents + 登录项）
#[tauri::command]
pub async fn startup_list() -> Result<Vec<StartupItem>, String> {
    tauri::async_runtime::spawn_blocking(startup::list)
        .await
        .map_err(|err| format!("读取启动项任务失败: {}", err))?
}

/// 启用 / 禁用一条启动项；返回刷新后的完整列表
#[tauri::command]
pub async fn startup_set_enabled(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<Vec<StartupItem>, String> {
    let data_dir = data_dir(&app);
    tauri::async_runtime::spawn_blocking(move || {
        startup::set_enabled(&id, enabled, data_dir.as_deref())
    })
    .await
    .map_err(|err| format!("切换启动项任务失败: {}", err))?
}

/// 本应用是否设置为登录时启动
#[tauri::command]
pub fn startup_app_autostart_get() -> Result<bool, String> {
    startup::app_autostart_get()
}

/// 设置 / 取消本应用登录时启动
#[tauri::command]
pub async fn startup_app_autostart_set(enabled: bool) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || startup::app_autostart_set(enabled))
        .await
        .map_err(|err| format!("设置登录启动任务失败: {}", err))?
}
