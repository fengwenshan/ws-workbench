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

use rusqlite::{Connection, OptionalExtension};
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

/// 会话预览最多保留的字符数（按字符，不按字节；避免中文被截断成半个）
const PREVIEW_MAX_CHARS: usize = 80;

/// 消息里需要加密保存的部分：除主键 / 会话 / 方向 / 类型 / 时间外全在里面。
#[derive(Debug, Serialize, Deserialize)]
struct MessagePayload {
    #[serde(default)]
    text: String,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    file_size: Option<u64>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    transfer_id: Option<String>,
    #[serde(default)]
    file_index: Option<usize>,
    #[serde(default)]
    file_total: Option<usize>,
}

impl MessagePayload {
    fn from_message(message: &ChatMessage) -> Self {
        Self {
            text: message.text.clone(),
            file_name: message.file_name.clone(),
            file_size: message.file_size,
            file_path: message.file_path.clone(),
            status: message.status.clone(),
            error: message.error.clone(),
            transfer_id: message.transfer_id.clone(),
            file_index: message.file_index,
            file_total: message.file_total,
        }
    }

    fn into_message(self, id: String, peer_id: &str, direction: String, kind: String, created_at_ms: i64) -> ChatMessage {
        ChatMessage {
            id,
            peer_id: peer_id.to_string(),
            direction,
            kind,
            text: self.text,
            file_name: self.file_name,
            file_size: self.file_size,
            file_path: self.file_path,
            status: self.status,
            error: self.error,
            transfer_id: self.transfer_id,
            file_index: self.file_index,
            file_total: self.file_total,
            created_at_ms,
        }
    }
}

/// 会话表里的一行（预览仍是密文，需要解密）
struct ConversationRow {
    id: String,
    alias: String,
    last_activity_ms: i64,
    last_preview_enc: String,
    unread: i64,
    created_at_ms: i64,
}

impl ConversationRow {
    /// 解密预览；单条坏数据（换机器 / 密钥丢失）只告警并显示空预览，不让列表整体失败。
    fn into_conversation(self, store: &ChatStore) -> ChatConversation {
        let last_preview = match store.unseal(&self.last_preview_enc) {
            Ok(preview) => preview,
            Err(err) => {
                eprintln!(
                    "[lanshare] 会话 {} 的预览解密失败，已显示为空白: {}",
                    self.id, err
                );
                String::new()
            }
        };
        ChatConversation {
            id: self.id,
            alias: self.alias,
            last_activity_ms: self.last_activity_ms,
            last_preview,
            unread: self.unread,
            created_at_ms: self.created_at_ms,
        }
    }
}

/// 消息表里的一行（载荷仍是密文）
struct MessageRow {
    id: String,
    direction: String,
    kind: String,
    created_at_ms: i64,
    payload_enc: String,
}

impl MessageRow {
    fn into_message(self, peer_id: &str, store: &ChatStore) -> Result<ChatMessage, String> {
        let json = store.unseal(&self.payload_enc)?;
        let payload: MessagePayload =
            serde_json::from_str(&json).map_err(|e| format!("解析消息载荷失败: {}", e))?;
        Ok(payload.into_message(
            self.id,
            peer_id,
            self.direction,
            self.kind,
            self.created_at_ms,
        ))
    }
}

/// 文本按字符截断（预览用）
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// 文件消息预览：`[文件] 文件名`（文件名为空时只留 `[文件]`）
fn file_preview(file_name: Option<&str>) -> String {
    match file_name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("[文件] {}", name),
        None => "[文件]".to_string(),
    }
}

/// 会话预览规则：文本取正文（超长截断），文件取 `[文件] 文件名`，其它为空串。
fn preview_of(kind: &str, text: &str, file_name: Option<&str>) -> String {
    match kind {
        "text" => truncate_chars(text, PREVIEW_MAX_CHARS),
        "file" => file_preview(file_name),
        _ => String::new(),
    }
}

/// 聊天记录存储句柄：只持有数据库路径与密钥，操作时临时开连接。
#[derive(Clone)]
pub(crate) struct ChatStore {
    path: PathBuf,
    keys: VaultKeys,
}

impl ChatStore {
    /// 生产入口：解析数据库路径 + vault 密钥，并跑一次 open 确保 schema v4 迁移完成。
    pub fn open(app: &AppHandle) -> Result<Self, String> {
        let path = crate::db::db_path(app)?;
        let keys = crate::crypto::vault_keys(app)?;
        // 先建/迁一次库：v3 及更早的库在这里升到 v4，同时验证库可写；
        // 失败由调用方降级成 snapshot.warning，不影响局域网分享的其它功能。
        crate::db::open(&path)?;
        Ok(Self { path, keys })
    }

    /// 测试入口
    #[cfg(test)]
    pub fn new(path: PathBuf, keys: VaultKeys) -> Self {
        Self { path, keys }
    }

    /// 每次操作新开连接（WAL + busy_timeout 由 `db::open` 统一设置）
    fn connect(&self) -> Result<Connection, String> {
        crate::db::open(&self.path)
    }

    /// 加密成 `v1.` 信封
    fn seal(&self, plain: &str) -> Result<String, String> {
        crate::crypto::encrypt_with_keys(&self.keys, plain)
    }

    /// 解密 `v1.` 信封
    fn unseal(&self, envelope: &str) -> Result<String, String> {
        crate::crypto::decrypt_with_keys(&self.keys, envelope)
    }

