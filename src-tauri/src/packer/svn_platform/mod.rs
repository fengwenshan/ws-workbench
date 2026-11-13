//! svn 命令行定位与路径参数的平台分派。
//!
//! - 候选路径：Windows 上 TortoiseSVN / SlikSVN 默认不写 PATH，macOS 上
//!   Homebrew / MacPorts 也可能只装了链接，各平台列一份常见位置；
//! - 路径参数：Windows 的 `canonicalize` 会带 `\\?\` / `\\?\UNC\` 前缀，
//!   svn.exe 不认，传参前要剥掉；其它平台原样返回。
//!
//! 平台实现见 `macos.rs` / `windows.rs` / `unsupported.rs`，互不引用；
//! 只有本文件的 `#[cfg]` 做分派。

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;

/// 不在 PATH 里时的 svn 可执行文件候选（按平台列）。
pub(crate) fn command_candidates() -> Vec<PathBuf> {
    platform::command_candidates()
}

/// 传给 svn 的路径文本（Windows 剥掉扩展长度前缀）。
pub(crate) fn path_argument(path: &Path) -> String {
    platform::path_argument(path)
}

/// Windows `canonicalize` 产生的扩展长度前缀（`\\?\`、`\\?\UNC\`）剥掉。
///
/// 纯字符串逻辑放在共享层，跨平台单测；只有 Windows 平台实现会调用。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn strip_verbatim_prefix(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{}", rest);
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_verbatim_prefix_handles_drive_and_unc() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\work\repo"), r"C:\work\repo");
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\server\share\repo"),
            r"\\server\share\repo"
        );
        // 没有前缀时原样返回，不能误伤普通路径
        assert_eq!(strip_verbatim_prefix("/Users/me/repo"), "/Users/me/repo");
    }
}
