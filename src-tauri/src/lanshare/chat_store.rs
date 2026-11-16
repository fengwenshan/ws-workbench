//! 局域网聊天的本地存储（SQLite，schema v4）。
//!
//! 设计文档：`docs/superpowers/specs/2026-11-16-lan-share-chat-redesign.md` §4.6。
//!
//! 约定：
//! - 会话 / 消息正文、文件名、会话预览都走 `crate::crypto` 的信封加密后落库，
//!   和剪贴板一致；解密失败时跳过坏数据，不让整页报错。
//! - 每次操作自己 `crate::db::open` 一个连接（WAL + busy_timeout 已有），
//!   调用方保证不在 `Inner` 的锁里做 DB IO。
//! - 时间统一 Unix 毫秒（与 `lanshare::unix_millis` 一致）。
//!
//! 本文件由 backend-chat-store 负责实现，接口签名见设计文档 §4.6，不要改签名。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::crypto::VaultKeys;

/// 聊天记录里的一个文件（发送 / 接收各一条消息）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: String,
    pub peer_id: String,
    /// `in`（收到）/ `out`（发出）
    pub direction: String,
    /// `text` / `file`
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_path: Option<String>,
    /// `sending` / `sent` / `failed` / `received`
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    /// 发送中的文件消息与 `lan-share://send-progress` 事件对应
    #[serde(default)]
    pub transfer_id: Option<String>,
    #[serde(default)]
    pub file_index: Option<usize>,
    #[serde(default)]
    pub file_total: Option<usize>,
    pub created_at_ms: i64,
}

impl ChatMessage {
    /// 文本消息（direction：`in` / `out`）
    pub fn text(
        id: impl Into<String>,
        peer_id: impl Into<String>,
        direction: impl Into<String>,
        text: impl Into<String>,
        created_at_ms: i64,
    ) -> Self {
        Self {
            id: id.into(),
            peer_id: peer_id.into(),
            direction: direction.into(),
            kind: "text".to_string(),
            text: text.into(),
            file_name: None,
            file_size: None,
            file_path: None,
            status: "sent".to_string(),
            error: None,
            transfer_id: None,
            file_index: None,
            file_total: None,
            created_at_ms,
        }
    }

    /// 文件消息
    #[allow(clippy::too_many_arguments)]
    pub fn file(
        id: impl Into<String>,
        peer_id: impl Into<String>,
        direction: impl Into<String>,
        file_name: impl Into<String>,
        file_size: u64,
        status: impl Into<String>,
        created_at_ms: i64,
    ) -> Self {
        Self {
            id: id.into(),
            peer_id: peer_id.into(),
            direction: direction.into(),
            kind: "file".to_string(),
            text: String::new(),
            file_name: Some(file_name.into()),
            file_size: Some(file_size),
            file_path: None,
            status: status.into(),
            error: None,
            transfer_id: None,
            file_index: None,
            file_total: None,
            created_at_ms,
        }
    }
}

/// 会话摘要（列表项）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatConversation {
    /// 对端设备身份（`fingerprint`，即主网卡 MAC）
    pub id: String,
    pub alias: String,
    pub last_activity_ms: i64,
    /// 最后一条消息预览（已解密；文件消息形如 `[文件] a.txt`）
    pub last_preview: String,
    pub unread: i64,
    pub created_at_ms: i64,
}

/// 聊天记录存储句柄：只持有数据库路径与密钥，操作时临时开连接。
#[derive(Clone)]
pub(crate) struct ChatStore {
    #[allow(dead_code)]
    path: PathBuf,
    #[allow(dead_code)]
    keys: VaultKeys,
}

impl ChatStore {
    /// 生产入口：解析数据库路径 + vault 密钥，并跑一次 open 确保 schema v4 迁移完成。
    pub fn open(_app: &AppHandle) -> Result<Self, String> {
        todo!("backend-chat-store: ChatStore::open")
    }

    /// 测试入口
    pub fn new(path: PathBuf, keys: VaultKeys) -> Self {
        Self { path, keys }
    }

    /// 会话列表（按最后活动时间倒序）
    pub fn list_conversations(&self) -> Result<Vec<ChatConversation>, String> {
        todo!("backend-chat-store: list_conversations")
    }

    /// 单个会话（不存在返回 None）
    pub fn get_conversation(&self, _peer_id: &str) -> Result<Option<ChatConversation>, String> {
        todo!("backend-chat-store: get_conversation")
    }

    /// 确保会话存在 / 更新别名与最后活动时间（不新增消息、不改未读）
    pub fn upsert_conversation(
        &self,
        _peer_id: &str,
        _alias: &str,
        _activity_ms: i64,
    ) -> Result<(), String> {
        todo!("backend-chat-store: upsert_conversation")
    }

    /// 追加一条消息并同步更新会话（预览 / 活动时间 / 未读）。
    ///
    /// `direction == "in"` 时未读 +1；`direction == "out"` 不加。
    pub fn append_message(
        &self,
        _peer_id: &str,
        _alias: &str,
        _message: &ChatMessage,
    ) -> Result<(), String> {
        todo!("backend-chat-store: append_message")
    }

    /// 最近 `limit` 条消息，按时间正序返回
    pub fn list_messages(
        &self,
        _peer_id: &str,
        _limit: usize,
    ) -> Result<Vec<ChatMessage>, String> {
        todo!("backend-chat-store: list_messages")
    }

    /// 更新消息状态（可同时写失败原因 / 落盘路径）
    pub fn update_message_status(
        &self,
        _peer_id: &str,
        _message_id: &str,
        _status: &str,
        _error: Option<&str>,
        _file_path: Option<&str>,
    ) -> Result<(), String> {
        todo!("backend-chat-store: update_message_status")
    }

    /// 清空未读
    pub fn mark_read(&self, _peer_id: &str) -> Result<(), String> {
        todo!("backend-chat-store: mark_read")
    }

    /// 删除会话及其全部消息（仅本机）
    pub fn delete_conversation(&self, _peer_id: &str) -> Result<(), String> {
        todo!("backend-chat-store: delete_conversation")
    }
}
