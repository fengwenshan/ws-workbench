//! macOS 侧安装列表、卸载与残留规则。
//!
//! 数据源：`/Applications`、`~/Applications`（含一层子目录，如 `Utilities`）。
//! `/System/Applications` 不扫 —— 系统应用不可卸载，扫出来只会让列表变长。
//!
//! 残留按设计 4.3 用「bundle id / 应用名精确匹配」，不做模糊包含：宁可漏，
//! 不能误删用户别的东西。所有候选路径在这里只负责「生成」，是否真的存在由
//! 扫描阶段用 `symlink_metadata` 判定（`exists=false` 的不返回给前端）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::models::{CleanerProgress, InstalledApp, LeftoverItem, UninstallOutcome};
use super::{dir_size_limited, format_date, home_dir, path_size};

/// 从 Info.plist 读出来的 bundle 元数据
#[derive(Debug, Clone, Default)]
pub struct BundleInfo {
    pub bundle_id: Option<String>,
    pub name: String,
    pub version: String,
    /// `CFBundleExecutable`：界面暂不展示，留给后续按可执行文件名精确匹配进程
    #[allow(dead_code)]
    pub executable: Option<String>,
}

/// 读 `Contents/Info.plist`。解析失败 / 文件不存在返回 None（有的「应用」只是
/// 一个空壳目录，列表里应该跳过而不是报错）。
pub fn read_bundle_info(bundle: &Path) -> Option<BundleInfo> {
    let plist_path = bundle.join("Contents").join("Info.plist");
    let value = plist::Value::from_file(&plist_path).ok()?;
    let dict = value.as_dictionary()?;
    let get = |key: &str| {
        dict.get(key)
            .and_then(|value| value.as_string())
            .map(str::to_string)
            .filter(|text| !text.is_empty())
    };
    let stem_name = bundle
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "未知应用".to_string());
    let name = get("CFBundleDisplayName")
        .or_else(|| get("CFBundleName"))
        .unwrap_or(stem_name);
    let version = get("CFBundleShortVersionString")
        .or_else(|| get("CFBundleVersion"))
        .unwrap_or_default();
    Some(BundleInfo {
        bundle_id: get("CFBundleIdentifier"),
        name,
        version,
        executable: get("CFBundleExecutable"),
    })
}

/// 应用是否允许卸载。
///
/// 路径在 `/System` 下、或 bundle id 以 `com.apple.` 开头 → 不可卸载。
/// 本版本不扫 `/System`，但判定逻辑留着：前端可能拿到旧缓存，SIP 应用
/// 即使被列出来也不能给卸载入口。
pub fn is_uninstallable(bundle: &Path) -> bool {
    if bundle.starts_with("/System") {
        return false;
    }
    match read_bundle_info(bundle) {
        Some(info) => !info
            .bundle_id
            .as_deref()
            .is_some_and(|id| id.starts_with("com.apple.")),
        // plist 读不出来时按「非系统路径即可卸载」处理，交给 trash 去报真实错误
        None => true,
    }
}

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

