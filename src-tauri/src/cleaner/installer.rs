//! 列表 / 卸载 / 残留的统一入口。
//!
//! 平台差异都下沉到 `installer_windows` / `installer_macos`；这里只做三件事：
//! 1. 按 app id 前缀分派；
//! 2. 删除前重新扫描一遍，**只允许删本次扫描结果里的条目**（设计 4.4 的
//!    「必须是扫描时列出的键/路径」），避免前端传任意路径进来；
//! 3. 把文件删除与注册表删除合并排序，需要提权的合并成一次授权。

use std::path::PathBuf;

use super::models::*;
use super::trash_guard::{self, split_registry_id};

/// 进度事件名（前端 `listen('cleaner-progress')`）
pub const EVENT_PROGRESS: &str = "cleaner-progress";

/// 已安装应用列表
pub fn list_apps(emit: &(dyn Fn(CleanerProgress) + Sync)) -> Result<Vec<InstalledApp>, String> {
    #[cfg(target_os = "macos")]
    {
        return Ok(super::installer_macos::list_apps(emit));
    }
    #[cfg(target_os = "windows")]
    {
        return super::installer_windows::list_apps(emit);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = emit;
        Err("当前平台不支持已安装应用扫描".to_string())
    }
}

/// 退出失败时返回给前端的统一结果（弹窗里展示原因，不执行卸载）。
fn quit_failure_outcome(message: String) -> UninstallOutcome {
    UninstallOutcome {
        ok: false,
        command: String::new(),
        exit_code: None,
        elevated: false,
        message,
    }
}

/// 卸载指定应用。
///
/// `quit_running=true` 时先检测并优雅退出应用：退不掉直接返回失败结果，
/// 不执行卸载（避免边跑边删导致缓存被重建 / 文件被占用）。
pub fn uninstall(
    app_id: &str,
    hint: Option<&InstalledApp>,
    quit_running: bool,
    emit: &dyn Fn(CleanerProgress),
) -> Result<UninstallOutcome, String> {
    if quit_running {
        if let Some(app) = hint {
            if let Err(error) = super::running::quit(app, emit) {
                return Ok(quit_failure_outcome(error));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        return super::installer_macos::uninstall(app_id, hint, emit);
    }
    #[cfg(target_os = "windows")]
    {
        return super::installer_windows::uninstall(app_id, hint, emit);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (app_id, hint, emit);
        Err("当前平台不支持卸载".to_string())
    }
}

/// 扫描残留
pub fn scan_leftovers(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<Vec<LeftoverItem>, String> {
    #[cfg(target_os = "macos")]
    {
        return super::installer_macos::scan_leftovers(app_id, hint, emit);
    }
    #[cfg(target_os = "windows")]
    {
        return super::installer_windows::scan_leftovers(app_id, hint, emit);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (app_id, hint, emit);
        Err("当前平台不支持残留扫描".to_string())
    }
}

/// 删除用户勾选的残留。
///
/// 删除动作按「可信来源」分三类：
/// - 用户目录里的文件 → 进废纸篓（`trash_guard::delete_files`）；
/// - 普通权限注册表键（HKCU）→ 直接删键；
/// - 需要提权的文件 + 注册表键（/Library、HKLM）→ 合并成一次系统授权。
pub fn delete_leftovers(
    app_id: &str,
    hint: Option<&InstalledApp>,
    paths: &[String],
    emit: &dyn Fn(CleanerProgress),
) -> Result<DeleteOutcome, String> {
    // 重新扫描：路径白名单是「本次扫描结果」，不信任前端传回来的任意路径
    let scanned = scan_leftovers(app_id, hint, emit)?;
    let requested: Vec<&str> = {
        let mut seen = std::collections::HashSet::new();
        paths
            .iter()
            .map(|path| path.as_str())
            .filter(|path| seen.insert(*path))
            .collect()
    };
    if requested.is_empty() {
        return Ok(DeleteOutcome::default());
    }

    let mut outcome = DeleteOutcome::default();
    let mut plain_files: Vec<PathBuf> = Vec::new();
    let mut elevated_files: Vec<PathBuf> = Vec::new();
    let mut plain_keys: Vec<(String, String)> = Vec::new();
    let mut elevated_keys: Vec<(String, String)> = Vec::new();

    for path in &requested {
        let Some(item) = scanned.iter().find(|item| item.path == *path) else {
            outcome.failed.push(DeleteFailure {
                path: (*path).to_string(),
                error: "不在本次扫描结果中（可能已被删除）".to_string(),
            });
            continue;
        };
        if !item.exists {
            outcome.failed.push(DeleteFailure {
                path: (*path).to_string(),
                error: "路径已不存在".to_string(),
            });
            continue;
        }
        // pkg 收据本版本只读展示（设计 4.3），任何情况下都不删
        if item.kind == "receipt" {
            outcome.failed.push(DeleteFailure {
                path: (*path).to_string(),
                error: "pkg 安装收据只读展示，不自动删除".to_string(),
            });
            continue;
        }

        match split_registry_id(&item.path) {
            Some((hive, key_path)) => {
                let entry = (hive.to_string(), key_path.to_string());
                if item.requires_elevation {
                    elevated_keys.push(entry);
                } else {
                    plain_keys.push(entry);
                }
            }
            None => {
                let target = PathBuf::from(&item.path);
                if item.requires_elevation {
                    elevated_files.push(target);
                } else {
                    plain_files.push(target);
                }
            }
        }
    }

    emit(CleanerProgress::new(
        "scan_leftovers",
        "running",
        0,
        requested.len(),
        format!("正在删除 {} 项…", plain_files.len() + elevated_files.len()),
    ));

    // 普通权限：文件进废纸篓
    if !plain_files.is_empty() {
        let result = trash_guard::delete_files(&plain_files);
        outcome.trashed += result.trashed;
        outcome.failed.extend(result.failed);
    }

    // 普通权限：HKCU 注册表键直接删
    #[cfg(target_os = "windows")]
    for (hive, key_path) in &plain_keys {
        match trash_guard::delete_registry_key(hive, key_path) {
            Ok(()) => outcome.trashed += 1,
            Err(error) => outcome.failed.push(DeleteFailure {
                path: format!("win:{}|{}", hive, key_path),
                error,
            }),
        }
    }
    #[cfg(not(target_os = "windows"))]
    for (hive, key_path) in &plain_keys {
        outcome.failed.push(DeleteFailure {
            path: format!("win:{}|{}", hive, key_path),
            error: "当前平台不支持注册表删除".to_string(),
        });
    }

    // 需要提权的文件和注册表键合并成一次授权
    if !elevated_files.is_empty() || !elevated_keys.is_empty() {
        let result = trash_guard::delete_elevated(&elevated_files, &elevated_keys);
        outcome.trashed += result.trashed;
        outcome.failed.extend(result.failed);
    }

    emit(CleanerProgress::new(
        "scan_leftovers",
        "done",
        requested.len(),
        requested.len(),
        format!(
            "已删除 {} 项，失败 {} 项",
            outcome.trashed,
            outcome.failed.len()
        ),
    ));
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_failure_becomes_failed_outcome() {
        let outcome = quit_failure_outcome("「Foo」仍有 1 个进程未退出".to_string());
        assert!(!outcome.ok);
        assert_eq!(outcome.exit_code, None);
        assert!(outcome.message.contains("未退出"));
    }
}
