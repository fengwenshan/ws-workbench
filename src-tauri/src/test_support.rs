//! 测试专用工具（只在 `cargo test` 里编译）。
//!
//! 旧测试直接用 `std::env::temp_dir()` 拼固定 / PID 目录名，收尾靠测试末尾手动
//! `remove_dir_all`：一旦断言 panic，收尾代码就跳过，反复跑测试会在系统临时目录里
//! 堆出几百个 `ws-*` 目录。`TempDir` 用 RAII 保证离开作用域（含 panic 展开）时清理。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// 测试用临时目录：构造时创建，Drop 时递归删除。
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// 以 `prefix` 建一个本轮测试内唯一、互不冲突的临时目录。
    pub(crate) fn new(prefix: &str) -> Self {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ws-test-{}-{}-{}",
            std::process::id(),
            seq,
            prefix
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("创建测试临时目录失败");
        Self { path }
    }

    /// 目录绝对路径。
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// 拼子路径（不自动创建，按测试自己的需要处理）。
    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