fn is_app_bundle(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("app")
        && path.is_dir()
        && path.join("Contents").join("Info.plist").is_file()
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

    let mut apps = super::parallel_map(&bundles, 8, |index, bundle| {
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
    apps
}

/// 卸载：先结束运行中的实例，再把 bundle 移入废纸篓。
///
/// 返回 `UninstallOutcome` 而不是 Err：SIP 保护 / 应用正被占用都是「用户需要
/// 看到原因」的正常失败，不是命令层异常。
pub fn uninstall(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<UninstallOutcome, String> {
    let bundle = bundle_from_id(app_id)?;
    if !is_uninstallable(&bundle) {
        return Err(format!(
            "{} 是系统应用（受 SIP 保护），不支持卸载",
            hint.map(|app| app.name.as_str()).unwrap_or("该应用")
        ));
    }
    if !bundle.exists() {
        return Err(format!("应用不存在: {}", bundle.display()));
    }

    emit(CleanerProgress::new(
        "uninstall",
        "running",
        0,
        1,
        "正在移入废纸篓…",
    ));
    let command = format!("移入废纸篓: {}", bundle.display());
    let outcome = match super::trash_guard::trash_path(&bundle) {
        Ok(()) => UninstallOutcome {
            ok: true,
            command,
            exit_code: None,
            elevated: false,
            message: "已移入废纸篓".to_string(),
        },
        Err(error) => UninstallOutcome {
            ok: false,
            command,
            exit_code: None,
            elevated: false,
            message: error,
        },
    };
    emit(CleanerProgress::new("uninstall", "done", 1, 1, "卸载结束"));
    Ok(outcome)
}

/// 从 app id（`mac:/Applications/Foo.app`）解析 bundle 路径
fn bundle_from_id(app_id: &str) -> Result<PathBuf, String> {
    let path = app_id
        .strip_prefix("mac:")
        .ok_or_else(|| format!("无效的应用标识: {}", app_id))?;
    let bundle = PathBuf::from(path);
    if bundle.as_os_str().is_empty() {
        return Err(format!("无效的应用标识: {}", app_id));
    }
    Ok(bundle)
}

// ============================== 残留规则 ==============================

/// 一条候选残留（尚未判存在）
#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    kind: &'static str,
    confidence: &'static str,
    requires_elevation: bool,
    is_dir: bool,
    selected: bool,
}

impl Candidate {
    fn new(
        path: PathBuf,
        kind: &'static str,
        confidence: &'static str,
        requires_elevation: bool,
        is_dir: bool,
    ) -> Self {
        Self {
            selected: confidence == "high",
            path,
            kind,
            confidence,
            requires_elevation,
            is_dir,
        }
    }
}

/// 精确同名的「插件/扩展类」目录：(目录, 后缀, kind, 置信度)。
///
/// 设计 4.0.1 从柠檬解析出来的补充根：按应用名或 bundle id **精确同名**匹配，
/// 不做包含匹配。扩展类目录里放的是别的应用也可能用的东西，所以置信度只给 medium。
const NAME_MATCH_DIRS: &[(&str, &str, &str, &str)] = &[
    ("Library/Extensions", ".kext", "app_data", "medium"),
    ("Library/Filesystems", "", "app_data", "medium"),
    ("Library/PreferencePanes", ".prefPane", "preference", "medium"),
    ("Library/StartupItems", "", "app_data", "medium"),
];

const HOME_NAME_MATCH_DIRS: &[(&str, &str, &str, &str)] = &[
    ("Library/PreferencePanes", ".prefPane", "preference", "medium"),
    ("Library/LaunchDaemons", ".plist", "launch_agent", "medium"),
    ("Library/Logs", "", "log", "medium"),
];

/// 生成 macOS 残留候选列表（不判存在，由调用方过滤）。
///
/// `bundle_id` 为空时只保留「按应用名精确匹配」的 medium 规则。
pub fn mac_leftover_candidates(bundle_id: &str, name: &str) -> Vec<LeftoverItem> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let id = bundle_id.trim();
    let name = name.trim();

    if !id.is_empty() {
        // high：按 bundle id 精确匹配用户目录下的标准残留位
        for (sub, kind, is_dir) in [
            ("Library/Caches", "cache", true),
            ("Library/Application Support", "app_data", true),
            ("Library/Saved Application State", "app_data", true),
            ("Library/Containers", "container", true),
            ("Library/HTTPStorages", "cache", true),
            ("Library/WebKit", "cache", true),
            ("Library/Application Scripts", "app_data", true),
        ] {
            candidates.push(Candidate::new(
                home.join(sub).join(id),
                kind,
                "high",
                false,
                is_dir,
            ));
        }
        candidates.push(Candidate::new(
            home.join("Library/Preferences").join(format!("{}.plist", id)),
            "preference",
            "high",
            false,
            false,
        ));
        candidates.push(Candidate::new(
            home.join("Library/LaunchAgents").join(format!("{}.plist", id)),
            "launch_agent",
            "high",
            false,
            false,
        ));

        // Group Containers 的目录名通常是 `<TeamID>.<bundleId>`，所以按后缀精确匹配
        let group_containers = home.join("Library/Group Containers");
        if let Ok(entries) = std::fs::read_dir(&group_containers) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name == id
                    || file_name.ends_with(&format!(".{}", id))
                {
                    candidates.push(Candidate::new(
                        entry.path(),
                        "container",
                        "high",
                        false,
                        true,
                    ));
                }
            }
        }

        // LaunchAgents 里同前缀的 plist（如 `<id>-helper.plist`）
        let agents = home.join("Library/LaunchAgents");
        if let Ok(entries) = std::fs::read_dir(&agents) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with(id) && file_name.ends_with(".plist") {
                    candidates.push(Candidate::new(
                        entry.path(),
                        "launch_agent",
                        "high",
                        false,
                        false,
                    ));
                }
            }
        }

        // medium：按应用名精确匹配（很多应用不按 bundle id 建目录）
        // 以及 bundle id 命名的额外目录
        for (sub, kind, is_dir) in [
            ("Library/Application Support", "app_data", true),
            ("Library/Caches", "cache", true),
            ("Library/Logs", "log", true),
        ] {
            candidates.push(Candidate::new(
                home.join(sub).join(name),
                kind,
                "medium",
                false,
                is_dir,
            ));
        }

        // 提权目录：/Library 下与 bundle id 精确匹配的项
        for (sub, kind, is_dir) in [
            ("Application Support", "app_data", true),
            ("Caches", "cache", true),
            ("LaunchAgents", "launch_agent", false),
            ("LaunchDaemons", "launch_agent", false),
            ("PrivilegedHelperTools", "app_data", false),
        ] {
            let base = PathBuf::from("/Library").join(sub);
            let path = if is_dir {
                base.join(id)
            } else {
                base.join(format!("{}.plist", id))
            };
            candidates.push(Candidate::new(path, kind, "medium", true, is_dir));
        }
        candidates.push(Candidate::new(
            PathBuf::from("/Library/Preferences").join(format!("{}.plist", id)),
            "preference",
            "medium",
            true,
            false,
        ));

        // 补充根：Extensions / Filesystems / PreferencePanes / StartupItems
        for (sub, suffix, kind, confidence) in NAME_MATCH_DIRS {
            let base = PathBuf::from("/").join(sub);
            candidates.push(Candidate::new(
                base.join(format!("{}{}", name, suffix)),
                kind,
                confidence,
                true,
                !suffix.is_empty(),
            ));
            candidates.push(Candidate::new(
                base.join(format!("{}{}", id, suffix)),
                kind,
                confidence,
                true,
                !suffix.is_empty(),
            ));
        }
        // ~/Library/{PreferencePanes, LaunchDaemons, Logs} 按应用名精确匹配
        for (sub, suffix, kind, confidence) in HOME_NAME_MATCH_DIRS {
            candidates.push(Candidate::new(
                home.join(sub).join(format!("{}{}", name, suffix)),
                kind,
                confidence,
                false,
                !suffix.is_empty(),
            ));
        }

        // ~/.Trash 里与 bundle id / 应用名精确匹配的条目：low，默认不勾选。
        // 删除时是「从废纸篓里真正删掉」（它本来就在废纸篓里），属永久删除，
        // 所以必须由用户手动勾选。
        let trash_dir = home.join(".Trash");
        for entry in [name.to_string(), id.to_string()] {
            if entry.is_empty() {
                continue;
            }
            candidates.push(Candidate {
                path: trash_dir.join(entry),
                kind: "cache",
                confidence: "low",
                requires_elevation: false,
                is_dir: true,
                selected: false,
            });
        }
    } else {
        // 没有 bundle id 时只能按应用名匹配
        for (sub, kind, is_dir) in [
            ("Library/Application Support", "app_data", true),
            ("Library/Caches", "cache", true),
            ("Library/Logs", "log", true),
        ] {
            candidates.push(Candidate::new(
                home.join(sub).join(name),
                kind,
                "medium",
                false,
                is_dir,
            ));
        }
    }

    // 去重（同一个路径可能同时被 high / medium 规则命中，保留高置信度那条）
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut items: Vec<LeftoverItem> = Vec::new();
    for candidate in candidates {
        if !seen.insert(candidate.path.clone()) {
            continue;
        }
        items.push(LeftoverItem {
            path: candidate.path.to_string_lossy().to_string(),
            kind: candidate.kind.to_string(),
            size_bytes: 0,
            is_dir: candidate.is_dir,
            confidence: candidate.confidence.to_string(),
            selected: candidate.selected,
            requires_elevation: candidate.requires_elevation,
            exists: false,
        });
    }
    items
}

