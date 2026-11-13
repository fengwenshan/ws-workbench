//! 平台无关的路径判定。
//!
//! 这里只放不依赖任何平台 API 的判断逻辑，两个平台和 macOS 上的单测都用同一份。
//! 注意：`trash_guard::is_in_user_trash` 故意不合并进来 —— 它只认 `~/.Trash`
//! 前缀（决定是否永久删除），而这里的 `is_trash_path` 还把任意层级的 `.Trash`
//! 组件也算作废纸篓（用于从清单里过滤掉回收站条目），两者语义不同。

use std::path::Path;

/// 是否位于废纸篓：`~/.Trash` 前缀，或任一路径组件为 `.Trash`。
///
/// 关联文件清单绝不列废纸篓条目：`trash_guard::trash_path` 对废纸篓内的路径走
/// `remove_permanently`（永久删除），而卸载窗口的「全选」会把这些条目送进删除
/// 命令 —— 等于顺手清空用户废纸篓，不属于「卸载」语义。扫描侧和删除侧都用
/// 这个函数兜一层。
pub(crate) fn is_trash_path(path: &Path) -> bool {
    let under_home_trash = super::home_dir()
        .map(|home| path.starts_with(home.join(".Trash")))
        .unwrap_or(false);
    under_home_trash
        || path
            .components()
            .any(|component| component.as_os_str() == ".Trash")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_home_trash_and_trash_component() {
        let home = super::super::home_dir().expect("测试环境应该有 HOME");
        assert!(is_trash_path(&home.join(".Trash/Foo.app")));
        assert!(is_trash_path(Path::new("/Volumes/Backup/.Trash/Foo.app")));
        assert!(!is_trash_path(Path::new("/Applications/Foo.app")));
        assert!(!is_trash_path(&home.join("Library/Caches/com.example.foo")));
        // 名字只是以 .Trash 开头的正常目录不算
        assert!(!is_trash_path(Path::new("/Applications/.Trashy/Foo.app")));
    }
}
