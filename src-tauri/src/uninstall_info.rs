//! 安装包卸载程序使用的路径清单。
//!
//! 只写本地路径，不含密钥/配置内容；卸载脚本按行读取，不做 `eval`。
//! 目的是让卸载程序能准确显示用户自定义的附件目录，而不是猜默认路径。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::models::AppSettings;

/// 清单文件名（各数据目录各放一份）
pub(crate) const FILE_NAME: &str = "uninstall-info.txt";
const VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UninstallInfo {
    pub app_bundle: String,
    pub data_dirs: Vec<String>,
    pub upload_dir: String,
    pub upload_custom: bool,
    pub db_path: String,
    pub clipboard_dir: String,
    pub cache_dirs: Vec<String>,
}

/// 从 `.../Foo.app/Contents/MacOS/Foo` 找到 `.../Foo.app`。
pub(crate) fn app_bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

/// App 可能用到的缓存/WebView 目录（只返回候选，不判断存在）。
///
/// Windows 上这些 macOS 路径不会存在，`collect` 过滤后自然为空。
fn cache_candidates(home: &Path) -> Vec<PathBuf> {
    let library = home.join("Library");
    vec![
        library.join("Caches").join("com.ws.workbench"),
        library.join("Caches").join("ws-workbench"),
        library.join("WebKit").join("com.ws.workbench"),
        library.join("WebKit").join("ws-workbench"),
        library.join("HTTPStorages").join("com.ws.workbench"),
        library
            .join("Saved Application State")
            .join("com.ws.workbench.savedState"),
        library.join("Preferences").join("com.ws.workbench.plist"),
        library.join("Logs").join("ws-workbench"),
    ]
}

/// 只把用户主目录下的路径当成 App 自有数据；避免把项目内开发兜底目录
/// 或用户共享目录写进卸载清单。
fn is_owned_path(path: &Path, home: Option<&Path>) -> bool {
    home.is_some_and(|home| path != home && path.starts_with(home))
}

fn push_unique(paths: &mut Vec<String>, path: PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }
    let text = path.to_string_lossy().to_string();
    if !paths.contains(&text) {
        paths.push(text);
    }
}

fn push_line(out: &mut String, key: &str, value: &str) {
    // 路径含换行会破坏按行解析；这种极端路径直接跳过。
    if value.contains('\n') || value.contains('\r') {
        return;
    }
    let _ = writeln!(out, "{key}={value}");
}

pub(crate) fn render(info: &UninstallInfo) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "version={VERSION}");
    push_line(&mut out, "app_bundle", &info.app_bundle);
    for dir in &info.data_dirs {
        push_line(&mut out, "data_dir", dir);
    }
    push_line(&mut out, "db", &info.db_path);
    push_line(&mut out, "upload_dir", &info.upload_dir);
    push_line(
        &mut out,
        "upload_custom",
        if info.upload_custom { "1" } else { "0" },
    );
    push_line(&mut out, "clipboard_dir", &info.clipboard_dir);
    for dir in &info.cache_dirs {
        push_line(&mut out, "cache_dir", dir);
    }
    out
}

