//! 「Hosts 在线编辑」后端核心：整文件读写、校验、备份、提权写入。
//!
//! 设计见 `docs/superpowers/specs/2026-11-07-clipboard-and-hosts-design.md`
//!（2026-11-09 修订：改为整文件在线编辑，不再解析条目 / 维护托管块）。
//!
//! 分层约定：
//! - 本文件只放**纯逻辑**（不碰文件系统）：内容校验、行数统计、时间格式化；
//! - `store`：写入前备份到 `<data_dir>/hosts-backups/`；
//! - `file`：读系统 hosts、按平台提权写入、刷新 DNS。
//!
//! 安全边界：固定路径；写入前校验大小与编码并自动备份；写失败时原文件不变
//!（临时文件 + 原子替换）；取消系统授权不会改动文件。

pub mod file;
pub mod store;

use serde::{Deserialize, Serialize};

/// 写入内容的硬上限：1 MiB（系统 hosts 正常只有几 KB）
pub const MAX_CONTENT_LEN: usize = 1024 * 1024;
/// 备份最多保留 30 份
pub const MAX_BACKUPS: usize = 30;

// ============================== 数据模型 ==============================

/// 页面状态：路径、可读性、完整内容、备份信息。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostsStatus {
    /// 固定路径（macOS `/etc/hosts`；Windows `C:\Windows\System32\drivers\etc\hosts`）
    pub path: String,
    pub exists: bool,
    /// 是否已成功读出内容（不存在时也按空文件处理，视为可写）
    pub readable: bool,
    /// 当前 hosts 完整内容（在线编辑器直接展示；不可读时为空串）
    pub content: String,
    pub line_count: usize,
    /// 内容字节数，状态栏展示用
    pub size: usize,
    pub backup_count: usize,
    pub last_backup_at: String,
    /// macos / windows / linux
    pub platform: String,
    /// 写入 hosts 是否需要管理员授权（macOS / Windows 都是）
    pub needs_admin: bool,
    /// 直接展示给用户的中文说明
    pub hint: String,
}

/// 保存 / 还原的结果
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostsSaveResult {
    /// 是否真的执行了写操作（内容与磁盘一致时为 false）
    pub saved: bool,
    pub path: String,
    /// 本次写入前的备份 id（没有写入 / 原本没有 hosts 时为空串）
    pub backup_id: String,
    pub line_count: usize,
    pub size: usize,
    pub flushed_dns: bool,
    pub message: String,
}

/// 一份备份
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostsBackup {
    /// `<unix_ms>_hosts`
    pub id: String,
    /// 本地时间 `YYYY-MM-DD HH:MM:SS`
    pub created_at: String,
    pub size: u64,
}

// ============================== 纯逻辑 ==============================

/// 统计文件行数：尾随换行不额外算一行，空文件为 0。
pub fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        return 0;
    }
    let mut lines = content.split('\n').count();
    if content.ends_with('\n') {
        lines = lines.saturating_sub(1);
    }
    lines
}

/// 保存前校验：只做「不能写坏文件」的硬性检查，不限制用户写什么条目。
///
/// 在线编辑意味着用户可以直接写任意 hosts 语法，这里只拒绝：
/// - 超过 1 MiB（明显不是 hosts 文件）；
/// - 含 NUL 字节（二进制内容，写进去会破坏系统解析）。
pub fn validate_content(content: &str) -> Result<(), String> {
    if content.len() > MAX_CONTENT_LEN {
        return Err(format!(
            "内容超过 1 MiB 上限（当前 {} 字节）",
            content.len()
        ));
    }
    if content.contains('\0') {
        return Err("内容包含 NUL 字符，不能写入 hosts（请检查是否粘贴了二进制内容）".to_string());
    }
    Ok(())
}

/// 本地时间 `YYYY-MM-DD HH:MM:SS`（备份列表展示用）。
pub fn format_ms(ms: u64) -> String {
    use chrono::TimeZone;
    let seconds = (ms / 1000) as i64;
    let nanos = ((ms % 1000) * 1_000_000) as u32;
    match chrono::Local.timestamp_opt(seconds, nanos) {
        chrono::LocalResult::Single(time) => time.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => String::new(),
    }
}

/// 当前 unix 毫秒数。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

// ============================== 单测 ==============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_without_extra_trailing_newline() {
        assert_eq!(count_lines(""), 0);
        assert_eq!(count_lines("127.0.0.1 localhost"), 1);
        assert_eq!(count_lines("127.0.0.1 localhost\n"), 1);
        assert_eq!(count_lines("127.0.0.1 localhost\n::1 localhost\n"), 2);
        assert_eq!(count_lines("a\n\nb"), 3);
    }

    #[test]
    fn validate_accepts_normal_hosts_text() {
        let content = "127.0.0.1 localhost\n::1 localhost\n10.0.0.8 api.example.com  # 测试\n";
        assert!(validate_content(content).is_ok());
        assert!(validate_content("").is_ok());
    }

    #[test]
    fn validate_rejects_oversized_content() {
        let content = "a".repeat(MAX_CONTENT_LEN + 1);
        let error = validate_content(&content).expect_err("oversized");
        assert!(error.contains("1 MiB"), "{error}");
    }

    #[test]
    fn validate_rejects_nul_bytes() {
        let error = validate_content("127.0.0.1 localhost\0evil").expect_err("nul");
        assert!(error.contains("NUL"), "{error}");
    }

    #[test]
    fn format_ms_is_readable_or_empty() {
        let text = format_ms(now_ms());
        assert_eq!(text.len(), 19, "{text}");
        assert_eq!(text.as_bytes()[4], b'-');
        assert_eq!(text.as_bytes()[10], b' ');
    }
}
