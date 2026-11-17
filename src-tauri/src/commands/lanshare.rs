//! 局域网分享命令（设计文档 4.1）。
//!
//! 命令层只做参数转交：运行期状态都在 `crate::lanshare::LanShareState` 里，
//! 事件由后台线程推给前端。

use tauri::{AppHandle, Manager};

use crate::lanshare::{ChatMessage, LanShareConversation, LanShareSnapshot, LanShareState};

fn state(app: &AppHandle) -> Result<tauri::State<'_, LanShareState>, String> {
    app.try_state::<LanShareState>()
        .ok_or_else(|| "局域网分享状态未初始化".to_string())
}

/// 页面初始化 / 手动刷新
#[tauri::command]
pub fn lan_share_snapshot(app: AppHandle) -> Result<LanShareSnapshot, String> {
    Ok(state(&app)?.snapshot())
}

/// 未读总数（首次加载用；后续变化走 `lan-share://unread` 事件订阅）
#[tauri::command]
pub fn lan_share_unread_total(app: AppHandle) -> Result<u32, String> {
    Ok(state(&app)?.total_unread())
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

/// 聊天列表：持久化会话 + 运行期在线状态（左侧机器列表）
#[tauri::command]
pub fn lan_share_conversations(app: AppHandle) -> Result<Vec<LanShareConversation>, String> {
    state(&app)?.conversations()
}

/// 某个会话的历史消息（时间正序，最多 200 条）
#[tauri::command]
pub fn lan_share_messages(app: AppHandle, peer_id: String) -> Result<Vec<ChatMessage>, String> {
    state(&app)?.messages(&peer_id)
}

/// 发送一条文本消息：本地先落库并返回，投递在后台线程，结果由事件推送
#[tauri::command]
pub fn lan_share_send_chat(
    app: AppHandle,
    peer_id: String,
    text: String,
) -> Result<ChatMessage, String> {
    state(&app)?.send_chat(&app, &peer_id, &text)
}

/// 清空某个会话的未读
#[tauri::command]
pub fn lan_share_mark_read(app: AppHandle, peer_id: String) -> Result<(), String> {
    state(&app)?.mark_read(&app, &peer_id)
}

/// 删除本机会话与聊天记录（对方不受影响）；`delete_files` 时同时删除本机收到的文件。
#[tauri::command]
pub fn lan_share_delete_conversation(
    app: AppHandle,
    peer_id: String,
    delete_files: bool,
) -> Result<(), String> {
    state(&app)?.delete_conversation(&app, &peer_id, delete_files)
}

/// 删除单条消息（仅本机聊天记录；对方设备不受影响，本机已收到的文件不删除）。
#[tauri::command]
pub fn lan_share_delete_message(
    app: AppHandle,
    peer_id: String,
    message_id: String,
) -> Result<(), String> {
    state(&app)?.delete_message(&app, &peer_id, &message_id)
}

/// 该会话里本机收到的、还带落盘路径的文件数量
#[tauri::command]
pub fn lan_share_conversation_attachment_count(
    app: AppHandle,
    peer_id: String,
) -> Result<u32, String> {
    state(&app)?.attachment_count(&peer_id)
}

/// 删除本机收到的文件但保留聊天记录；返回实际删除的文件数。
#[tauri::command]
pub fn lan_share_delete_attachments(app: AppHandle, peer_id: String) -> Result<u32, String> {
    state(&app)?.delete_attachments(&app, &peer_id)
}

/// 读系统剪贴板：文件列表优先，其次图片；返回暂存后的本地路径。
#[tauri::command]
pub fn lan_share_import_clipboard_files(app: AppHandle) -> Result<Vec<String>, String> {
    crate::lanshare::outbox::clipboard_paths(&app)
}

/// 拖拽文件（只有字节没有路径）：落暂存目录后走常规文件发送。
#[tauri::command]
pub fn lan_share_send_dropped_files(
    app: AppHandle,
    peer_id: String,
    files: Vec<crate::lanshare::outbox::DroppedFileInput>,
) -> Result<String, String> {
    state(&app)?.send_dropped_files(&app, &peer_id, &files)
}

/// 设置 / 清除本机给某台设备起的备注（只在本机显示，不广播给对端）。
/// `remark` 传空串表示删除备注。
#[tauri::command]
pub fn lan_share_set_peer_remark(
    app: AppHandle,
    peer_id: String,
    remark: String,
) -> Result<LanShareSnapshot, String> {
    state(&app)?.set_peer_remark(&app, &peer_id, &remark)
}

/// 打开文件所在目录（接收到的文件、发送的源文件都可以用）。
#[tauri::command]
pub fn lan_share_open_file_dir(path: String) -> Result<(), String> {
    let parent = parent_dir(&path)?;
    super::system::open_dir(parent.to_string_lossy().to_string())?;
    Ok(())
}

/// 取文件所在目录；相对路径没有父目录时回退到当前目录。
fn parent_dir(path: &str) -> Result<std::path::PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("缺少文件路径".to_string());
    }
    let parent = std::path::Path::new(path)
        .parent()
        .ok_or_else(|| format!("无法定位文件所在目录：{path}"))?;
    if parent.as_os_str().is_empty() {
        Ok(std::path::PathBuf::from("."))
    } else {
        Ok(parent.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_resolves_absolute_and_relative() {
        assert_eq!(
            parent_dir("/tmp/a/b.txt").unwrap(),
            std::path::PathBuf::from("/tmp/a")
        );
        assert_eq!(
            parent_dir("relative.txt").unwrap(),
            std::path::PathBuf::from(".")
        );
        assert!(parent_dir("   ").is_err());
    }
}
