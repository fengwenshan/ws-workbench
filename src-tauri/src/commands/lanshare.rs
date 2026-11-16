//! 局域网分享命令（设计文档 4.1）。
//!
//! 命令层只做参数转交：运行期状态都在 `crate::lanshare::LanShareState` 里，
//! 事件由后台线程推给前端。

use tauri::{AppHandle, Manager};

use crate::lanshare::{LanShareSnapshot, LanShareState};

fn state(app: &AppHandle) -> Result<tauri::State<'_, LanShareState>, String> {
    app.try_state::<LanShareState>()
        .ok_or_else(|| "局域网分享状态未初始化".to_string())
}

/// 页面初始化 / 手动刷新
#[tauri::command]
pub fn lan_share_snapshot(app: AppHandle) -> Result<LanShareSnapshot, String> {
    Ok(state(&app)?.snapshot())
}

/// 开启 / 关闭功能；启动失败把原因放进快照的 `last_error`
#[tauri::command]
pub fn lan_share_set_enabled(app: AppHandle, enabled: bool) -> Result<LanShareSnapshot, String> {
    Ok(state(&app)?.set_enabled(&app, enabled))
}

/// 修改设备名（空串回退主机名）
#[tauri::command]
pub fn lan_share_set_alias(app: AppHandle, alias: String) -> Result<LanShareSnapshot, String> {
    state(&app)?.set_alias(&app, &alias)
}

/// 指定接收目录（`null` / 空串回退默认目录）
#[tauri::command]
pub fn lan_share_set_download_dir(
    app: AppHandle,
    dir: Option<String>,
) -> Result<LanShareSnapshot, String> {
    state(&app)?.set_download_dir(&app, dir)
}

/// 接受 / 拒绝待接收请求
#[tauri::command]
pub fn lan_share_respond_offer(
    app: AppHandle,
    offer_id: String,
    accept: bool,
) -> Result<(), String> {
    state(&app)?.respond_offer(&app, &offer_id, accept)
}

/// 向指定设备发送文件；立即返回 transfer_id，进度走 `lan-share://send-progress` 事件
#[tauri::command]
pub fn lan_share_send_files(
    app: AppHandle,
    peer_fingerprint: String,
    paths: Vec<String>,
) -> Result<String, String> {
    state(&app)?.send_files(&app, &peer_fingerprint, paths)
}
