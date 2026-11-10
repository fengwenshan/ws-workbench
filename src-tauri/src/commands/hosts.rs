//! 「Hosts 在线编辑」的 Tauri 命令层：只做参数 / 错误翻译，业务在 `crate::hosts`。
//!
//! 命令名与 `lib.rs` 的 `generate_handler!` 一一对应：
//! `hosts_status` / `hosts_save` / `hosts_backups` / `hosts_restore` / `hosts_open_file_dir`。
//! 读取 hosts、备份、提权写入都是文件 / 进程操作，统一 `spawn_blocking`，
//! 避免用户在等系统授权框时卡住 WebView 主线程。

use crate::hosts::file;
use crate::hosts::store;
use crate::hosts::{self, HostsBackup, HostsSaveResult, HostsStatus};

/// 页面状态：路径 / 是否可读 / 完整内容 / 行数 / 备份 / 平台提示。
fn status_impl() -> Result<HostsStatus, String> {
    let path = file::hosts_path();
    let exists = file::hosts_exists();
    let current = file::read_hosts();
    let readable = current.is_ok();
    let content = current.unwrap_or_default();
    let backups = store::list_backups().unwrap_or_default();

    Ok(HostsStatus {
        path: path.to_string_lossy().to_string(),
        exists,
        readable,
        line_count: hosts::count_lines(&content),
        size: content.len(),
        content,
        backup_count: backups.len(),
        last_backup_at: backups
            .first()
            .map(|backup| backup.created_at.clone())
            .unwrap_or_default(),
        platform: platform_name().to_string(),
        needs_admin: cfg!(any(target_os = "macos", target_os = "windows")),
        hint: status_hint(exists, readable),
    })
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn status_hint(exists: bool, readable: bool) -> String {
    if !readable {
        return "hosts 文件不可读（或不是 UTF-8 编码），暂时不能在线编辑；请检查文件权限 / 编码后再试"
            .to_string();
    }
    let auth = if cfg!(target_os = "windows") {
        "保存需要管理员授权，会弹出 UAC 确认框；每次保存前会先备份当前内容"
    } else if cfg!(target_os = "macos") {
        "保存需要管理员授权，会弹出系统授权框；每次保存前会先备份当前内容"
    } else {
        "当前平台暂不支持写入 hosts"
    };
    if exists {
        format!("{auth}；取消授权不会改动文件。")
    } else {
        format!("未找到 hosts 文件，首次保存会自动创建；{auth}。")
    }
}

/// 读当前 hosts 文本 + 原始字节（备份与比较用同一份快照）。
fn read_current_hosts() -> Result<(String, Vec<u8>), String> {
    let bytes = file::read_hosts_bytes()?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        "hosts 文件不是 UTF-8 编码，暂不支持在线编辑（避免写坏原有内容）；请先手动把文件转成 UTF-8"
            .to_string()
    })?;
    Ok((text, bytes))
}

/// 写前备份（文件不存在时不产生空备份；内容为空但文件存在时也备份）。
fn backup_bytes(bytes: &[u8]) -> Result<String, String> {
    if !file::hosts_exists() {
        return Ok(String::new());
    }
    Ok(store::create_backup(bytes)?.id)
}

/// 保存整个 hosts 文件。
fn save_impl(content: &str) -> Result<HostsSaveResult, String> {
    hosts::validate_content(content)?;
    let (current, current_bytes) = read_current_hosts()?;
    let path = file::hosts_path().to_string_lossy().to_string();

    // 内容没变就不重复弹授权框，也不产生无意义备份
    if current == content {
        return Ok(HostsSaveResult {
            saved: false,
            path,
            backup_id: String::new(),
            line_count: hosts::count_lines(content),
            size: content.len(),
            flushed_dns: false,
            message: "hosts 内容与磁盘一致，无需写入".to_string(),
        });
    }

    let backup_id = backup_bytes(&current_bytes)?;
    let outcome = file::write_hosts(content)?;
    let message = if outcome.flushed_dns {
        "已保存 hosts，并已刷新 DNS 缓存".to_string()
    } else {
        "已保存 hosts；DNS 缓存刷新失败，如有解析异常请手动刷新".to_string()
    };

    Ok(HostsSaveResult {
        saved: true,
        path,
        backup_id,
        line_count: hosts::count_lines(content),
        size: content.len(),
        flushed_dns: outcome.flushed_dns,
        message,
    })
}

