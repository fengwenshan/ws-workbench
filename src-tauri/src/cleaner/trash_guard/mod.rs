//! 删除安全层：路径白名单 + 废纸篓 + 提权批处理 + 注册表键删除。
//!
//! 设计 4.4 的硬约束都在这里落地：
//! 1. **只进废纸篓**（`trash` crate），不做永久删除；只有需要 root/管理员权限的
//!    条目例外 —— 那些路径普通权限根本删不掉，提权后用 root 的废纸篓没有意义，
//!    所以按设计明确「提权批处理里用 rm / Remove-Item」。
//! 2. 删除前逐条过白名单：绝对路径、无 `..`、不是根/主目录、且在平台白名单根**之下**。
//! 3. 需要提权的条目合并成一次系统授权；失败逐条回传，不影响其他条目。
//!
//! 本文件只放平台无关的白名单/废纸篓/编排逻辑；提权脚本与注册表删除在
//! `macos.rs` / `windows.rs`，两个平台文件互不引用。

use std::path::{Path, PathBuf};

use super::models::{DeleteFailure, DeleteOutcome};
use super::registry_id::{validate_registry_key_path, HIVE_HKCU, HIVE_HKLM, HIVE_HKLM_WOW64};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

mod scripts;

#[cfg(target_os = "windows")]
pub use windows::delete_registry_key;

// ============================== 白名单 ==============================

/// 路径是否允许删除。
///
/// 这里是所有删除动作的最后一道闸门：扫描规则/前端勾选都可能出错，但白名单
/// 只认「平台规定的具体目录」，绝不接受整个用户主目录。
pub fn is_allowed(path: &Path) -> bool {
    if !is_safe_shape(path) {
        return false;
    }
    allowed_roots()
        .iter()
        .any(|root| is_strictly_under(path, root))
}

/// 路径本身是否安全（与平台无关的那部分）：
/// 绝对路径、不含 `..`、不是文件系统根、不是用户主目录、不在硬拒名单里。
fn is_safe_shape(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    // 根目录没有文件名（`/`、`C:\` 都是）
    if path.file_name().is_none() {
        return false;
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }
    if let Some(home) = super::home_dir() {
        if path == home {
            return false;
        }
    }
    !is_denied(path)
}

/// 硬拒绝的目录：即使将来白名单写错，这些也永远删不得。平台清单在平台文件里。
fn is_denied(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::is_denied(path)
    }
    #[cfg(target_os = "windows")]
    {
        windows::is_denied(path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        false
    }
}

/// 严格「在 root 之下」：等于 root 本身不算（避免把 /Applications 整个删掉）。
fn is_strictly_under(path: &Path, root: &Path) -> bool {
    path != root && path.starts_with(root)
}

/// 平台白名单根（设计 4.4）；平台差异在 `macos` / `windows` 里。
fn allowed_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos::allowed_roots()
    }
    #[cfg(target_os = "windows")]
    {
        windows::allowed_roots()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

// ============================== 废纸篓删除 ==============================

/// 单条删除进废纸篓（先过白名单，再调 trash）。
///
/// `~/.Trash` 里的条目是例外：它本来就躺在废纸篓里，再「移入废纸篓」只会失败
/// 或原地不动，所以这里按「从废纸篓里真正删除」处理。这条规则只对扫描出来的
/// 精确匹配项开放（`~/.Trash` 在白名单里但没有列入残留规则的话不会被勾选）。
pub fn trash_path(path: &Path) -> Result<(), String> {
    if !is_allowed(path) {
        return Err(format!("拒绝删除白名单之外的路径: {}", path.display()));
    }
    if is_in_user_trash(path) {
        return remove_permanently(path);
    }
    move_to_trash(path).map_err(|err| format!("移入废纸篓失败: {}", err))
}

/// 把路径移入废纸篓。
///
/// macOS 上 `trash` crate 默认走 Finder（`osascript`）：会触发「控制 Finder」的
/// 自动化授权弹窗、有删除音效，未授权时直接失败。`NSFileManager` 的
/// `trashItemAtURL` 不需要额外授权、更快，所以 macOS 固定用这个实现
/// （trash crate 的 `DeleteMethod::NsFileManager`）；其他平台用默认实现。
fn move_to_trash(path: &Path) -> Result<(), trash::Error> {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut context = trash::TrashContext::new();
        context.set_delete_method(DeleteMethod::NsFileManager);
        return context.delete(path);
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete(path)
    }
}

/// 路径是否在 `~/.Trash` 下
fn is_in_user_trash(path: &Path) -> bool {
    super::home_dir()
        .map(|home| path.starts_with(home.join(".Trash")))
        .unwrap_or(false)
}

/// 从废纸篓里真正删除（文件/目录都处理，符号链接只删链接本身）
fn remove_permanently(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|err| format!("读取失败: {}", err))?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path).map_err(|err| format!("永久删除目录失败: {}", err))
    } else {
        std::fs::remove_file(path).map_err(|err| format!("永久删除文件失败: {}", err))
    }
}

/// 普通权限批量删除（逐条进废纸篓），失败逐条记录。
pub fn delete_files(paths: &[PathBuf]) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    for path in paths {
        match trash_path(path) {
            Ok(()) => outcome.trashed += 1,
            Err(error) => outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error,
            }),
        }
    }
    outcome
}

