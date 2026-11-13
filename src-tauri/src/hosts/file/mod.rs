//! 系统 hosts 文件：路径、读取、提权写入、刷新 DNS（在线编辑保存 / 还原共用）。
//!
//! 路径固定（设计第 5 节，不接受前端传路径）：
//! - macOS：`/etc/hosts`
//! - Windows：`%SystemRoot%\System32\drivers\etc\hosts`（取不到 `SystemRoot` 时用 `C:\Windows` 兜底）
//!
//! 写入走一次系统授权（设计 4.3）：
//! - macOS：生成 sh 脚本 → `osascript -e 'do shell script "sh <quoted>" with administrator privileges'`
//!   脚本内 `cp` 到 `/etc/hosts.ws-workbench.tmp` → `chmod 644` → `mv -f` 原子替换 → 刷新 DNS；
//! - Windows：生成 ps1 脚本 → `Start-Process -Verb RunAs -Wait` 提权 PowerShell
//!   `Copy-Item` 覆盖 hosts → `ipconfig /flushdns`。
//!
//! 脚本里所有插值路径都必须转义：macOS 用 POSIX 单引号，Windows 用 PowerShell 的 `''`。
//! 这两条都有单测（脚本文本生成在 `scripts.rs`，所有平台都编译）。
//!
//! 本文件只放平台无关的读取 / 校验 / 提权编排与小工具：平台实现在 `macos.rs` /
//! `windows.rs`，两个平台文件互不引用；其它平台由 `unsupported.rs` 兜底。

use std::path::{Path, PathBuf};

use super::MAX_CONTENT_LEN;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

mod scripts;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;
#[cfg(target_os = "windows")]
use windows as platform;

/// 提权写入的结果
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteOutcome {
    /// DNS 缓存是否刷新成功；失败不回滚 hosts（内容已生效），只如实报告
    pub flushed_dns: bool,
}

/// 系统 hosts 文件路径（平台固定）。
pub fn hosts_path() -> PathBuf {
    platform::hosts_path()
}

/// hosts 文件所在目录（`hosts_open_file_dir` 用）。
pub fn hosts_dir() -> PathBuf {
    hosts_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/etc"))
}

/// hosts 文件是否存在。
pub fn hosts_exists() -> bool {
    hosts_path().exists()
}

/// 按原始字节读取当前 hosts；文件不存在返回空字节（首次应用时从空文件开始）。
pub fn read_hosts_bytes() -> Result<Vec<u8>, String> {
    match std::fs::read(hosts_path()) {
        Ok(bytes) => Ok(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("读取 hosts 失败：{err}")),
    }
}

/// 读当前 hosts 文本。
///
/// 非 UTF-8 时直接报错而不是 lossy 替换：预览 / 导入可以降级展示，但**写入**路径
/// 必须拿原文做「块外内容原样保留」的合并，替换字符会把块外内容写坏，所以宁可
/// 让用户先手动转码，也不能静默改坏系统文件。
pub fn read_hosts() -> Result<String, String> {
    let bytes = read_hosts_bytes()?;
    String::from_utf8(bytes).map_err(|_| {
        "hosts 文件不是 UTF-8 编码，暂不支持自动编辑（避免写坏原有内容）；请先手动把文件转成 UTF-8"
            .to_string()
    })
}

/// 提权把 `content` 写入系统 hosts 并刷新 DNS。
pub fn write_hosts(content: &str) -> Result<WriteOutcome, String> {
    if content.len() > MAX_CONTENT_LEN {
        return Err(format!(
            "生成内容超过 1 MiB 上限（当前 {} 字节）",
            content.len()
        ));
    }
    platform::write_hosts(content)
}

// ============================== 平台无关小工具 ==============================

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ws-workbench-{prefix}-{}.{extension}",
        uuid::Uuid::new_v4()
    ))
}

/// 写临时文件并尽量收紧权限：脚本里含 root 操作路径，不该让其它用户改。
fn write_temp_file(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    std::fs::write(path, content).map_err(|err| format!("写入临时文件失败：{err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    Ok(())
}

/// 判断 osascript / UAC 的错误输出是否为「用户取消授权」。
fn is_canceled(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("cancel")
        || lower.contains("user canceled")
        || text.contains("已取消")
        || text.contains("-128")
        // Windows ERROR_CANCELLED：用户点了 UAC 的「否」
        || lower.contains("1223")
}

// ============================== 单测 ==============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_path_is_fixed_per_platform() {
        #[cfg(target_os = "macos")]
        assert_eq!(hosts_path(), PathBuf::from("/etc/hosts"));
        #[cfg(target_os = "windows")]
        {
            let path = hosts_path();
            let text = path.to_string_lossy().to_ascii_lowercase();
            assert!(text.ends_with(r"\system32\drivers\etc\hosts"), "{text}");
        }
        // 目录用于 hosts_open_file_dir，必须是 hosts 的父目录
        assert_eq!(hosts_dir(), hosts_path().parent().expect("parent"));
    }

    #[test]
    fn cancel_detection_covers_osascript_and_uac() {
        assert!(is_canceled("execution error: User canceled. (-128)"));
        assert!(is_canceled("用户已取消"));
        assert!(is_canceled("ERROR_CANCELLED 1223"));
        assert!(!is_canceled("Permission denied"));
    }

    #[test]
    fn limits_are_sane() {
        assert_eq!(MAX_CONTENT_LEN, 1024 * 1024);
    }
}