/// 把候选列表变成「真实存在」的残留（填 exists/size/is_dir）
fn fill_existing(candidates: Vec<LeftoverItem>) -> Vec<LeftoverItem> {
    let mut items: Vec<LeftoverItem> = candidates
        .into_iter()
        .filter_map(|mut item| {
            let path = PathBuf::from(&item.path);
            let meta = std::fs::symlink_metadata(&path).ok()?;
            item.exists = true;
            item.is_dir = meta.is_dir();
            item.size_bytes = path_size(&path);
            Some(item)
        })
        .collect();

    // 高置信度、需要提权的排在后面：UI 按 kind 分组展示，顺序只影响观感
    items.sort_by(|left, right| {
        let rank = |confidence: &str| match confidence {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        };
        rank(&left.confidence)
            .cmp(&rank(&right.confidence))
            .then_with(|| left.path.cmp(&right.path))
    });
    items
}

/// 安装收据（pkgutil receipt）：本版本只读展示，不自动删除
fn receipt_candidates(bundle_id: &str, name: &str) -> Vec<LeftoverItem> {
    let mut items: Vec<LeftoverItem> = Vec::new();
    let Ok(entries) = std::fs::read_dir("/var/db/receipts") else {
        return items;
    };
    let id = bundle_id.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let mut matched: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let lowered = file_name.to_ascii_lowercase();
        let hit = (!id.is_empty() && lowered.starts_with(&id))
            || (name.chars().count() >= 3 && lowered.starts_with(&name));
        if hit {
            matched.push(entry.path());
        }
    }
    matched.sort();
    for path in matched {
        items.push(LeftoverItem {
            path: path.to_string_lossy().to_string(),
            kind: "receipt".to_string(),
            size_bytes: path_size(&path),
            is_dir: false,
            confidence: "low".to_string(),
            // 收据不可删，所以默认不勾选
            selected: false,
            requires_elevation: true,
            exists: true,
        });
    }
    items
}

