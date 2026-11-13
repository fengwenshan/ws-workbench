//! 卸载窗口的关联文件清单（Windows）：安装目录候选 + 复用残留扫描的结果。
//!
//! 程序文件组的候选来自 `install_dirs`（列表缓存 / 注册表 `InstallLocation` /
//! `DisplayIcon` 目录 / 卸载程序所在目录兜底，且拒绝系统目录），支持与缓存组则
//! 直接把 `scan_leftovers` 的条目按 kind 映射，分组与默认勾选交给
//! `cleaner::related::build_related_files` 统一决定。

use std::path::{Path, PathBuf};

use crate::cleaner::models::{
    CleanerProgress, InstalledApp, LeftoverItem, RelatedFileItem, RelatedFiles,
};

use super::leftovers::scan_leftovers;
use super::parse::parse_uninstall_command;
use super::registry::{
    display_icon_path, is_forbidden_install_dir, open_entry_key, parse_app_id, read_string,
};

/// 安装目录候选：列表缓存 + 注册表 `InstallLocation` / `DisplayIcon` 目录 /
/// `UninstallString` 可执行文件所在目录（安装位置为空时的兜底）。
///
/// 只要有一个「真实存在的」`InstallLocation`，就绝不再用卸载程序所在目录兜底：
/// 否则 `msiexec.exe` / 系统目录会被当成应用目录；注册表里的位置过期时仍允许兜底。
// 实机已验收（2026-11-13，Win32 清单第 2 条前半段）：`UninstallString` 指向
// System32\msiexec.exe / 卸载程序在 System32 的条目，安装位置缺失时「程序文件」组
// 为空，不会把 C:\Windows\System32 或 Program Files 根目录列进去；由
// `windows_smoke::windows_msi_entry_never_offers_system_dirs` 覆盖。
fn install_dirs(app_id: &str, hint: Option<&InstalledApp>) -> Vec<PathBuf> {
    let mut locations: Vec<PathBuf> = Vec::new();
    let mut extras: Vec<PathBuf> = Vec::new();
    let mut fallback: Vec<PathBuf> = Vec::new();
    if let Some(app) = hint {
        let location = app.install_location.trim().trim_matches('"');
        if !location.is_empty() {
            locations.push(PathBuf::from(location));
        }
    }
    if let Ok((hive, key_path)) = parse_app_id(app_id) {
        if let Ok(key) = open_entry_key(hive, key_path) {
            let location = read_string(&key, "InstallLocation");
            let location = location.trim().trim_matches('"');
            if !location.is_empty() {
                locations.push(PathBuf::from(location));
            }
            if let Some(icon) = display_icon_path(&read_string(&key, "DisplayIcon")) {
                if let Some(parent) = icon.parent() {
                    extras.push(parent.to_path_buf());
                }
            }
            if let Some(command) =
                parse_uninstall_command(&read_string(&key, "UninstallString"), false)
            {
                if let Some(parent) = Path::new(&command.program).parent() {
                    fallback.push(parent.to_path_buf());
                }
            }
        }
    }
    // 有「真实存在的」安装位置时优先用它；注册表里的位置可能过期（目录已被删），
    // 这时仍回退到卸载程序/DisplayIcon 目录，避免程序文件组整组消失。
    let candidates: Vec<PathBuf> = if locations.iter().any(|path| path.is_dir()) {
        locations.into_iter().chain(extras).collect()
    } else {
        extras.into_iter().chain(fallback).collect()
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for path in candidates {
        if path.is_dir() && !is_forbidden_install_dir(&path) && !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    dirs
}

/// 一个存在的文件/目录 → 关联文件条目
fn related_item_from_path(
    path: &Path,
    kind: &str,
    confidence: &str,
    requires_elevation: bool,
) -> RelatedFileItem {
    let size_bytes = if path.is_dir() {
        crate::cleaner::dir_size_limited(path, 4, 200_000)
    } else {
        std::fs::symlink_metadata(path)
            .map(|meta| meta.len())
            .unwrap_or(0)
    };
    RelatedFileItem {
        path: path.to_string_lossy().to_string(),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string()),
        kind: kind.to_string(),
        size_bytes,
        is_dir: path.is_dir(),
        version: String::new(),
        confidence: confidence.to_string(),
        requires_elevation,
        exists: true,
        selected: false,
    }
}

/// 残留条目 → 关联文件条目（注册表项展示键名，其余展示最后一段文件名）
fn related_item_from_leftover(item: LeftoverItem) -> RelatedFileItem {
    let name = match crate::cleaner::registry_id::split_registry_id(&item.path) {
        Some((_, key_path)) => key_path.rsplit('\\').next().unwrap_or(key_path).to_string(),
        None => Path::new(&item.path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| item.path.clone()),
    };
    RelatedFileItem {
        path: item.path,
        name,
        kind: item.kind,
        size_bytes: item.size_bytes,
        is_dir: item.is_dir,
        version: String::new(),
        confidence: item.confidence,
        requires_elevation: item.requires_elevation,
        exists: item.exists,
        selected: false,
    }
}

/// 卸载窗口的关联文件清单（Windows）。
///
/// 程序文件 = 安装目录（以及安装位置缺失时的兜底目录）；支持/缓存 =
/// 现有残留扫描结果按 kind 映射（registry/shortcut → 支持文件，
/// cache/log → 缓存文件）。`pkgutil` 收据是 macOS 概念，这里没有。
pub fn related_files(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<RelatedFiles, String> {
    emit(CleanerProgress::new(
        "related_files",
        "scanning",
        0,
        0,
        "正在扫描关联文件…",
    ));

    let mut items: Vec<RelatedFileItem> = Vec::new();
    // 1) 程序文件：安装目录
    for dir in install_dirs(app_id, hint) {
        items.push(related_item_from_path(&dir, "install_dir", "high", false));
    }
    // 2) 支持 / 缓存：复用残留扫描（含注册表项与快捷方式）
    for leftover in scan_leftovers(app_id, hint, emit)? {
        items.push(related_item_from_leftover(leftover));
    }

    let files = crate::cleaner::related::build_related_files(app_id, items, String::new());
    emit(CleanerProgress::new(
        "related_files",
        "done",
        files.item_count,
        files.item_count,
        format!("找到 {} 个关联文件", files.item_count),
    ));
    Ok(files)
}
