//! 残留扫描（设计 4.3 的 Windows 规则）：安装目录、注册表残留、AppData/ProgramData
//! 同名目录、开始菜单与桌面快捷方式。
//!
//! 只负责「找候选并给出 confidence / 是否需要提权」，删除由 `trash_guard` 按白名单执行。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cleaner::models::{CleanerProgress, InstalledApp, LeftoverItem};

use super::parse::Hive;
use super::registry::{open_entry_key, parse_app_id, read_string};

/// 一个文件/目录残留（存在才算）
fn file_leftover(
    path: &Path,
    kind: &str,
    confidence: &str,
    selected: bool,
    requires_elevation: bool,
) -> Option<LeftoverItem> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let is_dir = meta.is_dir();
    let size_bytes = if is_dir {
        crate::cleaner::dir_size_limited(path, 4, 200_000)
    } else {
        meta.len()
    };
    Some(LeftoverItem {
        path: path.to_string_lossy().to_string(),
        kind: kind.to_string(),
        size_bytes,
        is_dir,
        confidence: confidence.to_string(),
        selected,
        requires_elevation,
        exists: true,
    })
}

/// 注册表残留：`SOFTWARE\<Publisher>\<Name>`（HKLM 64/32 视图 + HKCU）
fn registry_candidates(publisher: &str, name: &str) -> Vec<(Hive, String)> {
    let publisher = publisher.trim();
    let name = name.trim();
    if publisher.is_empty() || name.is_empty() {
        return Vec::new();
    }
    let base_path = format!(r"SOFTWARE\{}\{}", publisher, name);
    let mut out: Vec<(Hive, String)> = Vec::new();
    let mut seen: HashSet<(Hive, String)> = HashSet::new();
    for hive in [Hive::Hklm, Hive::HklmWow64, Hive::Hkcu] {
        if !seen.insert((hive, base_path.clone())) {
            continue;
        }
        if open_entry_key(hive, &base_path).is_ok() {
            out.push((hive, base_path.clone()));
        }
    }
    out
}

/// `%APPDATA%` / `%LOCALAPPDATA%` / `%PROGRAMDATA%` 下的同名目录
fn appdata_candidates(name: &str, publisher: &str) -> Vec<LeftoverItem> {
    let name = name.trim();
    if name.is_empty() {
        return Vec::new();
    }
    let publisher = publisher.trim();
    let mut items: Vec<LeftoverItem> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for var in ["APPDATA", "LOCALAPPDATA", "PROGRAMDATA"] {
        let Some(root) = std::env::var_os(var).map(PathBuf::from) else {
            continue;
        };
        let mut push = |path: PathBuf,
                        kind: &str,
                        confidence: &str,
                        selected: bool,
                        items: &mut Vec<LeftoverItem>| {
            if !seen.insert(path.clone()) {
                return;
            }
            if let Some(item) = file_leftover(&path, kind, confidence, selected, false) {
                items.push(item);
            }
        };
        push(root.join(name), "app_data", "medium", false, &mut items);
        if !publisher.is_empty() {
            push(
                root.join(publisher).join(name),
                "app_data",
                "medium",
                false,
                &mut items,
            );
        }
    }
    // %LOCALAPPDATA%\Programs\<Name> 是 per-user 安装位置，属于 high
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        if let Some(item) = file_leftover(
            &local.join("Programs").join(name),
            "install_dir",
            "high",
            true,
            false,
        ) {
            items.push(item);
        }
    }
    items
}

/// 开始菜单 / 桌面里与应用同名的快捷方式
fn shortcut_candidates(name: &str) -> Vec<LeftoverItem> {
    let name = name.trim();
    if name.is_empty() {
        return Vec::new();
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["APPDATA", "PROGRAMDATA"] {
        if let Some(root) = std::env::var_os(var).map(PathBuf::from) {
            roots.push(root.join(r"Microsoft\Windows\Start Menu\Programs"));
        }
    }
    if let Some(home) = crate::cleaner::home_dir() {
        roots.push(home.join("Desktop"));
    }
    if let Some(public) = std::env::var_os("PUBLIC").map(PathBuf::from) {
        roots.push(public.join("Desktop"));
    }

    let mut items: Vec<LeftoverItem> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        let walker = walkdir::WalkDir::new(&root)
            .follow_links(false)
            .max_depth(3)
            .into_iter()
            .filter_map(|entry| entry.ok());
        for entry in walker {
            let path = entry.path();
            let is_lnk = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"));
            if !is_lnk {
                continue;
            }
            let stem = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            if !stem.eq_ignore_ascii_case(name) {
                continue;
            }
            if !seen.insert(path.to_path_buf()) {
                continue;
            }
            if let Some(item) = file_leftover(path, "shortcut", "high", true, false) {
                items.push(item);
            }
        }
    }
    items
}

/// 残留扫描（设计 4.3 的 Windows 规则）
// TODO(platform-verify:windows): 真实机器上残留扫描能命中安装目录 / AppData / 开始菜单与桌面快捷方式；验证步骤：Windows 实机卸载一个测试应用后点「扫描残留」，核对列出的路径确实属于该应用，勾选删除后确认进了回收站（设计文档 9.2「Windows 验收清单」第 3 条）；期望：安装目录、注册表项、AppData/ProgramData、快捷方式按 confidence 合理列出，删除后原路径消失。
pub fn scan_leftovers(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<Vec<LeftoverItem>, String> {
    let (hive, key_path) = parse_app_id(app_id)?;
    // 卸载完成后卸载器往往把 Uninstall 键一起删了；此时靠列表缓存里的
    // 名字/发布者/安装目录继续扫残留，而不是直接报「打开注册表项失败」
    let (name, publisher, install_location) = match open_entry_key(hive, key_path) {
        Ok(key) => {
            let mut name = read_string(&key, "DisplayName");
            if name.trim().is_empty() {
                name = hint.map(|app| app.name.clone()).unwrap_or_default();
            }
            (
                name,
                read_string(&key, "Publisher"),
                read_string(&key, "InstallLocation"),
            )
        }
        Err(error) => match hint {
            Some(app) => (
                app.name.clone(),
                app.publisher.clone(),
                app.install_location.clone(),
            ),
            None => return Err(error),
        },
    };

    emit(CleanerProgress::new(
        "scan_leftovers",
        "scanning",
        0,
        0,
        format!("正在扫描 {} 的残留…", name),
    ));
    let mut items: Vec<LeftoverItem> = Vec::new();

    // 1) 安装目录（high）
    let location = install_location.trim().trim_matches('"');
    if !location.is_empty() {
        if let Some(item) = file_leftover(Path::new(location), "install_dir", "high", true, false) {
            items.push(item);
        }
    }

    // 2) 注册表残留（只列出，删除需要用户显式勾选并提权）
    for (residual_hive, residual_path) in registry_candidates(&publisher, &name) {
        let requires_elevation = residual_hive != Hive::Hkcu;
        items.push(LeftoverItem {
            path: format!("win:{}|{}", residual_hive.token(), residual_path),
            kind: "registry".to_string(),
            size_bytes: 0,
            is_dir: false,
            confidence: "medium".to_string(),
            selected: false,
            requires_elevation,
            exists: true,
        });
    }

    // 3) AppData / ProgramData 同名目录
    items.extend(appdata_candidates(&name, &publisher));

    // 4) 开始菜单 / 桌面快捷方式
    items.extend(shortcut_candidates(&name));

    emit(CleanerProgress::new(
        "scan_leftovers",
        "done",
        items.len(),
        items.len(),
        format!("找到 {} 条残留", items.len()),
    ));
    Ok(items)
}
