//! 「弹窗拦截」页签（仅 Windows）的命令入口。

use crate::cleaner::popup::{self, PopupHit, PopupSettings, PopupStatus};

#[tauri::command]
pub fn popup_status() -> PopupStatus {
    popup::status()
}

#[tauri::command]
pub fn popup_start(app: tauri::AppHandle) -> Result<PopupStatus, String> {
    popup::start(app)
}

#[tauri::command]
pub fn popup_stop() -> Result<PopupStatus, String> {
    popup::stop()
}

#[tauri::command]
pub fn popup_recent() -> Vec<PopupHit> {
    popup::recent()
}

#[tauri::command]
pub fn popup_clear() {
    popup::clear();
}

#[tauri::command]
pub fn popup_dismiss(id: u64, action: String) -> Result<(), String> {
    popup::dismiss(id, &action)
}

#[tauri::command]
pub fn popup_ignore(process: String, ignored: bool) -> Result<Vec<String>, String> {
    popup::ignore(&process, ignored)
}

/// 拦截模式设置（拦截开关 / 动作 / 阈值 / 每分钟上限）
#[tauri::command]
pub fn popup_settings_get() -> PopupSettings {
    popup::settings_get()
}

/// 保存拦截模式设置；后端会把越界值收进安全范围后返回最终值
#[tauri::command]
pub fn popup_settings_set(settings: PopupSettings) -> Result<PopupSettings, String> {
    popup::settings_set(settings)
}
