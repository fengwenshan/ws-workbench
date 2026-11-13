//! macOS：Homebrew（Apple Silicon / Intel）/ MacPorts / 系统自带的 svn 位置。

use std::path::{Path, PathBuf};

pub(super) fn command_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in ["/opt/homebrew/bin/svn", "/usr/local/bin/svn", "/usr/bin/svn"] {
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
