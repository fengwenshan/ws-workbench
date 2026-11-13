//! Windows：TortoiseSVN / SlikSVN / VisualSVN 默认不写 PATH，按常见安装位置找。

use std::path::{Path, PathBuf};

use super::strip_verbatim_prefix;

pub(super) fn command_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "LOCALAPPDATA"]
        .iter()
        .filter_map(|name| std::env::var_os(name).map(PathBuf::from))
        .collect();
    for root in roots {
        for relative in [
            r"TortoiseSVN\bin\svn.exe",
            r"SlikSvn\bin\svn.exe",
            r"VisualSVN Server\bin\svn.exe",
            r"VisualSVN\bin\svn.exe",
        ] {
            let path = root.join(relative);
            if path.is_file() {
                out.push(path);
            }
        }
    }
    out
}

pub(super) fn path_argument(path: &Path) -> String {
    strip_verbatim_prefix(&path.to_string_lossy())
}
