//! 「屏幕休眠」页签的命令入口。

use crate::cleaner::sleep::triggers::{self, TriggerKind, TriggersState};
use crate::cleaner::sleep::{self, KeepAwakeStatus};

#[tauri::command]
pub fn keepawake_status() -> KeepAwakeStatus {
    sleep::status()
}

#[tauri::command]
pub fn keepawake_start(
    duration_seconds: Option<u64>,
    allow_display_sleep: bool,
) -> Result<KeepAwakeStatus, String> {
    sleep::start(duration_seconds, allow_display_sleep)
}

#[tauri::command]
pub fn keepawake_stop() -> Result<KeepAwakeStatus, String> {
    sleep::stop()
}

// ============================== 自动保持规则 ==============================

// 规则状态里有进程枚举 + `pmset` 查询，都是阻塞操作，统一 spawn_blocking，
// 避免卡住 WebView 主线程（和 startup 的命令同一约定）。

#[tauri::command]
pub async fn sleep_triggers_state() -> Result<TriggersState, String> {
    tauri::async_runtime::spawn_blocking(triggers::state)
        .await
        .map_err(|err| format!("读取自动保持规则失败: {}", err))
}

#[tauri::command]
pub async fn sleep_triggers_set_armed(
    app: tauri::AppHandle,
    armed: bool,
) -> Result<TriggersState, String> {
    tauri::async_runtime::spawn_blocking(move || triggers::set_armed(app, armed))
        .await
        .map_err(|err| format!("切换自动保持任务失败: {}", err))?
}

#[tauri::command]
pub async fn sleep_triggers_add(
    app: tauri::AppHandle,
    kind: TriggerKind,
    value: String,
) -> Result<TriggersState, String> {
    tauri::async_runtime::spawn_blocking(move || triggers::add(app, kind, &value))
        .await
        .map_err(|err| format!("新增自动规则任务失败: {}", err))?
}

#[tauri::command]
pub async fn sleep_triggers_remove(
    app: tauri::AppHandle,
    id: String,
) -> Result<TriggersState, String> {
    tauri::async_runtime::spawn_blocking(move || triggers::remove(app, &id))
        .await
        .map_err(|err| format!("删除自动规则任务失败: {}", err))?
}

#[tauri::command]
pub async fn sleep_triggers_set_enabled(
    app: tauri::AppHandle,
    id: String,
    enabled: bool,
) -> Result<TriggersState, String> {
    tauri::async_runtime::spawn_blocking(move || triggers::set_enabled(app, &id, enabled))
        .await
        .map_err(|err| format!("更新自动规则任务失败: {}", err))?
}
