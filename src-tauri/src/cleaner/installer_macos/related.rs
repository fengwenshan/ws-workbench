use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::super::models::{CleanerProgress, InstalledApp, RelatedFileItem, RelatedFiles};
use super::super::paths::is_trash_path;
use super::super::{home_dir, path_size};
use super::bundle::{read_bundle_info, BundleInfo};
use super::bundle_from_id;
use super::leftovers::mac_leftover_candidates;
use super::list::{is_app_bundle, is_app_dir};

// ============================== 关联文件（卸载窗口，设计 4.2） ==============================

/// 兜底扫描根：Spotlight 不可用/无结果时只扫这些目录的一层 `.app`。
fn fallback_bundle_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications"), PathBuf::from("/Users/Shared")];
    if let Some(home) = home_dir() {
        roots.push(home.join("Applications"));
        roots.push(home.join("Desktop"));
        roots.push(home.join("Downloads"));
    }
    roots
}

/// `mdfind "kMDItemCFBundleIdentifier == <id>"` 的结果；命令失败/未索引返回空。
fn mdfind_apps(bundle_id: &str) -> Vec<PathBuf> {
    let bundle_id = bundle_id.trim();
    if bundle_id.is_empty() {
        return Vec::new();
    }
    let query = format!("kMDItemCFBundleIdentifier == {}", bundle_id);
    let Ok(output) = std::process::Command::new("mdfind").arg(&query).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// 扫一个目录的一层 `.app`（兜底扫描用，不递归子目录）
fn scan_apps_one_level(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_app_bundle(path))
        .collect()
}

/// 副本过滤：只留存在的 `.app`，排除主 bundle、废纸篓、`/System`；去重排序。
fn filter_bundle_copies(candidates: Vec<PathBuf>, main: &Path) -> Vec<PathBuf> {
    let main = main.canonicalize().unwrap_or_else(|_| main.to_path_buf());
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        // 设计 4.2：mdfind 结果只要求「以 .app 结尾且存在」；不强制可解析，
        // 半残（Info.plist 缺失）的副本也要能列出来清理
        if !is_app_dir(&candidate) {
            continue;
        }
        // 系统应用永远不列（也不允许删）
        if candidate.starts_with("/System") {
            continue;
        }
        // 已经在废纸篓里的副本不列：删它等于永久删除，不属于「卸载」语义
        if candidate
            .components()
            .any(|component| component.as_os_str() == ".Trash")
        {
            continue;
        }
        let canonical = candidate.canonicalize().unwrap_or_else(|_| candidate.clone());
        if canonical == main {
            continue;
        }
        if !seen.insert(canonical.clone()) {
            continue;
        }
        out.push(canonical);
    }
    out.sort();
    out
}

/// 主 bundle 之外的其它 `.app` 副本（程序文件组）。
///
/// 先按 bundle id 走 Spotlight（同一个应用可能被复制到多个目录）；命令不可用
/// 或没有结果时兜底扫描常见目录，并用 `Info.plist` 的 bundle id 精确比对。
pub(crate) fn app_bundle_copies(bundle: &Path, bundle_id: &str) -> Vec<PathBuf> {
    let mut candidates = mdfind_apps(bundle_id);
    if candidates.is_empty() {
        candidates = fallback_bundle_roots()
            .iter()
            .flat_map(|root| scan_apps_one_level(root))
            .filter(|path| bundle_id_matches(path, bundle_id))
            .collect();
    }
    filter_bundle_copies(candidates, bundle)
}

/// 单测版本：只扫给定目录（一层）并按 bundle id 校验，不调用 `mdfind`。
#[cfg(test)]
pub(crate) fn app_bundle_copies_in(
    bundle: &Path,
    bundle_id: &str,
    roots: &[PathBuf],
) -> Vec<PathBuf> {
    let candidates: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| scan_apps_one_level(root))
        .filter(|path| bundle_id_matches(path, bundle_id))
        .collect();
    filter_bundle_copies(candidates, bundle)
}

fn bundle_id_matches(bundle: &Path, bundle_id: &str) -> bool {
    !bundle_id.trim().is_empty()
        && read_bundle_info(bundle)
            .and_then(|info| info.bundle_id)
            .is_some_and(|id| id == bundle_id.trim())
}

/// `mdls -raw -name kMDItemLastUsedDate` 的原始输出
fn mdls_last_used(bundle: &Path) -> Option<String> {
    let output = std::process::Command::new("mdls")
        .args(["-raw", "-name", "kMDItemLastUsedDate", "--"])
        .arg(bundle)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() || text == "(null)" {
        return None;
    }
    Some(text)
}

/// 「使用情况」文案：今天 / 7 天内 / 30 天内 / 最近未使用；取不到返回空串。
/// 文案映射本身在 `related::last_used_text_from`（平台无关，设计 4.1）。
pub(crate) fn last_used_text(bundle: &Path) -> String {
    let Some(raw) = mdls_last_used(bundle) else {
        return String::new();
    };
    super::super::related::last_used_text_from(&raw, chrono::Local::now().fixed_offset())
}

