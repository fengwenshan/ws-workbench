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

#[tauri::command]
pub fn sleep_triggers_state() -> TriggersState {
    triggers::state()
}

#[tauri::command]
pub fn sleep_triggers_set_armed(
    app: tauri::AppHandle,
    armed: bool,
) -> Result<TriggersState, String> {
    triggers::set_armed(app, armed)
}

#[tauri::command]
pub fn sleep_triggers_add(
    app: tauri::AppHandle,
    kind: TriggerKind,
    value: String,
) -> Result<TriggersState, String> {
    triggers::add(app, kind, &value)
}

#[tauri::command]
pub fn sleep_triggers_remove(app: tauri::AppHandle, id: String) -> Result<TriggersState, String> {
    triggers::remove(app, &id)
}

#[tauri::command]
pub fn sleep_triggers_set_enabled(
    app: tauri::AppHandle,
    id: String,
    enabled: bool,
) -> Result<TriggersState, String> {
    triggers::set_enabled(app, &id, enabled)
}
