//! macOS 提权删除实现：白名单根、硬拒名单、osascript 一次授权。

use std::path::{Path, PathBuf};

use super::super::home_dir;
use super::super::models::{DeleteFailure, DeleteOutcome};

/// 硬拒绝目录（macOS）。
pub(super) fn is_denied(path: &Path) -> bool {
    const DENIED: &[&str] = &[
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/private",
        "/dev",
        "/etc",
        "/var",
        "/Library/Apple",
    ];
    DENIED.iter().any(|denied| path.starts_with(denied))
}

/// macOS 白名单根（设计 4.4）。
pub(super) fn allowed_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    roots.push(PathBuf::from("/Applications"));
    if let Some(home) = home_dir() {
        roots.push(home.join("Applications"));
        for sub in [
            "Application Support",
            "Caches",
            "Preferences",
            "Logs",
            "Saved Application State",
            "Containers",
            "Group Containers",
            "HTTPStorages",
            "WebKit",
            "Cookies",
            "Application Scripts",
            "LaunchAgents",
            // 设计 4.0.1（柠檬解析）：用户级插件/启动项/日志目录
            "PreferencePanes",
            "LaunchDaemons",
        ] {
            roots.push(home.join("Library").join(sub));
        }
        // 废纸篓里被精确匹配到的条目：本来就在废纸篓里，删除=真正删除，
        // 所以单独放开（见 trash_path 里的说明）
        roots.push(home.join(".Trash"));
        // 卸载窗口会列出「同一个应用被复制到桌面/下载」的其它副本
        // （design 4.2 的兜底扫描目录）。这些副本只有出现在本次关联文件扫描
        // 结果里才会被勾选删除，白名单仍只认具体目录，主目录本身依旧拒绝。
        roots.push(home.join("Desktop"));
        roots.push(home.join("Downloads"));
    }
    // 同上，/Users/Shared 是多用户共享目录，也只放开「本次扫描命中」的副本
    roots.push(PathBuf::from("/Users/Shared"));
    for sub in [
        "Application Support",
        "Caches",
        "Preferences",
        "LaunchAgents",
        "LaunchDaemons",
        "PrivilegedHelperTools",
        // 设计 4.0.1（柠檬解析）：系统级扩展/插件/启动项目录
        "Extensions",
        "Filesystems",
        "PreferencePanes",
        "StartupItems",
    ] {
        roots.push(PathBuf::from("/Library").join(sub));
    }
    roots
}

/// shell 单引号转义（POSIX）：`it's` → `'it'\''s'`。只给 macOS 提权脚本用。
pub(super) fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub(super) fn delete_files_elevated(
    paths: &[PathBuf],
    registry_keys: &[(String, String)],
) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    // macOS 没有注册表；调用方不该传，传了也按失败回传，绝不静默丢弃
    for (hive, key_path) in registry_keys {
        outcome.failed.push(DeleteFailure {
            path: format!("win:{}|{}", hive, key_path),
            error: "macOS 不支持注册表删除".to_string(),
        });
    }

    // 逐行 `rm -rf`，每行回显 OK/FAIL 前缀，osascript 会把脚本的 stdout 带回来，
    // 于是不需要一个 root 可写的临时结果文件。
    let mut script = String::from("#!/bin/sh\n");
    for path in paths {
        let quoted = shell_quote(&path.to_string_lossy());
        script.push_str(&format!(
            "if /bin/rm -rf -- {quoted} 2>/dev/null; then echo \"CLEANER_OK\t{quoted}\"; \
             else echo \"CLEANER_FAIL\t{quoted}\"; fi\n"
        ));
    }

    let script_path = std::env::temp_dir().join(format!(
        "ws-workbench-cleaner-elevated-{}.sh",
        uuid::Uuid::new_v4()
    ));
    if let Err(err) = std::fs::write(&script_path, script) {
        for path in paths {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: format!("生成提权脚本失败: {}", err),
            });
        }
        return outcome;
    }

    let command = format!("sh {}", shell_quote(&script_path.to_string_lossy()));
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let (text, ok) = crate::process::run_with_timeout(
        Path::new("/usr/bin/osascript"),
        &["-e", &apple_script],
        std::time::Duration::from_secs(10 * 60),
    );
    let _ = std::fs::remove_file(&script_path);

    if !ok {
        let canceled = text.to_ascii_lowercase().contains("cancel")
            || text.contains("已取消")
            || text.contains("-128");
        let error = if canceled {
            "已取消授权".to_string()
        } else {
            format!("提权删除失败: {}", text.trim())
        };
        for path in paths {
            outcome.failed.push(DeleteFailure {
                path: path.to_string_lossy().to_string(),
                error: error.clone(),
            });
        }
        return outcome;
    }

    // 解析回显；脚本里 rm 成功即 OUTPUT 中有 OK 行
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("CLEANER_OK\t") {
            outcome.trashed += 1;
            seen.insert(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("CLEANER_FAIL\t") {
            outcome.failed.push(DeleteFailure {
                path: rest.to_string(),
                error: "提权删除失败（文件被占用或权限不足）".to_string(),
            });
            seen.insert(rest.to_string());
        }
    }
    // 输出解析不出来时不要让用户误以为「一条都没删」，按成功计数兜底
    if seen.is_empty() {
        outcome.trashed = paths.len();
    }
    outcome
}
