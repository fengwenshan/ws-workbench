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
/// 安装列表发生变化的广播：主窗口收到后重新拉列表
pub const EVENT_APPS_CHANGED: &str = "cleaner-apps-changed";
/// 卸载窗口复用时广播：窗口重新拉目标与清单
pub const EVENT_UNINSTALL_TARGET_CHANGED: &str = "cleaner-uninstall-target-changed";

/// 目标应用是否就是当前运行的应用本体（设计 4.2 / 4.3：禁止卸载自己）。
///
/// 平台实现放在各自的 installer 文件里：macOS 比较当前进程所在的 `.app`
/// 与目标 bundle 的 canonical 路径；Windows 比较当前 exe 是否位于目标安装目录。
/// dev 裸二进制（不在 .app 内）在 macOS 上永远返回 false。
pub fn is_self_app(app: &InstalledApp) -> bool {
    #[cfg(target_os = "macos")]
    {
        return app
            .id
            .strip_prefix("mac:")
            .map(|path| super::installer_macos::is_self_bundle(std::path::Path::new(path)))
            .unwrap_or(false);
    }
    #[cfg(target_os = "windows")]
    {
        let current_exe = std::env::current_exe().unwrap_or_default();
        return super::installer_windows::is_self_install(&current_exe, &app.install_location);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app;
        false
    }
}

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
    } else if let Some(app) = hint {
        // 前端已经强制退出过；这里再兜一层，避免「进程还在、应用先被删」。
        let state = super::running::detect(app);
        if state.running {
            return Ok(UninstallOutcome {
                ok: false,
                command: String::new(),
                exit_code: None,
                elevated: false,
                message: format!(
                    "应用仍在运行（{} 个进程），已中止卸载，未删除任何文件",
                    state.processes
                ),
            });
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

/// 扫描关联文件（卸载窗口用）。
///
/// 具体组装在 `related.rs`（平台分派），这里保留同名入口，保证「列表/卸载/
/// 残留/关联文件」都从 `installer` 这一层进（设计 4.1）。
pub fn related_files(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<RelatedFiles, String> {
    super::related::related_files(app_id, hint, emit)
}

/// 删除时对「已不存在 / 不在扫描结果里」的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingPolicy {
    /// 残留删除：路径必须仍在本次扫描结果里且存在，否则记失败
    Fail,
    /// 关联文件删除：卸载程序已删掉、或扫描后消失的路径记 skipped，不算失败
    Skip,
}

/// 删除用户勾选的条目（`delete_leftovers` / `delete_related` 共用）。
///
/// 删除动作按「可信来源」分三类：
/// - 用户目录里的文件 → 进废纸篓（`trash_guard::delete_files`）；
/// - 普通权限注册表键（HKCU）→ 直接删键；
/// - 需要提权的文件 + 注册表键（/Library、HKLM）→ 合并成一次系统授权。
fn delete_selected(
    items: &[LeftoverItem],
    paths: &[String],
    missing: MissingPolicy,
    task: &str,
    emit: &dyn Fn(CleanerProgress),
) -> Result<DeleteOutcome, String> {
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
        let Some(item) = items.iter().find(|item| item.path == *path) else {
            // 「扫描后消失」在关联文件删除里是正常情况（卸载程序已经删了）
            match missing {
                MissingPolicy::Skip => outcome.skipped += 1,
                MissingPolicy::Fail => outcome.failed.push(DeleteFailure {
                    path: (*path).to_string(),
                    error: "不在本次扫描结果中（可能已被删除）".to_string(),
                }),
            }
            continue;
        };
        if !item.exists {
            match missing {
                MissingPolicy::Skip => outcome.skipped += 1,
                MissingPolicy::Fail => outcome.failed.push(DeleteFailure {
                    path: (*path).to_string(),
                    error: "路径已不存在".to_string(),
                }),
            }
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
        task,
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

    let total = outcome.trashed + outcome.skipped + outcome.failed.len();
    emit(CleanerProgress::new(
        task,
        "done",
        total,
        total,
        format!(
            "已删除 {} 项，跳过 {} 项，失败 {} 项",
            outcome.trashed,
            outcome.skipped,
            outcome.failed.len()
        ),
    ));
    Ok(outcome)
}

/// 删除用户勾选的残留。
///
/// 路径白名单是「本次扫描结果」：不在里面的一律记失败（设计 4.4）。
pub fn delete_leftovers(
    app_id: &str,
    hint: Option<&InstalledApp>,
    paths: &[String],
    emit: &dyn Fn(CleanerProgress),
) -> Result<DeleteOutcome, String> {
    // 重新扫描：路径白名单是「本次扫描结果」，不信任前端传回来的任意路径
    let scanned = scan_leftovers(app_id, hint, emit)?;
    delete_selected(&scanned, paths, MissingPolicy::Fail, "scan_leftovers", emit)
}

/// 是否位于废纸篓（`~/.Trash` 前缀，或任一路径组件为 `.Trash`）。
///
/// 关联文件删除永远不碰废纸篓：`trash_guard::trash_path` 对废纸篓内的路径走
/// `remove_permanently`（永久删除），卸载窗口的「全选」若带上这些条目，等于
/// 顺手清空用户废纸篓。扫描侧（`installer_macos::related_files`）已过滤，
/// 这里再兜一层，保证即使白名单被污染也不会永久删除。
fn in_trash(path: &str) -> bool {
    let path = std::path::Path::new(path);
    let under_home_trash = super::home_dir()
        .map(|home| path.starts_with(home.join(".Trash")))
        .unwrap_or(false);
    under_home_trash
        || path
            .components()
            .any(|component| component.as_os_str() == ".Trash")
}

/// 白名单过滤：只放行最近一次关联扫描里的路径，其余记失败。
/// 废纸篓中的条目即使出现在白名单里也拒绝（见 [`in_trash`]）。
fn filter_whitelisted(
    allowed: &std::collections::HashSet<&str>,
    paths: &[String],
) -> (Vec<String>, DeleteOutcome) {
    let mut outcome = DeleteOutcome::default();
    let mut accepted: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for path in paths {
        if !seen.insert(path.as_str()) {
            continue;
        }
        if !allowed.contains(path.as_str()) {
            outcome.failed.push(DeleteFailure {
                path: path.clone(),
                error: "不在本次扫描结果中".to_string(),
            });
        } else if in_trash(path) {
            outcome.failed.push(DeleteFailure {
                path: path.clone(),
                error: "废纸篓中的条目不在关联文件删除范围内".to_string(),
            });
        } else {
            accepted.push(path.clone());
        }
    }
    (accepted, outcome)
}

/// 删除卸载窗口勾选的关联文件（设计 4.4）。
///
/// 两道闸门：
/// 1. `allowed` = 最近一次 `cleaner_related_files` 扫描到的路径（命令层从
///    `CleanerState` 传入）；不在其中的路径直接记失败，绝不删；
/// 2. 删除前重新扫描一遍拿元数据（`requires_elevation` / `kind`）：卸载程序
///    已经把文件删掉时记 `skipped`，不算失败。
pub fn delete_related(
    app_id: &str,
    hint: Option<&InstalledApp>,
    allowed: &[String],
    paths: &[String],
    emit: &dyn Fn(CleanerProgress),
) -> Result<DeleteOutcome, String> {
    let allowed: std::collections::HashSet<&str> =
        allowed.iter().map(String::as_str).collect();
    let (accepted, mut outcome) = filter_whitelisted(&allowed, paths);
    if accepted.is_empty() {
        return Ok(outcome);
    }

    // 重新扫描：应用包可能已被系统卸载程序移走，扫描结果只用来补元数据
    let related = related_files(app_id, hint, emit)?;
    let items: Vec<LeftoverItem> = related
        .groups
        .iter()
        .flat_map(|group| {
            group.items.iter().map(|item| LeftoverItem {
                path: item.path.clone(),
                kind: item.kind.clone(),
                size_bytes: item.size_bytes,
                is_dir: item.is_dir,
                confidence: item.confidence.clone(),
                selected: item.selected,
                requires_elevation: item.requires_elevation,
                exists: item.exists,
            })
        })
        .collect();

    let result = delete_selected(&items, &accepted, MissingPolicy::Skip, "related_files", emit)?;
    outcome.trashed += result.trashed;
    outcome.skipped += result.skipped;
    outcome.failed.extend(result.failed);
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

    #[test]
    fn delete_related_rejects_paths_not_in_scan() {
        let allowed: std::collections::HashSet<&str> =
            ["/Users/me/Library/Caches/com.example.foo"]
                .into_iter()
                .collect();
        let (accepted, outcome) = filter_whitelisted(
            &allowed,
            &[
                "/tmp/evil".to_string(),
                "/Users/me/Library/Caches/com.example.foo".to_string(),
            ],
        );
        assert_eq!(
            accepted,
            vec!["/Users/me/Library/Caches/com.example.foo".to_string()],
            "只放行本次扫描结果里的路径"
        );
        assert_eq!(outcome.failed.len(), 1);
        assert!(outcome.failed[0].error.contains("不在本次扫描结果中"));
        assert_eq!(outcome.skipped, 0);
    }

    #[test]
    fn delete_related_never_accepts_trash_paths_even_if_whitelisted() {
        // 扫描侧已过滤废纸篓，这里验证白名单兜底：即使缓存里混进废纸篓路径，
        // 也只会记失败，不会交给 delete_selected / trash_guard 永久删除。
        let home = super::super::home_dir().expect("测试环境应该有 HOME");
        let trash = home
            .join(".Trash")
            .join("com.example.foo")
            .to_string_lossy()
            .to_string();
        let allowed: std::collections::HashSet<&str> =
            [trash.as_str(), "/Users/me/Library/Caches/com.example.foo"]
                .into_iter()
                .collect();
        let (accepted, outcome) = filter_whitelisted(
            &allowed,
            &[
                trash.clone(),
                "/Users/me/Library/Caches/com.example.foo".to_string(),
            ],
        );
        assert_eq!(
            accepted,
            vec!["/Users/me/Library/Caches/com.example.foo".to_string()],
            "废纸篓路径不能进入删除列表"
        );
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].path, trash);
        assert!(outcome.failed[0].error.contains("废纸篓"));
    }

    #[test]
    fn delete_related_skips_missing_paths() {
        // 重新扫描后这些路径完全不见了（卸载程序已经删掉）→ skipped，不算失败
        let items: Vec<LeftoverItem> = Vec::new();
        let paths = vec!["/Users/me/Library/Caches/com.example.foo".to_string()];
        let emit = |_: CleanerProgress| {};
        let outcome =
            delete_selected(&items, &paths, MissingPolicy::Skip, "related_files", &emit).unwrap();
        assert_eq!(outcome.skipped, 1);
        assert!(outcome.failed.is_empty());
        assert_eq!(outcome.trashed, 0);
    }

    #[test]
    fn delete_leftovers_keeps_failing_missing_paths() {
        // 老流程（残留删除）语义不变：路径消失仍是失败
        let items: Vec<LeftoverItem> = Vec::new();
        let paths = vec!["/tmp/gone".to_string()];
        let emit = |_: CleanerProgress| {};
        let outcome = delete_selected(
            &items,
            &paths,
            MissingPolicy::Fail,
            "scan_leftovers",
            &emit,
        )
        .unwrap();
        assert_eq!(outcome.skipped, 0);
        assert_eq!(outcome.failed.len(), 1);
        assert!(outcome.failed[0].error.contains("不在本次扫描结果中"));
    }
}
