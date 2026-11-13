use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::super::models::{CleanerProgress, InstalledApp};
use super::super::{dir_size_limited, format_date, home_dir, parallel_map};
use super::bundle::{is_uninstallable, read_bundle_info};
use super::{current_app_bundle, same_path};

/// 扫出所有 `.app` 包（不带图标/大小，用于先出总数）
fn scan_bundles() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![PathBuf::from("/Applications")];
    if let Some(home) = home_dir() {
        roots.push(home.join("Applications"));
    }

    let mut bundles: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut push = |path: PathBuf, bundles: &mut Vec<PathBuf>| {
        if seen.insert(path.clone()) {
            bundles.push(path);
        }
    };

    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // 隐藏目录（.localized 等）不是应用
            if name.starts_with('.') {
                continue;
            }
            if is_app_bundle(&path) {
                push(path, &mut bundles);
                continue;
            }
            // 一层子目录（/Applications/Utilities 之类）
            if path.is_dir() {
                let Ok(sub_entries) = std::fs::read_dir(&path) else {
                    continue;
                };
                for sub in sub_entries.flatten() {
                    let sub_path = sub.path();
                    if is_app_bundle(&sub_path) {
                        push(sub_path, &mut bundles);
                    }
                }
            }
        }
    }
    bundles
}

/// `.app` 目录（不要求能解析 Info.plist：半残副本也要能被列出来清理）
pub(super) fn is_app_dir(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("app") && path.is_dir()
}

pub(super) fn is_app_bundle(path: &Path) -> bool {
    is_app_dir(path) && path.join("Contents").join("Info.plist").is_file()
}

/// 列表主流程：先枚举（快），再 8 个 worker 并发取图标/算大小。
pub fn list_apps(emit: &(dyn Fn(CleanerProgress) + Sync)) -> Vec<InstalledApp> {
    emit(CleanerProgress::new(
        "list_apps",
        "scanning",
        0,
        0,
        "正在扫描应用目录…",
    ));
    let bundles = scan_bundles();
    let total = bundles.len();
    emit(CleanerProgress::new(
        "list_apps",
        "loading_icons",
        0,
        total,
        format!("找到 {} 个应用，正在读取图标与大小…", total),
    ));

    let mut apps = parallel_map(&bundles, 8, |index, bundle| {
        let info = read_bundle_info(bundle).unwrap_or_default();
        let install_date = std::fs::metadata(bundle)
            .and_then(|meta| meta.created().or_else(|_| meta.modified()))
            .map(format_date)
            .unwrap_or_default();
        if index % 3 == 0 || index + 1 == total {
            emit(CleanerProgress::new(
                "list_apps",
                "sizing",
                index + 1,
                total,
                format!("正在统计大小：{}", info.name),
            ));
        }
        InstalledApp {
            id: format!("mac:{}", bundle.to_string_lossy()),
            name: info.name.clone(),
            version: info.version.clone(),
            publisher: info
                .bundle_id
                .clone()
                .unwrap_or_default(),
            install_location: bundle
                .parent()
                .map(|parent| parent.to_string_lossy().to_string())
                .unwrap_or_default(),
            size_bytes: dir_size_limited(bundle, 64, 200_000),
            icon: crate::commands::openwith::mac_icon(bundle),
            source: "applications".to_string(),
            can_uninstall: is_uninstallable(bundle),
            uninstall_kind: "app_bundle".to_string(),
            install_date,
            detail: info.bundle_id.unwrap_or_default(),
        }
    });
    emit(CleanerProgress::new(
        "list_apps",
        "done",
        total,
        total,
        format!("已加载 {} 个应用", total),
    ));
    apps.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
    });
    // 不能卸载自己：过滤掉当前运行的 `.app` 本体。
    // dev 裸二进制不在 bundle 内（current_app_bundle 为 None），不会误过滤。
    let self_bundle = current_app_bundle();
    apps.retain(|app| match (&self_bundle, app.id.strip_prefix("mac:")) {
        (Some(self_bundle), Some(path)) => !same_path(self_bundle, Path::new(path)),
        _ => true,
    });
    apps
}
