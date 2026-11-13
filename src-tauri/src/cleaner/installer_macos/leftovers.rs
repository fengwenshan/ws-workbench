use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::super::models::{CleanerProgress, InstalledApp, LeftoverItem};
use super::super::{home_dir, path_size};
use super::bundle::{read_bundle_info, BundleInfo};
use super::bundle_from_id;

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
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    mac_leftover_candidates_in(&home, bundle_id, name)
}

/// 与 `mac_leftover_candidates` 相同，但 home 由调用方传入（单测用临时目录）。
pub(super) fn mac_leftover_candidates_in(
    home: &Path,
    bundle_id: &str,
    name: &str,
) -> Vec<LeftoverItem> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let id = bundle_id.trim();
    let name = name.trim();

    if !id.is_empty() {
        // high：按 bundle id 精确匹配用户目录下的标准残留位
        for (sub, kind, is_dir) in [
            ("Library/Caches", "cache", true),
            ("Library/Application Support", "app_data", true),
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
        // Saved Application State 的实际目录名带 `.savedState` 后缀
        candidates.push(Candidate::new(
            home.join("Library/Saved Application State").join(format!("{}.savedState", id)),
            "app_data",
            "high",
            false,
            true,
        ));
        candidates.push(Candidate::new(
            home.join("Library/Preferences").join(format!("{}.plist", id)),
            "preference",
            "high",
            false,
            false,
        ));
        // ByHost：`<bundleId>.<UUID>.plist`，也覆盖 `<bundleId>.ShipIt.<UUID>.plist`
        if let Ok(entries) = std::fs::read_dir(home.join("Library/Preferences/ByHost")) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with(&format!("{}.", id)) && file_name.ends_with(".plist") {
                    candidates.push(Candidate::new(
                        entry.path(),
                        "preference",
                        "high",
                        false,
                        false,
                    ));
                }
            }
        }
        candidates.push(Candidate::new(
            home.join("Library/HTTPStorages").join(format!("{}.binarycookies", id)),
            "cache",
            "high",
            false,
            false,
        ));
        candidates.push(Candidate::new(
            home.join("Library/Cookies").join(format!("{}.binarycookies", id)),
            "cache",
            "medium",
            false,
            false,
        ));
        // 崩溃报告 / 诊断报告：文件名形如 `<应用名>_<UUID>.plist` 或 `<bundleId>_...`
        for (dir, kind, confidence) in [
            ("Library/Application Support/CrashReporter", "crash_report", "medium"),
            ("Library/Logs/DiagnosticReports", "log", "medium"),
        ] {
            let dir = home.join(dir);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let hit = (!name.is_empty() && file_name.starts_with(&format!("{}_", name)))
                    || file_name.starts_with(&format!("{}_", id));
                if hit {
                    let path = entry.path();
                    let is_dir = path.is_dir();
                    candidates.push(Candidate::new(path, kind, confidence, false, is_dir));
                }
            }
        }
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

    // 低置信度深扫：固定规则覆盖不到的厂商目录 / helper 容器 / ByHost 变体。
    // 放在固定候选之后，去重时保留前面的 high/medium 规则。
    candidates.extend(deep_scan_candidates(home, id, name));

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

/// 深扫根与深度：(相对 home 的目录, 最大深度, 残留类型)。
///
/// 深度 1 只扫目录自身的一层子项（如 `Containers/<id>.MiniProgram`），
/// 深度 2/3 用来覆盖厂商自己的二级目录（如 `Caches/Google/Chrome`）。
const DEEP_ROOTS: &[(&str, usize, &str)] = &[
    ("Library/Caches", 2, "cache"),
    ("Library/Application Support", 3, "app_data"),
    ("Library/Containers", 1, "container"),
    ("Library/Group Containers", 1, "container"),
    ("Library/HTTPStorages", 1, "cache"),
    ("Library/WebKit", 1, "cache"),
    ("Library/Application Scripts", 1, "app_data"),
    ("Library/Saved Application State", 1, "app_data"),
    ("Library/Preferences", 1, "preference"),
    ("Library/Preferences/ByHost", 1, "preference"),
    ("Library/Logs", 2, "log"),
];

/// 深扫最多返回多少条：防止某个应用名太通用时一次扫出几千条把弹窗卡死。
const DEEP_SCAN_LIMIT: usize = 200;

/// 去掉 macOS 系统给残留文件名加的后缀，便于和 bundle id / 应用名比较。
fn strip_known_suffix(name: &str) -> &str {
    for suffix in [".savedState", ".binarycookies", ".plist"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped;
        }
    }
    name
}

/// 深扫的名字匹配：bundle id 精确/前缀/后缀 + 应用名精确 + bundle id 末段精确。
///
/// 只做「名字等于 id / 以 `id.` 开头 / 以 `.id` 结尾」这类带边界的匹配，
/// 不做 contains，避免把 `com.example.foobar` 误算成 `com.example.foo` 的残留。
fn deep_name_match(candidate: &str, id: &str, app_name: &str, id_last: &str) -> bool {
    let bare = strip_known_suffix(candidate);
    let with_id = |value: &str| -> bool {
        !id.is_empty()
            && (value == id
                || value.starts_with(&format!("{}.", id))
                || value.ends_with(&format!(".{}", id)))
    };
    if with_id(candidate) || with_id(bare) {
        return true;
    }
    if !app_name.is_empty() && (candidate == app_name || bare == app_name) {
        return true;
    }
    // 末段太短时（如 `com.a.b` 的 `b`）容易误伤，只在 >= 4 个字符时启用
    id_last.len() >= 4 && (candidate == id_last || bare == id_last)
}

/// 低置信度深扫：只生成候选，全部 `selected=false`，由用户自己判断。
fn deep_scan_candidates(home: &Path, id: &str, name: &str) -> Vec<Candidate> {
    let id_last = id.rsplit('.').next().unwrap_or("");
    let mut out: Vec<Candidate> = Vec::new();
    'roots: for (root, depth, kind) in DEEP_ROOTS {
        let walker = walkdir::WalkDir::new(home.join(root))
            .min_depth(1)
            .max_depth(*depth)
            .follow_links(false)
            .into_iter()
            // 隐藏目录（如 `Library/.hidden`）既不是残留也不该继续往下走
            .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
        for entry in walker.filter_map(Result::ok) {
            if out.len() >= DEEP_SCAN_LIMIT {
                break 'roots;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !deep_name_match(&file_name, id, name, id_last) {
                continue;
            }
            let is_dir = entry.file_type().is_dir();
            out.push(Candidate {
                path: entry.path().to_path_buf(),
                kind,
                confidence: "low",
                requires_elevation: false,
                is_dir,
                selected: false,
            });
        }
    }
    out
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