/// 提权批量删除：文件 + 注册表键合并成**一次**系统授权。
///
/// 一次操作只弹一次授权框（设计 4.4）。校验全部放在生成脚本之前：
/// 绝不能把未校验的路径/键写进 root 脚本 —— 提权脚本的每一行都等价于
/// 管理员手工执行，白名单在这里是最后一道闸门。
pub fn delete_elevated(files: &[PathBuf], registry_keys: &[(String, String)]) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    let mut allowed_files: Vec<PathBuf> = Vec::new();
    for path in files {
        if is_allowed(path) {
            allowed_files.push(path.clone());
        } else {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: "拒绝删除白名单之外的路径".to_string(),
            });
        }
    }
    let mut allowed_keys: Vec<(String, String)> = Vec::new();
    for (hive, key_path) in registry_keys {
        match validate_registry_key_path(key_path) {
            Ok(normalized) if [HIVE_HKLM, HIVE_HKLM_WOW64, HIVE_HKCU].contains(&hive.as_str()) => {
                allowed_keys.push((hive.clone(), normalized));
            }
            Ok(_) => outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error: format!("未知注册表根: {}", hive),
            }),
            Err(error) => outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error,
            }),
        }
    }
    if allowed_files.is_empty() && allowed_keys.is_empty() {
        return outcome;
    }

    #[cfg(target_os = "macos")]
    {
        let result = macos::delete_files_elevated(&allowed_files, &allowed_keys);
        outcome.trashed += result.trashed;
        outcome.failed.extend(result.failed);
    }
    #[cfg(target_os = "windows")]
    {
        let result = windows::delete_files_elevated(&allowed_files, &allowed_keys);
        outcome.trashed += result.trashed;
        outcome.failed.extend(result.failed);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        for path in &allowed_files {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: "当前平台不支持提权删除".to_string(),
            });
        }
        for (hive, key_path) in &allowed_keys {
            outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error: "当前平台不支持注册表删除".to_string(),
            });
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_system_paths() {
        assert!(!is_allowed(Path::new("/System/Library/Foo")));
        assert!(!is_allowed(Path::new("/usr/bin/foo")));
        assert!(!is_allowed(Path::new("/")));
        assert!(!is_allowed(Path::new("relative/path")));
        assert!(!is_allowed(Path::new(
            "/Applications/../System/Library/Foo"
        )));
    }

    // `/Applications`、`~/Library` 这些是 macOS 的路径规则；Windows 的白名单
    // 在 `is_allowed` 的 Windows 分支里单独判断，这条测试只在 macOS 跑。
    #[cfg(target_os = "macos")]
    #[test]
    fn allows_application_bundles_and_user_library() {
        assert!(is_allowed(Path::new("/Applications/Foo.app")));
        let home = super::super::home_dir().expect("测试环境应该有 HOME");
        assert!(is_allowed(&home.join("Library/Caches/com.example.foo")));
        assert!(is_allowed(
            &home.join("Library/Preferences/com.example.foo.plist")
        ));
        assert!(is_allowed(&home.join("Library/Application Support/Foo")));
        assert!(is_allowed(&home.join("Applications/Foo.app")));
        assert!(is_allowed(Path::new(
            "/Library/LaunchDaemons/com.example.foo.plist"
        )));
        // 关联文件扫描的兜底目录：应用副本可能被放在桌面/下载/共享目录
        assert!(is_allowed(&home.join("Desktop/Foo.app")));
        assert!(is_allowed(&home.join("Downloads/Foo.app")));
        assert!(is_allowed(Path::new("/Users/Shared/Foo.app")));
        // 但白名单根自身（桌面/下载目录）永远不能删
        assert!(!is_allowed(&home.join("Desktop")));
        assert!(!is_allowed(&home.join("Downloads")));
        assert!(!is_allowed(Path::new("/Users/Shared")));
    }

    #[test]
    fn rejects_home_and_application_root_themselves() {
        let home = super::super::home_dir().expect("测试环境应该有 HOME");
        assert!(!is_allowed(&home), "整个主目录永远不能删");
        assert!(
            !is_allowed(Path::new("/Applications")),
            "白名单根自身不能删"
        );
        assert!(
            !is_allowed(&home.join("Library/Caches")),
            "白名单根自身不能删"
        );
    }

    #[test]
    fn trash_rejects_paths_outside_whitelist() {
        let err = trash_path(Path::new("/System/Library/CoreServices/Finder.app")).unwrap_err();
        assert!(err.contains("白名单"), "实际错误: {}", err);
    }

    /// PowerShell 的转义是「单引号翻倍」，不是 POSIX 的 `'\''`。
    /// 这条测试防回归：写错会直接影响 Windows 提权删除（含命令注入）。
    #[test]
    fn powershell_quote_doubles_single_quotes() {
        assert_eq!(super::scripts::powershell_quote("plain"), "'plain'");
        assert_eq!(
            super::scripts::powershell_quote(r"C:\Bob's Tool\a.exe"),
            r"'C:\Bob''s Tool\a.exe'"
        );
        // 注入尝试：整段必须留在单引号字符串内部
        assert_eq!(
            super::scripts::powershell_quote("x'; Remove-Item C:\\ -Recurse; '"),
            "'x''; Remove-Item C:\\ -Recurse; '''"
        );
        // 不变量：原串里每个单引号都变成一对，不会留下能闭合字符串的单个引号
        let quoted = super::scripts::powershell_quote("a'b'c");
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert_eq!(quoted.matches("''").count(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn posix_shell_quote_keeps_apostrophe_literal() {
        assert_eq!(super::macos::shell_quote("it's"), r"'it'\''s'");
    }
}
