//! 非 macOS / Windows 平台的占位实现：读路径沿用 `/etc/hosts`，写入直接失败。
//!
//! 本项目当前只支持 macOS / Windows；其它平台能编译但功能不保证。

use std::path::PathBuf;

use super::WriteOutcome;

/// 系统 hosts 文件路径：与原来的非 Windows 行为一致，沿用 `/etc/hosts`。
pub(super) fn hosts_path() -> PathBuf {
    PathBuf::from("/etc/hosts")
}

pub(super) fn write_hosts(_content: &str) -> Result<WriteOutcome, String> {
    Err("当前平台暂不支持写入 hosts".to_string())
}