/// 一个 `.app` 包 → 程序文件条目（不存在返回 None）
fn bundle_related_item(path: &Path, name: &str, version: &str) -> Option<RelatedFileItem> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(RelatedFileItem {
        path: path.to_string_lossy().to_string(),
        name: name.to_string(),
        kind: "app_bundle".to_string(),
        size_bytes: if meta.is_dir() {
            path_size(path)
        } else {
            meta.len()
        },
        is_dir: meta.is_dir(),
        version: version.to_string(),
        confidence: "high".to_string(),
        requires_elevation: false,
        exists: true,
        selected: false,
    })
}

/// 展示名：路径最后一段（macOS 这里的条目都是普通文件/目录）
fn item_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// 关联文件清单的废纸篓过滤：程序文件副本在 `filter_bundle_copies` 里已过滤，
/// 这里对支持/缓存（`mac_leftover_candidates` 会带 `~/.Trash/<应用名>` 这类 low
/// 条目）再兜一层，保证整个 `RelatedFiles` 里不会出现任何废纸篓路径。
pub(super) fn retain_non_trash(items: &mut Vec<RelatedFileItem>) {
    items.retain(|item| !is_trash_path(Path::new(&item.path)));
}

/// 卸载窗口的关联文件清单（macOS）。
///
/// 程序文件 = 主 bundle + Spotlight 找到的其它副本；支持/缓存复用
/// `mac_leftover_candidates` 的规则按 kind 映射；`pkgutil` 收据不列（设计 2）。
pub(crate) fn related_files(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<RelatedFiles, String> {
    let bundle = bundle_from_id(app_id)?;
    let info = read_bundle_info(&bundle).or_else(|| {
        // 应用已被移入废纸篓：用列表缓存兜底（与 scan_leftovers 一致）
        hint.map(|app| BundleInfo {
            bundle_id: Some(app.detail.clone()).filter(|id| !id.is_empty()),
            name: app.name.clone(),
            version: app.version.clone(),
            executable: None,
        })
    });
    let (bundle_id, name, version) = match info {
        Some(info) => (
            info.bundle_id.unwrap_or_default(),
            info.name,
            info.version,
        ),
        None => (
            String::new(),
            bundle
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default(),
            String::new(),
        ),
    };

    emit(CleanerProgress::new(
        "related_files",
        "scanning",
        0,
        0,
        format!("正在扫描 {} 的关联文件…", name),
    ));

    let mut items: Vec<RelatedFileItem> = Vec::new();

    // 程序文件 1：主 bundle
    if let Some(item) = bundle_related_item(&bundle, &name, &version) {
        items.push(item);
    }
    // 程序文件 2：其它副本（按 bundle id；版本取副本自己的 Info.plist）
    for copy in app_bundle_copies(&bundle, &bundle_id) {
        let copy_info = read_bundle_info(&copy).unwrap_or_default();
        let copy_name = if copy_info.name.trim().is_empty() {
            copy.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone())
        } else {
            copy_info.name
        };
        let copy_version = if copy_info.version.trim().is_empty() {
            version.clone()
        } else {
            copy_info.version
        };
        if let Some(item) = bundle_related_item(&copy, &copy_name, &copy_version) {
            items.push(item);
        }
    }

    // 支持 / 缓存：复用现有残留规则（不包含 pkgutil 收据）。
    // `mac_leftover_candidates` 里的 low 条目（深扫命中的厂商目录、
    // `~/.Trash/<应用名>` 等）照常参与组装；low 默认不勾选（由
    // `build_related_files` 统一决定），只有用户显式「全选」才会被删除。
    for leftover in mac_leftover_candidates(&bundle_id, &name) {
        let path = PathBuf::from(&leftover.path);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        items.push(RelatedFileItem {
            path: leftover.path.clone(),
            name: item_display_name(&path),
            kind: leftover.kind,
            size_bytes: if meta.is_dir() {
                path_size(&path)
            } else {
                meta.len()
            },
            is_dir: meta.is_dir(),
            version: String::new(),
            confidence: leftover.confidence,
            requires_elevation: leftover.requires_elevation,
            exists: true,
            selected: false,
        });
    }

    // 废纸篓里的条目绝不进清单：全选会把它送进删除，而废纸篓路径是永久删除
    // （见 `is_trash_path` 注释）。程序文件副本已提前过滤，这里覆盖支持/缓存。
    retain_non_trash(&mut items);

    let last_used = last_used_text(&bundle);
    let files = super::super::related::build_related_files(app_id, items, last_used);
    emit(CleanerProgress::new(
        "related_files",
        "done",
        files.item_count,
        files.item_count,
        format!("找到 {} 个关联文件", files.item_count),
    ));
    Ok(files)
}
