//! 其它平台（Linux 等）：沿用系统包管理器 / snap 的常见 svn 路径。
//!
//! 这些平台不是当前发布目标，保持原行为：能找到就用，找不到回退 PATH 里的 `svn`。

use std::path::{Path, PathBuf};

pub(super) fn command_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in ["/usr/bin/svn", "/usr/local/bin/svn", "/snap/bin/svn"] {
        let path = PathBuf::from(raw);
        if path.is_file() {
            out.push(path);
        }
    }
    out
}

pub(super) fn path_argument(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
