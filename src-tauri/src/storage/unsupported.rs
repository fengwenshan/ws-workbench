//! 未支持平台的占位实现：没有旧数据目录可迁移。
//!
//! 本项目当前只支持 macOS / Windows；其它平台能编译但功能不保证。

use std::path::{Path, PathBuf};

pub(crate) fn legacy_dirs(_home: &Path, _app_data: &Path) -> Vec<PathBuf> {
    Vec::new()
}