/// 从备份还原：同样先备份当前内容，再提权覆盖并刷新 DNS。
fn restore_impl(backup_id: &str) -> Result<HostsSaveResult, String> {
    let bytes = store::read_backup(backup_id)?;
    if bytes.len() > hosts::MAX_CONTENT_LEN {
        return Err(format!(
            "备份内容超过 1 MiB 上限（当前 {} 字节）",
            bytes.len()
        ));
    }
    let restored = String::from_utf8(bytes).map_err(|_| {
        "备份内容不是 UTF-8 编码，无法还原（备份可能已损坏）".to_string()
    })?;
    hosts::validate_content(&restored)?;

    let (current, current_bytes) = read_current_hosts()?;
    let path = file::hosts_path().to_string_lossy().to_string();
    if restored == current {
        return Ok(HostsSaveResult {
            saved: false,
            path,
            backup_id: String::new(),
            line_count: hosts::count_lines(&current),
            size: current.len(),
            flushed_dns: false,
            message: "备份内容与当前 hosts 相同，无需还原".to_string(),
        });
    }

    let new_backup_id = backup_bytes(&current_bytes)?;
    let outcome = file::write_hosts(&restored)?;
    let message = if outcome.flushed_dns {
        format!("已从备份 {backup_id} 还原 hosts，并已刷新 DNS 缓存")
    } else {
        format!("已从备份 {backup_id} 还原 hosts；DNS 缓存刷新失败，如有解析异常请手动刷新")
    };

    Ok(HostsSaveResult {
        saved: true,
        path,
        backup_id: new_backup_id,
        line_count: hosts::count_lines(&restored),
        size: restored.len(),
        flushed_dns: outcome.flushed_dns,
        message,
    })
}

// ============================== Tauri 命令 ==============================

#[tauri::command]
pub async fn hosts_status() -> Result<HostsStatus, String> {
    tauri::async_runtime::spawn_blocking(status_impl)
        .await
        .map_err(|err| format!("读取 hosts 状态失败：{err}"))?
}

#[tauri::command]
pub async fn hosts_save(content: String) -> Result<HostsSaveResult, String> {
    tauri::async_runtime::spawn_blocking(move || save_impl(&content))
        .await
        .map_err(|err| format!("保存 hosts 任务失败：{err}"))?
}

#[tauri::command]
pub async fn hosts_backups() -> Result<Vec<HostsBackup>, String> {
    tauri::async_runtime::spawn_blocking(store::list_backups)
        .await
        .map_err(|err| format!("读取备份列表失败：{err}"))?
}

#[tauri::command]
pub async fn hosts_restore(backup_id: String) -> Result<HostsSaveResult, String> {
    tauri::async_runtime::spawn_blocking(move || restore_impl(&backup_id))
        .await
        .map_err(|err| format!("还原 hosts 任务失败：{err}"))?
}

/// 打开 hosts 所在目录（只读展示用）。
#[tauri::command]
pub fn hosts_open_file_dir() -> Result<(), String> {
    let dir = file::hosts_dir();
    crate::commands::open_dir(dir.to_string_lossy().to_string()).map(|_| ())
}

// ============================== 集成 smoke ==============================

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// 读本机 hosts、再以同一内容「保存」：必须返回 saved=false（不弹授权、不产生备份）。
    /// 只读系统文件，不改动任何内容，跑在开发机 / CI 的 unix 环境。
    #[cfg(unix)]
    #[test]
    fn status_read_and_identical_save_is_noop() {
        let status = status_impl().expect("读 hosts 状态失败");
        assert!(!status.path.is_empty());
        assert!(status.readable, "hosts 应可读");

        let result = save_impl(&status.content).expect("保存相同内容失败");
        assert!(!result.saved);
        assert_eq!(result.line_count, status.line_count);
        assert!(result.message.contains("无需写入"), "{}", result.message);
    }
}
