//! 「屏幕休眠」页签的命令入口。

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
