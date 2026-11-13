//! macOS 的旧数据目录候选。
//!
//! 标准数据目录由 Tauri `app_data_dir` 决定（`~/Library/Application Support/com.ws.workbench`），
//! 不在这里写死；本文件只列「旧版本可能用过、需要迁移」的目录。
//!
//! 平台验证状态：本文件只在 macOS 上编译，macOS CI（test-macos）验证。

use std::path::{Path, PathBuf};

/// 旧版本可能存数据的目录：
/// - `~/Library/Application Support/ws-workbench`：旧版按产品名建的数据目录；
/// - `~/.ws-workbench`：更早的主目录点目录，也是受限环境下的兜底目录；
/// - `app_data_dir` 的兄弟目录 `ws-workbench`：identifier 变化后可能留下的旧目录。
pub(crate) fn legacy_dirs(home: &Path, app_data: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        home.join("Library")
            .join("Application Support")
            .join("ws-workbench"),
        home.join(".ws-workbench"),
    ];
    if let Some(parent) = app_data.parent() {
        dirs.push(parent.join("ws-workbench"));
    }
    // 去重并排除标准目录本身
    dirs.retain(|dir| dir != app_data);
    dirs.dedup();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_macos_legacy_dirs() {
        let home = Path::new("/Users/me");
        let app_data = Path::new("/Users/me/Library/Application Support/com.ws.workbench");
        let dirs = legacy_dirs(home, app_data);
        assert!(dirs.contains(&PathBuf::from(
            "/Users/me/Library/Application Support/ws-workbench"
        )));
        assert!(dirs.contains(&PathBuf::from("/Users/me/.ws-workbench")));
        assert!(!dirs.contains(&app_data.to_path_buf()), "标准目录不应作为旧目录");
    }
}