    /// 会话列表（按最后活动时间倒序）
    pub fn list_conversations(&self) -> Result<Vec<ChatConversation>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, alias, last_activity_ms, last_preview_enc, unread, created_at_ms \
                 FROM lan_share_conversations ORDER BY last_activity_ms DESC, id ASC",
            )
            .map_err(|e| format!("查询会话失败: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ConversationRow {
                    id: row.get(0)?,
                    alias: row.get(1)?,
                    last_activity_ms: row.get(2)?,
                    last_preview_enc: row.get(3)?,
                    unread: row.get(4)?,
                    created_at_ms: row.get(5)?,
                })
            })
            .map_err(|e| format!("查询会话失败: {}", e))?;
        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(|e| format!("读取会话失败: {}", e))?;
            out.push(row.into_conversation(self));
        }
        Ok(out)
    }

    /// 单个会话（不存在返回 None）
    // 目前只有测试用到；保留成公开接口，方便后续「会话详情」类功能直接复用
    #[allow(dead_code)]
    pub fn get_conversation(&self, peer_id: &str) -> Result<Option<ChatConversation>, String> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT id, alias, last_activity_ms, last_preview_enc, unread, created_at_ms \
                 FROM lan_share_conversations WHERE id = ?1",
                rusqlite::params![peer_id],
                |row| {
                    Ok(ConversationRow {
                        id: row.get(0)?,
                        alias: row.get(1)?,
                        last_activity_ms: row.get(2)?,
                        last_preview_enc: row.get(3)?,
                        unread: row.get(4)?,
                        created_at_ms: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("查询会话失败: {}", e))?;
        Ok(row.map(|row| row.into_conversation(self)))
    }

    /// 确保会话存在 / 更新别名与最后活动时间（不新增消息、不改未读）
    pub fn upsert_conversation(
        &self,
        peer_id: &str,
        alias: &str,
        activity_ms: i64,
    ) -> Result<(), String> {
        let empty_preview_enc = self.seal("")?;
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO lan_share_conversations \
               (id, alias, last_activity_ms, last_preview_enc, unread, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, 0, ?3) \
             ON CONFLICT(id) DO UPDATE SET \
               alias = CASE WHEN excluded.alias <> '' THEN excluded.alias \
                            ELSE lan_share_conversations.alias END, \
               last_activity_ms = MAX(lan_share_conversations.last_activity_ms, \
                                      excluded.last_activity_ms)",
            rusqlite::params![peer_id, alias, activity_ms, empty_preview_enc],
        )
        .map_err(|e| format!("写入会话失败: {}", e))?;
        Ok(())
    }

    /// 追加一条消息并同步更新会话（预览 / 活动时间 / 未读）。
    ///
    /// `direction == "in"` 时未读 +1；`direction == "out"` 不加。
    pub fn append_message(
        &self,
        peer_id: &str,
        alias: &str,
        message: &ChatMessage,
    ) -> Result<(), String> {
        let payload = MessagePayload::from_message(message);
        let payload_json =
            serde_json::to_string(&payload).map_err(|e| format!("序列化消息载荷失败: {}", e))?;
        let payload_enc = self.seal(&payload_json)?;
        let preview = preview_of(&message.kind, &message.text, message.file_name.as_deref());
        let preview_enc = self.seal(&preview)?;
        let unread_delta: i64 = if message.direction == "in" { 1 } else { 0 };

        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {}", e))?;
        // 会话 upsert：保留 created_at_ms；alias 非空才覆盖；活动时间只前进不回退；预览刷新；in 未读 +1
        tx.execute(
            "INSERT INTO lan_share_conversations \
               (id, alias, last_activity_ms, last_preview_enc, unread, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(id) DO UPDATE SET \
               alias = CASE WHEN excluded.alias <> '' THEN excluded.alias \
                            ELSE lan_share_conversations.alias END, \
               last_activity_ms = MAX(lan_share_conversations.last_activity_ms, \
                                      excluded.last_activity_ms), \
               last_preview_enc = excluded.last_preview_enc, \
               unread = lan_share_conversations.unread + excluded.unread",
            rusqlite::params![
                peer_id,
                alias,
                message.created_at_ms,
                preview_enc,
                unread_delta,
                message.created_at_ms
            ],
        )
        .map_err(|e| format!("写入会话失败: {}", e))?;
        tx.execute(
            "INSERT INTO lan_share_messages \
               (id, peer_id, direction, kind, created_at_ms, payload_enc) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                message.id,
                peer_id,
                message.direction,
                message.kind,
                message.created_at_ms,
                payload_enc
            ],
        )
        .map_err(|e| format!("写入消息失败: {}", e))?;
        tx.commit().map_err(|e| format!("提交事务失败: {}", e))
    }

    /// 最近 `limit` 条消息，按时间正序返回
    pub fn list_messages(
        &self,
        peer_id: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        // 先按时间倒序取最近 N 条，再反转成正序
        let mut stmt = conn
            .prepare(
                "SELECT id, direction, kind, created_at_ms, payload_enc \
                 FROM lan_share_messages WHERE peer_id = ?1 \
                 ORDER BY created_at_ms DESC, id DESC LIMIT ?2",
            )
            .map_err(|e| format!("查询消息失败: {}", e))?;
        let rows = stmt
            .query_map(rusqlite::params![peer_id, limit as i64], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    direction: row.get(1)?,
                    kind: row.get(2)?,
                    created_at_ms: row.get(3)?,
                    payload_enc: row.get(4)?,
                })
            })
            .map_err(|e| format!("查询消息失败: {}", e))?;
        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(|e| format!("读取消息失败: {}", e))?;
            match row.into_message(peer_id, self) {
                Ok(message) => out.push(message),
                Err(err) => {
                    // 单条坏数据跳过，别让整个会话打不开
                    eprintln!("[lanshare] 跳过一条无法解密的消息: {}", err);
                }
            }
        }
        out.reverse();
        Ok(out)
    }

    /// 更新消息状态（可同时写失败原因 / 落盘路径）
    ///
    /// `error` 传 `Some` 覆盖失败原因、传 `None` 清空；`file_path` 只有传 `Some` 才覆盖
    /// （已收到的文件路径是留存数据，仅改状态时不该被擦掉）。
    pub fn update_message_status(
        &self,
        peer_id: &str,
        message_id: &str,
        status: &str,
        error: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<(), String> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {}", e))?;
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT payload_enc, kind FROM lan_share_messages \
                 WHERE peer_id = ?1 AND id = ?2",
                rusqlite::params![peer_id, message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("查询消息失败: {}", e))?;
        let Some((payload_enc, kind)) = row else {
            // 会话可能刚被删除：静默忽略，别把已过期的发送线程回写变成用户可见错误
            eprintln!(
                "[lanshare] 更新消息状态时找不到消息（可能已删除会话）: {}",
                message_id
            );
            return Ok(());
        };
        let json = match self.unseal(&payload_enc) {
            Ok(json) => json,
            Err(err) => {
                eprintln!("[lanshare] 消息 {} 解密失败，跳过状态更新: {}", message_id, err);
                return Ok(());
            }
        };
        let mut payload: MessagePayload =
            serde_json::from_str(&json).map_err(|e| format!("解析消息载荷失败: {}", e))?;
        payload.status = status.to_string();
        payload.error = error.map(str::to_string);
        if let Some(path) = file_path {
            payload.file_path = Some(path.to_string());
        }
        let updated_json =
            serde_json::to_string(&payload).map_err(|e| format!("序列化消息载荷失败: {}", e))?;
        let updated_enc = self.seal(&updated_json)?;
        tx.execute(
            "UPDATE lan_share_messages SET payload_enc = ?1 WHERE peer_id = ?2 AND id = ?3",
            rusqlite::params![updated_enc, peer_id, message_id],
        )
        .map_err(|e| format!("更新消息状态失败: {}", e))?;

        // 这条是会话最后一条时，预览跟着新状态刷新（文件消息失败仍保留 `[文件] 名`）
        let last_id: Option<String> = tx
            .query_row(
                "SELECT id FROM lan_share_messages WHERE peer_id = ?1 \
                 ORDER BY created_at_ms DESC, id DESC LIMIT 1",
                rusqlite::params![peer_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("查询最后一条消息失败: {}", e))?;
        if last_id.as_deref() == Some(message_id) {
            let preview = preview_of(&kind, &payload.text, payload.file_name.as_deref());
            let preview_enc = self.seal(&preview)?;
            tx.execute(
                "UPDATE lan_share_conversations SET last_preview_enc = ?1 WHERE id = ?2",
                rusqlite::params![preview_enc, peer_id],
            )
            .map_err(|e| format!("更新会话预览失败: {}", e))?;
        }
        tx.commit().map_err(|e| format!("提交事务失败: {}", e))
    }

    /// 全部会话的未读总数（托盘角标用）
    pub fn total_unread(&self) -> Result<u32, String> {
        let conn = self.connect()?;
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(unread), 0) FROM lan_share_conversations",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("统计未读失败: {}", e))?;
        Ok(total.max(0) as u32)
    }

    /// 该会话里「本机收到的、还带落盘路径」的文件消息数
    pub fn attachment_count(&self, peer_id: &str) -> Result<u32, String> {
        Ok(self.received_file_paths(peer_id)?.len() as u32)
    }

    /// 该会话里收到的文件路径（只读，删除会话前收集用）
    pub fn received_file_paths(&self, peer_id: &str) -> Result<Vec<String>, String> {
        let rows = self.incoming_file_rows(peer_id)?;
        let mut paths = Vec::new();
        for (id, payload_enc) in rows {
            let payload = match self.decrypt_file_payload(&id, &payload_enc) {
                Some(payload) => payload,
                None => continue,
            };
            if let Some(path) = payload.file_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                paths.push(path.to_string());
            }
        }
        Ok(paths)
    }

    /// 把收到的文件的落盘路径从库里清掉，并标记 `status = deleted`。
    ///
    /// 返回被清掉的路径；文件本身由调用方删除（先清库、再删文件）。
    pub fn clear_received_file_paths(&self, peer_id: &str) -> Result<Vec<String>, String> {
        let rows = self.incoming_file_rows(peer_id)?;
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {}", e))?;
        let mut paths = Vec::new();
        for (id, payload_enc) in rows {
            let Some(mut payload) = self.decrypt_file_payload(&id, &payload_enc) else {
                continue;
            };
            let Some(path) = payload
                .file_path
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            payload.file_path = None;
            payload.status = "deleted".to_string();
            payload.error = None;
            let json = serde_json::to_string(&payload)
                .map_err(|e| format!("序列化消息载荷失败: {}", e))?;
            let enc = self.seal(&json)?;
            tx.execute(
                "UPDATE lan_share_messages SET payload_enc = ?1 WHERE peer_id = ?2 AND id = ?3",
                rusqlite::params![enc, peer_id, id],
            )
            .map_err(|e| format!("更新消息载荷失败: {}", e))?;
            paths.push(path);
        }
        tx.commit().map_err(|e| format!("提交事务失败: {}", e))?;
        Ok(paths)
    }

    /// 收到的文件消息原始行（未解密）
    fn incoming_file_rows(&self, peer_id: &str) -> Result<Vec<(String, String)>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, payload_enc FROM lan_share_messages \
                 WHERE peer_id = ?1 AND direction = 'in' AND kind = 'file' \
                 ORDER BY created_at_ms ASC, id ASC",
            )
            .map_err(|e| format!("查询文件消息失败: {}", e))?;
        let rows = stmt
            .query_map(rusqlite::params![peer_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("查询文件消息失败: {}", e))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("读取文件消息失败: {}", e))?);
        }
        Ok(out)
    }

    /// 解密单条文件消息载荷；坏数据只告警并跳过
    fn decrypt_file_payload(&self, id: &str, payload_enc: &str) -> Option<MessagePayload> {
        let json = match self.unseal(payload_enc) {
            Ok(json) => json,
            Err(err) => {
                eprintln!("[lanshare] 文件消息 {} 解密失败，跳过: {}", id, err);
                return None;
            }
        };
        match serde_json::from_str(&json) {
            Ok(payload) => Some(payload),
            Err(err) => {
                eprintln!("[lanshare] 文件消息 {} 载荷解析失败，跳过: {}", id, err);
                None
            }
        }
    }

    /// 清空未读；返回是否真的从「有未读」变成 0（避免前端反复触发已读事件）。
    pub fn mark_read(&self, peer_id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE lan_share_conversations SET unread = 0 WHERE id = ?1 AND unread <> 0",
                rusqlite::params![peer_id],
            )
            .map_err(|e| format!("清零未读失败: {}", e))?;
        Ok(changed > 0)
    }

    /// 删除单条消息（仅本机记录），返回是否真的删掉一条。
    ///
    /// - 消息不存在 → `Ok(false)`（幂等，不报错；发送线程晚回写的竞态也算这种）；
    /// - 入站消息删掉后未读近似减 1（没有按条的已读标记，只能 `MAX(unread - 1, 0)`）；
    /// - 删的是会话最新一条时，把会话的 `last_activity_ms` / 预览回退到剩下的最新一条；
    ///   一条不剩就删掉会话行 —— 等价于「清除聊天记录」，离线对端不再出现在列表里；
    /// - 本机已收到的文件不删除（只删消息记录，物理文件保留）。
    pub fn delete_message(&self, peer_id: &str, message_id: &str) -> Result<bool, String> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {}", e))?;
        let direction: Option<String> = tx
            .query_row(
                "SELECT direction FROM lan_share_messages WHERE peer_id = ?1 AND id = ?2",
                rusqlite::params![peer_id, message_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("查询消息失败: {}", e))?;
        let Some(direction) = direction else {
            return Ok(false);
        };
        tx.execute(
            "DELETE FROM lan_share_messages WHERE peer_id = ?1 AND id = ?2",
            rusqlite::params![peer_id, message_id],
        )
        .map_err(|e| format!("删除消息失败: {}", e))?;
        if direction == "in" {
            tx.execute(
                "UPDATE lan_share_conversations SET unread = MAX(unread - 1, 0) WHERE id = ?1",
                rusqlite::params![peer_id],
            )
            .map_err(|e| format!("更新未读失败: {}", e))?;
        }
        // 剩下最新一条：刷新会话活动时间 / 预览；一条不剩就删会话行
        let latest: Option<(String, i64, String)> = tx
            .query_row(
                "SELECT kind, created_at_ms, payload_enc FROM lan_share_messages \
                 WHERE peer_id = ?1 ORDER BY created_at_ms DESC, id DESC LIMIT 1",
                rusqlite::params![peer_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("查询最新消息失败: {}", e))?;
        match latest {
            Some((kind, created_at_ms, payload_enc)) => {
                let preview = self.preview_of_encrypted(&kind, &payload_enc);
                let preview_enc = self.seal(&preview)?;
                tx.execute(
                    "UPDATE lan_share_conversations \
                     SET last_activity_ms = ?1, last_preview_enc = ?2 WHERE id = ?3",
                    rusqlite::params![created_at_ms, preview_enc, peer_id],
                )
                .map_err(|e| format!("更新会话预览失败: {}", e))?;
            }
            None => {
                tx.execute(
                    "DELETE FROM lan_share_conversations WHERE id = ?1",
                    rusqlite::params![peer_id],
                )
                .map_err(|e| format!("删除空会话失败: {}", e))?;
            }
        }
        tx.commit().map_err(|e| format!("提交事务失败: {}", e))?;
        Ok(true)
    }

    /// 解密一条消息载荷后拼会话预览；单条坏数据只告警并用占位（文本空 / 文件只显示 `[文件]`）
    fn preview_of_encrypted(&self, kind: &str, payload_enc: &str) -> String {
        let payload = self
            .unseal(payload_enc)
            .map_err(|e| format!("解密消息失败: {}", e))
            .and_then(|json| {
                serde_json::from_str::<MessagePayload>(&json)
                    .map_err(|e| format!("解析消息载荷失败: {}", e))
            });
        match payload {
            Ok(payload) => preview_of(kind, &payload.text, payload.file_name.as_deref()),
            Err(err) => {
                eprintln!("[lanshare] 剩余最新一条消息解密失败，预览用占位: {}", err);
                preview_of(kind, "", None)
            }
        }
    }

    /// 删除会话及其全部消息（仅本机）
    pub fn delete_conversation(&self, peer_id: &str) -> Result<(), String> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {}", e))?;
        tx.execute(
            "DELETE FROM lan_share_messages WHERE peer_id = ?1",
            rusqlite::params![peer_id],
        )
        .map_err(|e| format!("删除会话消息失败: {}", e))?;
        tx.execute(
            "DELETE FROM lan_share_conversations WHERE id = ?1",
            rusqlite::params![peer_id],
        )
        .map_err(|e| format!("删除会话失败: {}", e))?;
        tx.commit().map_err(|e| format!("提交事务失败: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "A4:83:E7:12:34:56";

    fn test_keys() -> VaultKeys {
        VaultKeys {
            enc_key: [0x11; 32],
            mac_key: [0x22; 32],
        }
    }

    /// 临时目录 + 已迁移到 v4 的库（`ChatStore::new` 不建表，测试自己先 open 一次）
    fn test_store() -> (tempfile::TempDir, ChatStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        crate::db::open(&path).expect("初始化数据库失败");
        (dir, ChatStore::new(path, test_keys()))
    }

    fn raw_count(store: &ChatStore, sql: &str, peer_id: &str) -> i64 {
        let conn = Connection::open(&store.path).unwrap();
        conn.query_row(sql, rusqlite::params![peer_id], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn append_in_and_out_updates_conversation() {
        let (_dir, store) = test_store();
        store
            .append_message(
                PEER,
                "冯文山的 MacBook",
                &ChatMessage::text("m1", PEER, "in", "你好", 1_000),
            )
            .unwrap();

        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.id, PEER);
        assert_eq!(conversation.alias, "冯文山的 MacBook");
        assert_eq!(conversation.last_preview, "你好");
        assert_eq!(conversation.last_activity_ms, 1_000);
        assert_eq!(conversation.created_at_ms, 1_000);
        assert_eq!(conversation.unread, 1);

        // out 消息不加未读，但会刷新预览 / 活动时间；空 alias 不覆盖，created_at 不变
        store
            .append_message(PEER, "", &ChatMessage::text("m2", PEER, "out", "收到", 2_000))
            .unwrap();
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.unread, 1);
        assert_eq!(conversation.last_preview, "收到");
        assert_eq!(conversation.last_activity_ms, 2_000);
        assert_eq!(conversation.created_at_ms, 1_000, "created_at_ms 不应被后写覆盖");
        assert_eq!(conversation.alias, "冯文山的 MacBook", "空 alias 不应覆盖旧别名");

        // 非空 alias 才覆盖；更旧的消息不会让活动时间回退
        store
            .append_message(PEER, "新名字", &ChatMessage::text("m3", PEER, "in", "再聊", 1_500))
            .unwrap();
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.alias, "新名字");
        assert_eq!(conversation.unread, 2);
        assert_eq!(
            conversation.last_activity_ms, 2_000,
            "last_activity_ms 只前进不回退"
        );
    }

    #[test]
    fn append_file_message_uses_file_preview() {
        let (_dir, store) = test_store();
        let mut file = ChatMessage::file("f1", PEER, "out", "安装包.dmg", 1024, "sending", 5_000);
        file.transfer_id = Some("t1".to_string());
        file.file_index = Some(0);
        file.file_total = Some(2);
        store.append_message(PEER, "MacBook", &file).unwrap();

        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.last_preview, "[文件] 安装包.dmg");
        assert_eq!(conversation.unread, 0, "文件 out 消息不加未读");

        let messages = store.list_messages(PEER, 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, "file");
        assert_eq!(messages[0].file_name.as_deref(), Some("安装包.dmg"));
        assert_eq!(messages[0].file_size, Some(1024));
        assert_eq!(messages[0].transfer_id.as_deref(), Some("t1"));
        assert_eq!(messages[0].file_index, Some(0));
        assert_eq!(messages[0].file_total, Some(2));
    }

    #[test]
    fn text_preview_truncates_by_chars() {
        let (_dir, store) = test_store();
        let long = "中".repeat(120);
        store
            .append_message(PEER, "MacBook", &ChatMessage::text("m1", PEER, "in", &long, 1_000))
            .unwrap();
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.last_preview.chars().count(), PREVIEW_MAX_CHARS);
        assert!(conversation.last_preview.chars().all(|c| c == '中'));
    }

    #[test]
    fn upsert_conversation_keeps_preview_and_unread() {
        let (_dir, store) = test_store();
        // 第一次 upsert：建会话，预览为空、未读 0
        store.upsert_conversation(PEER, "旧名字", 1_000).unwrap();
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.last_preview, "");
        assert_eq!(conversation.unread, 0);
        assert_eq!(conversation.created_at_ms, 1_000);

        // 追加一条 in 消息后，再 upsert 不应动预览 / 未读
        store
            .append_message(PEER, "旧名字", &ChatMessage::text("m1", PEER, "in", "hi", 2_000))
            .unwrap();
        store.upsert_conversation(PEER, "新名字", 3_000).unwrap();
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.alias, "新名字");
        assert_eq!(conversation.last_activity_ms, 3_000);
        assert_eq!(conversation.last_preview, "hi", "upsert 不应改预览");
        assert_eq!(conversation.unread, 1, "upsert 不应改未读");
        assert_eq!(conversation.created_at_ms, 1_000);

        // 空 alias / 更早活动时间都不该改动已有值
        store.upsert_conversation(PEER, "", 500).unwrap();
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.alias, "新名字");
        assert_eq!(conversation.last_activity_ms, 3_000);
    }

    #[test]
    fn list_conversations_sorts_by_activity_desc() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("a1", PEER, "in", "a", 1_000))
            .unwrap();
        let peer_b = "AA:BB:CC:DD:EE:FF";
        store
            .append_message(peer_b, "B", &ChatMessage::text("b1", peer_b, "in", "b", 3_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("a2", PEER, "out", "a2", 2_000))
            .unwrap();

        let conversations = store.list_conversations().unwrap();
        assert_eq!(conversations.len(), 2);
        assert_eq!(conversations[0].id, peer_b);
        assert_eq!(conversations[1].id, PEER);
        assert_eq!(conversations[1].last_preview, "a2");
    }

    #[test]
    fn list_messages_orders_ascending_and_limits() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("m1", PEER, "in", "一", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m2", PEER, "out", "三", 3_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m3", PEER, "in", "二", 2_000))
            .unwrap();

        let ids: Vec<String> = store
            .list_messages(PEER, 10)
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect();
        assert_eq!(ids, vec!["m1", "m3", "m2"], "按时间正序返回");

        let ids: Vec<String> = store
            .list_messages(PEER, 2)
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect();
        assert_eq!(ids, vec!["m3", "m2"], "取最近 limit 条后仍是正序");

        assert!(store.list_messages(PEER, 0).unwrap().is_empty());
        assert!(store.list_messages("unknown", 10).unwrap().is_empty());
    }

    #[test]
    fn update_message_status_persists_and_refreshes_preview() {
        let (_dir, store) = test_store();
        store
            .append_message(
                PEER,
                "A",
                &ChatMessage::text("m1", PEER, "out", "hello", 1_000),
            )
            .unwrap();
        assert_eq!(store.get_conversation(PEER).unwrap().unwrap().last_preview, "hello");

        store
            .update_message_status(PEER, "m1", "failed", Some("设备已离线"), None)
            .unwrap();
        let message = &store.list_messages(PEER, 10).unwrap()[0];
        assert_eq!(message.status, "failed");
        assert_eq!(message.error.as_deref(), Some("设备已离线"));
        // 文本消息失败时预览仍是正文
        assert_eq!(store.get_conversation(PEER).unwrap().unwrap().last_preview, "hello");

        // 状态回写成功时清空旧的失败原因
        store
            .update_message_status(PEER, "m1", "sent", None, None)
            .unwrap();
        let message = &store.list_messages(PEER, 10).unwrap()[0];
        assert_eq!(message.status, "sent");
        assert_eq!(message.error, None);

        // 文件消息失败：预览保留 `[文件] 名`，路径不被误擦
        let mut file = ChatMessage::file("f1", PEER, "in", "报告.pdf", 2048, "received", 2_000);
        file.file_path = Some("/tmp/报告.pdf".to_string());
        store.append_message(PEER, "A", &file).unwrap();
        assert_eq!(
            store.get_conversation(PEER).unwrap().unwrap().last_preview,
            "[文件] 报告.pdf"
        );
        store
            .update_message_status(PEER, "f1", "failed", Some("写入失败"), None)
            .unwrap();
        let messages = store.list_messages(PEER, 10).unwrap();
        assert_eq!(messages[1].status, "failed");
        assert_eq!(messages[1].error.as_deref(), Some("写入失败"));
        assert_eq!(
            messages[1].file_path.as_deref(),
            Some("/tmp/报告.pdf"),
            "None 的 file_path 参数不应擦掉已保存的路径"
        );
        // 文件消息失败时预览仍然是文件摘要
        assert_eq!(
            store.get_conversation(PEER).unwrap().unwrap().last_preview,
            "[文件] 报告.pdf"
        );

        // 非最后一条消息的状态更新不动会话预览
        store
            .update_message_status(PEER, "m1", "failed", Some("旧消息"), None)
            .unwrap();
        assert_eq!(
            store.get_conversation(PEER).unwrap().unwrap().last_preview,
            "[文件] 报告.pdf"
        );

        // 更新不存在的消息是安全的 no-op
        store
            .update_message_status(PEER, "missing", "sent", None, None)
            .unwrap();
    }

    #[test]
    fn mark_read_clears_unread() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("m1", PEER, "in", "一", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m2", PEER, "in", "二", 2_000))
            .unwrap();
        assert_eq!(store.get_conversation(PEER).unwrap().unwrap().unread, 2);

        assert!(store.mark_read(PEER).unwrap(), "有未读时应报告发生变化");
        assert!(!store.mark_read(PEER).unwrap(), "已是 0 时不应再报告变化");
        assert_eq!(store.get_conversation(PEER).unwrap().unwrap().unread, 0);
        // 已读之后再收到消息，未读从 0 重新计数
        store
            .append_message(PEER, "A", &ChatMessage::text("m3", PEER, "in", "三", 3_000))
            .unwrap();
        assert_eq!(store.get_conversation(PEER).unwrap().unwrap().unread, 1);
        // 不存在的会话不报错
        store.mark_read("unknown").unwrap();
    }

    #[test]
    fn delete_conversation_cascades_and_keeps_other_peers() {
        let (_dir, store) = test_store();
        let peer_b = "AA:BB:CC:DD:EE:FF";
        store
            .append_message(PEER, "A", &ChatMessage::text("a1", PEER, "in", "a", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("a2", PEER, "out", "a2", 2_000))
            .unwrap();
        store
            .append_message(peer_b, "B", &ChatMessage::text("b1", peer_b, "in", "b", 3_000))
            .unwrap();

        store.delete_conversation(PEER).unwrap();
        assert!(store.get_conversation(PEER).unwrap().is_none());
        assert!(store.list_messages(PEER, 10).unwrap().is_empty());
        assert_eq!(
            raw_count(
                &store,
                "SELECT COUNT(*) FROM lan_share_messages WHERE peer_id = ?1",
                PEER
            ),
            0
        );
        // 其它会话不受影响
        assert_eq!(store.list_conversations().unwrap().len(), 1);
        assert_eq!(store.list_messages(peer_b, 10).unwrap().len(), 1);
        // 重复删除 / 删除不存在的会话不报错
        store.delete_conversation(PEER).unwrap();

        // 删除后对方再来消息能重新建会话
        store
            .append_message(PEER, "A", &ChatMessage::text("a3", PEER, "in", "又来了", 4_000))
            .unwrap();
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.unread, 1);
        assert_eq!(conversation.last_preview, "又来了");
    }

    #[test]
    fn payload_and_preview_are_encrypted_at_rest() {
        let (_dir, store) = test_store();
        store
            .append_message(
                PEER,
                "机密设备",
                &ChatMessage::text("m1", PEER, "in", "机密正文", 1_000),
            )
            .unwrap();
        store
            .append_message(
                PEER,
                "机密设备",
                &ChatMessage::file("m2", PEER, "in", "机密文件.pdf", 100, "received", 2_000),
            )
            .unwrap();

        let conn = Connection::open(&store.path).unwrap();
        let payloads: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT payload_enc FROM lan_share_messages")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(payloads.len(), 2);
        for payload in &payloads {
            assert!(payload.starts_with("v1."), "消息载荷应是密文信封: {}", payload);
            assert!(!payload.contains("机密正文"));
            assert!(!payload.contains("机密文件"));
        }
        let preview: String = conn
            .query_row(
                "SELECT last_preview_enc FROM lan_share_conversations WHERE id = ?1",
                rusqlite::params![PEER],
                |row| row.get(0),
            )
            .unwrap();
        assert!(preview.starts_with("v1."), "会话预览应是密文信封: {}", preview);
        assert!(!preview.contains("机密文件"));
        // 别名按设计是明文列，不加密
        let alias: String = conn
            .query_row(
                "SELECT alias FROM lan_share_conversations WHERE id = ?1",
                rusqlite::params![PEER],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alias, "机密设备");
    }

    #[test]
    fn total_unread_sums_all_conversations() {
        let (_dir, store) = test_store();
        let peer_b = "AA:BB:CC:DD:EE:FF";
        store
            .append_message(PEER, "A", &ChatMessage::text("a1", PEER, "in", "1", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("a2", PEER, "in", "2", 2_000))
            .unwrap();
        store
            .append_message(peer_b, "B", &ChatMessage::text("b1", peer_b, "in", "3", 3_000))
            .unwrap();
        assert_eq!(store.total_unread().unwrap(), 3);
        store.mark_read(PEER).unwrap();
        assert_eq!(store.total_unread().unwrap(), 1);
        store.delete_conversation(peer_b).unwrap();
        assert_eq!(store.total_unread().unwrap(), 0);
    }

    #[test]
    fn attachment_count_only_counts_incoming_with_path() {
        let (_dir, store) = test_store();
        store
            .append_message(
                PEER,
                "A",
                &ChatMessage::file("in1", PEER, "in", "收到.pdf", 10, "received", 1_000),
            )
            .unwrap();
        store
            .update_message_status(PEER, "in1", "received", None, Some("/tmp/收到.pdf"))
            .unwrap();
        store
            .append_message(
                PEER,
                "A",
                &ChatMessage::file("out1", PEER, "out", "发出.pdf", 10, "sent", 2_000),
            )
            .unwrap();
        store
            .update_message_status(PEER, "out1", "sent", None, Some("/tmp/发出.pdf"))
            .unwrap();
        // 收到的文件但还没落盘路径（旧数据）不计
        store
            .append_message(
                PEER,
                "A",
                &ChatMessage::file("in2", PEER, "in", "无路径.pdf", 10, "received", 3_000),
            )
            .unwrap();

        let peer_b = "AA:BB:CC:DD:EE:FF";
        store
            .append_message(
                peer_b,
                "B",
                &ChatMessage::file("b1", peer_b, "in", "b.pdf", 10, "received", 4_000),
            )
            .unwrap();
        store
            .update_message_status(peer_b, "b1", "received", None, Some("/tmp/b.pdf"))
            .unwrap();

        assert_eq!(store.attachment_count(PEER).unwrap(), 1);
        assert_eq!(
            store.received_file_paths(PEER).unwrap(),
            vec!["/tmp/收到.pdf".to_string()]
        );
        assert_eq!(store.attachment_count(peer_b).unwrap(), 1);
    }

    #[test]
    fn clear_received_file_paths_marks_deleted_and_keeps_other_peer() {
        let (_dir, store) = test_store();
        store
            .append_message(
                PEER,
                "A",
                &ChatMessage::file("in1", PEER, "in", "收到.pdf", 10, "received", 1_000),
            )
            .unwrap();
        store
            .update_message_status(PEER, "in1", "received", None, Some("/tmp/收到.pdf"))
            .unwrap();
        store
            .append_message(
                PEER,
                "A",
                &ChatMessage::file("out1", PEER, "out", "发出.pdf", 10, "sent", 2_000),
            )
            .unwrap();
        store
            .update_message_status(PEER, "out1", "sent", None, Some("/tmp/发出.pdf"))
            .unwrap();
        let peer_b = "AA:BB:CC:DD:EE:FF";
        store
            .append_message(
                peer_b,
                "B",
                &ChatMessage::file("b1", peer_b, "in", "b.pdf", 10, "received", 3_000),
            )
            .unwrap();
        store
            .update_message_status(peer_b, "b1", "received", None, Some("/tmp/b.pdf"))
            .unwrap();

        let cleared = store.clear_received_file_paths(PEER).unwrap();
        assert_eq!(cleared, vec!["/tmp/收到.pdf".to_string()]);
        let messages = store.list_messages(PEER, 10).unwrap();
        let incoming = messages.iter().find(|m| m.id == "in1").unwrap();
        assert_eq!(incoming.status, "deleted");
        assert_eq!(incoming.file_path, None);
        let outgoing = messages.iter().find(|m| m.id == "out1").unwrap();
        assert_eq!(outgoing.status, "sent");
        assert_eq!(outgoing.file_path.as_deref(), Some("/tmp/发出.pdf"));
        assert_eq!(store.attachment_count(PEER).unwrap(), 0);
        assert_eq!(store.attachment_count(peer_b).unwrap(), 1);
        // 再清一次是空操作
        assert!(store.clear_received_file_paths(PEER).unwrap().is_empty());
    }

    #[test]
    fn file_path_is_encrypted_at_rest() {
        let (_dir, store) = test_store();
        let secret_path = "/Users/secret/机密目录/合同.pdf";
        store
            .append_message(
                PEER,
                "A",
                &ChatMessage::file("f1", PEER, "in", "合同.pdf", 10, "received", 1_000),
            )
            .unwrap();
        store
            .update_message_status(PEER, "f1", "received", None, Some(secret_path))
            .unwrap();

        let conn = Connection::open(&store.path).unwrap();
        let payload: String = conn
            .query_row(
                "SELECT payload_enc FROM lan_share_messages WHERE id = 'f1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(payload.starts_with("v1."));
        assert!(!payload.contains("合同.pdf"));
        assert!(!payload.contains("机密目录"));
        assert!(!payload.contains(secret_path));
        // 解密后路径仍在
        let messages = store.list_messages(PEER, 10).unwrap();
        assert_eq!(messages[0].file_path.as_deref(), Some(secret_path));
    }

    #[test]
    fn corrupt_rows_are_skipped_without_failing_the_api() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("m1", PEER, "in", "好", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m2", PEER, "out", "收到", 2_000))
            .unwrap();

        {
            let conn = Connection::open(&store.path).unwrap();
            conn.execute(
                "UPDATE lan_share_messages SET payload_enc = 'v1.bad' WHERE id = 'm1'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE lan_share_conversations SET last_preview_enc = 'v1.bad' WHERE id = ?1",
                rusqlite::params![PEER],
            )
            .unwrap();
        }

        // 坏消息被跳过，好的那条照常返回
        let messages = store.list_messages(PEER, 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "m2");
        // 坏预览显示为空串，列表接口不报错
        let conversations = store.list_conversations().unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].last_preview, "");
        // 坏消息的状态更新是 no-op，不报错
        store
            .update_message_status(PEER, "m1", "sent", None, None)
            .unwrap();
        // 删除后要重算预览时，坏载荷只让预览变占位，不会让删除失败
        assert!(store.delete_message(PEER, "m2").unwrap());
        let conversations = store.list_conversations().unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].last_preview, "");
    }

    #[test]
    fn delete_middle_message_keeps_others_and_preview() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("m1", PEER, "in", "一", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m2", PEER, "in", "二", 2_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m3", PEER, "out", "三", 3_000))
            .unwrap();
        assert_eq!(store.get_conversation(PEER).unwrap().unwrap().unread, 2);

        assert!(store.delete_message(PEER, "m2").unwrap());

        let ids: Vec<String> = store
            .list_messages(PEER, 10)
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect();
        assert_eq!(ids, vec!["m1", "m3"], "只删中间那条，其它消息保留");
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.last_preview, "三", "预览仍是最新一条");
        assert_eq!(conversation.last_activity_ms, 3_000);
        assert_eq!(conversation.unread, 1, "删入站消息未读减 1");

        // 删 out 消息不动未读；预览回退到剩下最新一条
        assert!(store.delete_message(PEER, "m3").unwrap());
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.unread, 1);
        assert_eq!(conversation.last_preview, "一");
    }

    #[test]
    fn delete_last_message_removes_empty_conversation() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("m1", PEER, "in", "旧", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m2", PEER, "in", "新", 2_000))
            .unwrap();

        // 删最后一条：会话保留，活动时间 / 预览回退到剩下的最新一条
        assert!(store.delete_message(PEER, "m2").unwrap());
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.last_preview, "旧");
        assert_eq!(conversation.last_activity_ms, 1_000);
        assert_eq!(conversation.unread, 1);

        // 再删最后一条：会话行也删掉（离线对端从列表消失），消息清空
        assert!(store.delete_message(PEER, "m1").unwrap());
        assert!(store.get_conversation(PEER).unwrap().is_none());
        assert!(store.list_messages(PEER, 10).unwrap().is_empty());
        assert!(store.list_conversations().unwrap().is_empty());
        assert_eq!(
            raw_count(
                &store,
                "SELECT COUNT(*) FROM lan_share_messages WHERE peer_id = ?1",
                PEER
            ),
            0
        );
        assert_eq!(
            raw_count(
                &store,
                "SELECT COUNT(*) FROM lan_share_conversations WHERE id = ?1",
                PEER
            ),
            0
        );
    }

    #[test]
    fn delete_inbound_message_never_makes_unread_negative() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("m1", PEER, "in", "一", 1_000))
            .unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("m2", PEER, "in", "二", 2_000))
            .unwrap();
        assert!(store.mark_read(PEER).unwrap());
        assert_eq!(store.get_conversation(PEER).unwrap().unwrap().unread, 0);

        // 已读（unread=0）后再删入站消息：MAX(unread-1, 0) 兜底，不会变负
        assert!(store.delete_message(PEER, "m1").unwrap());
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.unread, 0);
        assert_eq!(store.list_messages(PEER, 10).unwrap().len(), 1);
    }

    #[test]
    fn delete_missing_message_is_idempotent() {
        let (_dir, store) = test_store();
        store
            .append_message(PEER, "A", &ChatMessage::text("m1", PEER, "in", "hi", 1_000))
            .unwrap();

        // 不存在 / 别的 peer 下的 id → false，且不动已有数据
        assert!(!store.delete_message(PEER, "missing").unwrap());
        assert!(!store.delete_message("unknown-peer", "m1").unwrap());
        assert!(store.get_conversation(PEER).unwrap().is_some());
        assert_eq!(store.list_messages(PEER, 10).unwrap().len(), 1);

        // 重复删同一条：第一次 true，第二次 false
        assert!(store.delete_message(PEER, "m1").unwrap());
        assert!(!store.delete_message(PEER, "m1").unwrap());
    }

    #[test]
    fn delete_message_is_isolated_per_peer() {
        let (_dir, store) = test_store();
        let peer_b = "AA:BB:CC:DD:EE:FF";
        store
            .append_message(PEER, "A", &ChatMessage::text("a1", PEER, "in", "a", 1_000))
            .unwrap();
        store
            .append_message(peer_b, "B", &ChatMessage::text("b1", peer_b, "in", "b", 2_000))
            .unwrap();

        assert!(store.delete_message(PEER, "a1").unwrap());
        assert!(store.get_conversation(PEER).unwrap().is_none());
        let conversations = store.list_conversations().unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, peer_b);
        assert_eq!(conversations[0].last_preview, "b");
        assert_eq!(store.list_messages(peer_b, 10).unwrap().len(), 1);
        // 归属不串：B 会话下没有 a1
        assert!(!store.delete_message(peer_b, "a1").unwrap());
    }

    #[test]
    fn delete_file_message_keeps_file_on_disk_and_restores_previous_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        crate::db::open(&path).unwrap();
        let store = ChatStore::new(path, test_keys());

        // 收到的文件：落盘路径在加密载荷里；删消息不能删物理文件
        let file_path = dir.path().join("收到的报告.pdf");
        std::fs::write(&file_path, b"data").unwrap();
        store
            .append_message(PEER, "A", &ChatMessage::text("t1", PEER, "in", "看这个", 1_000))
            .unwrap();
        let mut file = ChatMessage::file("f1", PEER, "in", "收到的报告.pdf", 4, "received", 2_000);
        file.file_path = Some(file_path.to_string_lossy().to_string());
        store.append_message(PEER, "A", &file).unwrap();
        assert_eq!(
            store.get_conversation(PEER).unwrap().unwrap().last_preview,
            "[文件] 收到的报告.pdf"
        );

        assert!(store.delete_message(PEER, "f1").unwrap());
        assert!(file_path.exists(), "删除消息不应删除本机已收到的文件");
        let conversation = store.get_conversation(PEER).unwrap().unwrap();
        assert_eq!(conversation.last_preview, "看这个", "预览回退到上一条文本");
        assert_eq!(conversation.last_activity_ms, 1_000);
        assert_eq!(conversation.unread, 1, "原未读 2，删一条入站文件消息后减 1");
    }
}