fn collect(app: &AppHandle, settings: &AppSettings) -> UninstallInfo {
    let home = crate::cleaner::home_dir();
    let mut data_dirs: Vec<String> = Vec::new();

    // Tauri 的 app data / config / local data 目录（macOS 上通常重合）
    for dir in [
        app.path().app_data_dir().ok(),
        app.path().app_config_dir().ok(),
        app.path().app_local_data_dir().ok(),
    ]
    .into_iter()
    .flatten()
    {
        if dir.exists() && is_owned_path(&dir, home.as_deref()) {
            push_unique(&mut data_dirs, dir);
        }
    }
    // 配置实际写入的候选目录（含 ~/.ws-workbench）；项目内 `.config` 是开发兜底，跳过
    for (name, dir) in crate::config::storage_candidates(app) {
        if name.ends_with("/.config") {
            continue;
        }
        if dir.exists() && is_owned_path(&dir, home.as_deref()) {
            push_unique(&mut data_dirs, dir);
        }
    }
    // cleaner 模块自己的数据目录（popup / sleep 规则等）
    if let Ok(dir) = crate::cleaner::app_data_dir() {
        if is_owned_path(&dir, home.as_deref()) {
            push_unique(&mut data_dirs, dir);
        }
    }

    let app_bundle = std::env::current_exe()
        .ok()
        .and_then(|exe| app_bundle_from_exe(&exe))
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    let upload_custom = settings
        .default_upload_dir
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let upload_dir = crate::commands::attachments::resolve_upload_dir(
        app.clone(),
        settings.default_upload_dir.clone(),
    )
    .unwrap_or_default();

    let db_path = crate::db::db_path(app)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let clipboard_dir = crate::db::storage_dir(app)
        .ok()
        .map(|dir| dir.join(crate::clipboard::store::ATTACHMENTS_DIR))
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    let cache_dirs = home
        .as_deref()
        .map(cache_candidates)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    UninstallInfo {
        app_bundle,
        data_dirs,
        upload_dir,
        upload_custom,
        db_path,
        clipboard_dir,
        cache_dirs,
    }
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{FILE_NAME}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// 刷新清单：写到所有数据目录 + app_data_dir + cleaner 数据目录。
pub(crate) fn refresh(app: &AppHandle, settings: &AppSettings) {
    let info = collect(app, settings);
    let text = render(&info);

    let mut targets: Vec<PathBuf> = info.data_dirs.iter().map(PathBuf::from).collect();
    if let Ok(dir) = app.path().app_data_dir() {
        targets.push(dir);
    }
    if let Ok(dir) = crate::cleaner::app_data_dir() {
        targets.push(dir);
    }

    for dir in targets {
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let path = dir.join(FILE_NAME);
        if let Err(err) = write_private(&path, &text) {
            eprintln!("[uninstall-info] 写入 {} 失败: {}", path.display(), err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn app_bundle_is_found_from_macos_executable() {
        let exe = PathBuf::from("/Applications/ws-workbench.app/Contents/MacOS/ws-workbench");
        assert_eq!(
            app_bundle_from_exe(&exe),
            Some(PathBuf::from("/Applications/ws-workbench.app"))
        );
    }

    #[test]
    fn app_bundle_is_none_outside_dot_app() {
        let exe = PathBuf::from("/Users/me/project/src-tauri/target/debug/ws-workbench");
        assert_eq!(app_bundle_from_exe(&exe), None);
    }

    #[test]
    fn render_skips_newline_paths_and_keeps_flags() {
        let info = UninstallInfo {
            app_bundle: "/Applications/ws-workbench.app".to_string(),
            data_dirs: vec![
                "/Users/me/Library/Application Support/ws-workbench".to_string(),
                "/Users/me/bad\npath".to_string(),
            ],
            upload_dir: "/Users/me/attachments".to_string(),
            upload_custom: true,
            db_path: "/Users/me/db/ws-workbench.db".to_string(),
            clipboard_dir: "/Users/me/clipboard-attachments".to_string(),
            cache_dirs: vec!["/Users/me/Library/WebKit/com.ws.workbench".to_string()],
        };
        let text = render(&info);
        assert!(text.contains("version=1\n"));
        assert!(text.contains("upload_custom=1\n"));
        assert!(text.contains("data_dir=/Users/me/Library/Application Support/ws-workbench\n"));
        assert!(!text.contains("bad\npath"));
        assert_eq!(
            text.lines().filter(|l| l.starts_with("data_dir=")).count(),
            1
        );
    }

    #[test]
    fn cache_candidates_cover_known_app_paths() {
        let home = PathBuf::from("/Users/me");
        let paths: Vec<String> = cache_candidates(&home)
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths
            .iter()
            .any(|p| p.ends_with("Library/WebKit/com.ws.workbench")));
        assert!(paths
            .iter()
            .any(|p| p.ends_with("Library/Caches/com.ws.workbench")));
        assert!(paths
            .iter()
            .any(|p| p.ends_with("Library/Preferences/com.ws.workbench.plist")));
    }

    #[test]
    fn only_paths_under_home_are_owned() {
        let home = Path::new("/Users/me");
        assert!(is_owned_path(
            Path::new("/Users/me/Library/Application Support/ws-workbench"),
            Some(home)
        ));
        assert!(!is_owned_path(Path::new("/Users/me"), Some(home)));
        assert!(!is_owned_path(
            Path::new("/Users/other/Library/Application Support/ws-workbench"),
            Some(home)
        ));
        assert!(!is_owned_path(Path::new("/Users/me/Library"), None));
    }

    #[test]
    fn render_contains_all_sections() {
        let info = UninstallInfo {
            app_bundle: "/Applications/ws-workbench.app".to_string(),
            data_dirs: vec!["/Users/me/Library/Application Support/com.ws.workbench".to_string()],
            upload_dir: "/Users/me/attachments".to_string(),
            upload_custom: false,
            db_path: "/Users/me/db/ws-workbench.db".to_string(),
            clipboard_dir: "/Users/me/clipboard-attachments".to_string(),
            cache_dirs: vec!["/Users/me/Library/Caches/com.ws.workbench".to_string()],
        };
        let text = render(&info);
        for key in [
            "app_bundle=",
            "data_dir=",
            "db=",
            "upload_dir=",
            "clipboard_dir=",
            "cache_dir=",
        ] {
            assert!(text.contains(key), "missing {key}");
        }
        assert!(text.contains("upload_custom=0\n"));
    }
}
