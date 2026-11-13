//! macOS hosts 路径与提权写入实现。
//!
//! 平台实现文件：只在 macOS target 编译，禁止引用 `file::windows`。
//!
//! 写入走一次系统授权（设计 4.3）：生成 sh 脚本（文本生成在 `scripts.rs`）→
//! `osascript -e 'do shell script "sh <quoted>" with administrator privileges'`；
//! 脚本内 `cp` 到 `/etc/hosts.ws-workbench.tmp` → `chmod 644` → `mv -f` 原子替换 → 刷新 DNS。

use std::path::{Path, PathBuf};

use super::scripts::{macos_write_script, shell_quote};
use super::{is_canceled, unique_temp_path, write_temp_file, WriteOutcome};

/// 系统 hosts 文件路径（平台固定）：macOS 固定 `/etc/hosts`。
pub(super) fn hosts_path() -> PathBuf {
    PathBuf::from("/etc/hosts")
}

pub(super) fn write_hosts(content: &str) -> Result<WriteOutcome, String> {
    let destination = hosts_path();
    let content_file = unique_temp_path("hosts", "txt");
    let staging = destination.with_file_name("hosts.ws-workbench.tmp");
    write_temp_file(&content_file, content, 0o644)?;

    let script_path = unique_temp_path("hosts-elevated", "sh");
    let script = macos_write_script(&content_file, &destination, &staging);
    if let Err(err) = write_temp_file(&script_path, &script, 0o700) {
        let _ = std::fs::remove_file(&content_file);
        return Err(err);
    }

    let command = format!("sh {}", shell_quote(&script_path.to_string_lossy()));
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let (text, ok) = crate::process::run_with_timeout(
        Path::new("/usr/bin/osascript"),
        &["-e", &apple_script],
        std::time::Duration::from_secs(10 * 60),
    );
    let _ = std::fs::remove_file(&script_path);
    let _ = std::fs::remove_file(&content_file);

    if !ok {
        return Err(if is_canceled(&text) {
            "已取消授权".to_string()
        } else {
            format!("写入 hosts 失败：{}", text.trim())
        });
    }
    if !text.contains("WS_WRITE_OK") {
        return Err(format!(
            "写入 hosts 失败：{}",
            if text.trim().is_empty() {
                "提权脚本没有回显成功标记"
            } else {
                text.trim()
            }
        ));
    }
    Ok(WriteOutcome {
        flushed_dns: text.contains("WS_FLUSH_OK"),
    })
}
