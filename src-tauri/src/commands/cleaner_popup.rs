//! 「弹窗拦截」页签（仅 Windows）的命令入口。

use crate::cleaner::popup::{self, PopupHit, PopupStatus};

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