/// 扫描某应用的残留：候选 → 判存在 → 补收据。
pub fn scan_leftovers(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<Vec<LeftoverItem>, String> {
    let bundle = bundle_from_id(app_id)?;
    let info = read_bundle_info(&bundle).or_else(|| {
        // 应用已被移入废纸篓：用列表缓存里的名字 + detail（bundle id）兜底
        hint.map(|app| BundleInfo {
            bundle_id: Some(app.detail.clone()).filter(|id| !id.is_empty()),
            name: app.name.clone(),
            version: app.version.clone(),
            executable: None,
        })
    });
    let (bundle_id, name) = match info {
        Some(info) => (info.bundle_id.unwrap_or_default(), info.name),
        None => {
            // 连缓存都没有：只剩「按包名匹配」
            let name = bundle
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            (String::new(), name)
        }
    };

    emit(CleanerProgress::new(
        "scan_leftovers",
        "scanning",
        0,
        0,
        format!("正在扫描 {} 的残留…", name),
    ));
    let mut items = fill_existing(mac_leftover_candidates(&bundle_id, &name));
    items.extend(receipt_candidates(&bundle_id, &name));
    emit(CleanerProgress::new(
        "scan_leftovers",
        "done",
        items.len(),
        items.len(),
        format!("找到 {} 条残留", items.len()),
    ));
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_info_from_plist_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Foo.app/Contents");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("Info.plist"),
            r#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.example.foo</string>
<key>CFBundleName</key><string>Foo</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
</dict></plist>"#,
        )
        .unwrap();
        let info = read_bundle_info(&dir.path().join("Foo.app")).unwrap();
        assert_eq!(info.bundle_id.as_deref(), Some("com.example.foo"));
        assert_eq!(info.name, "Foo");
        assert_eq!(info.version, "1.2.3");
    }

    #[test]
    fn leftover_candidates_for_bundle_id() {
        let paths = mac_leftover_candidates("com.example.foo", "Foo");
        assert!(paths
            .iter()
            .any(|p| p.path.ends_with("Library/Caches/com.example.foo")));
        assert!(paths.iter().any(|p| p.kind == "preference"));
        assert!(paths
            .iter()
            .any(|p| p.path.starts_with("/Library/") && p.requires_elevation));
        // 柠檬补充根：按应用名精确匹配
        assert!(paths.iter().any(|p| p.path == "/Library/Extensions/Foo.kext"));
        assert!(paths.iter().any(|p| p.path == "/Library/PreferencePanes/Foo.prefPane"));
        // 废纸篓规则：low 且默认不勾选
        let trash_item = paths
            .iter()
            .find(|p| p.path.contains("/.Trash/"))
            .expect("应该有废纸篓候选");
        assert_eq!(trash_item.confidence, "low");
        assert!(!trash_item.selected);
    }

    #[test]
    fn system_app_is_not_uninstallable() {
        assert!(!is_uninstallable(Path::new("/System/Applications/Safari.app")));
        // 非系统路径（不存在也只是「非系统」，卸载阶段会报应用不存在）
        assert!(is_uninstallable(Path::new("/Applications/Foo.app")));
    }

    #[test]
    fn size_counting_does_not_follow_symlink_loops() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Loop.app/Contents/Resources");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("data.bin"), b"0123456789").unwrap();
        // 指回上层的符号链接：跟随就会无限递归
        std::os::unix::fs::symlink(dir.path(), app.join("loop")).unwrap();
        assert_eq!(dir_size_limited(&dir.path().join("Loop.app"), 64, 10_000), 10);
    }
}
