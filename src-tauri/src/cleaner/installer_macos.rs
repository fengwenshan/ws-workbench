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

use super::models::{
    CleanerProgress, InstalledApp, LeftoverItem, RelatedFileItem, RelatedFiles, UninstallOutcome,
};
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

/// `.app` 目录（不要求能解析 Info.plist：半残副本也要能被列出来清理）
fn is_app_dir(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("app") && path.is_dir()
}

fn is_app_bundle(path: &Path) -> bool {
    is_app_dir(path) && path.join("Contents").join("Info.plist").is_file()
}

// ============================== 自身识别 ==============================

/// 当前进程所在的 `.app` 包。
///
/// 发布版从 `.app/Contents/MacOS/<bin>` 启动，向上一定能找到 `.app`；
/// dev 构建是 `target/debug/ws-workbench-dev` 这样的裸二进制，返回 None
/// —— 此时「不能卸载自己」的过滤不生效（列表里也不会有裸二进制）。
pub(crate) fn current_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    app_bundle_from_exe(&exe)
}

/// 从可执行文件路径向上找第一个 `.app` 目录祖先（单测注入 exe 用）。
///
/// 要求是**目录**且后缀为 `.app`，避免把恰好以 `.app` 结尾的可执行文件误判成包。
pub(crate) fn app_bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.ancestors().find(|ancestor| {
        ancestor.extension().and_then(|ext| ext.to_str()) == Some("app") && ancestor.is_dir()
    })?;
    Some(bundle.canonicalize().unwrap_or_else(|_| bundle.to_path_buf()))
}

/// 目标 bundle 是否就是当前运行的应用本体（canonical 路径比较，软链也能识别）。
pub(crate) fn is_self_bundle(bundle: &Path) -> bool {
    let Some(self_bundle) = current_app_bundle() else {
        return false;
    };
    same_path(&self_bundle, bundle)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
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
    // 不能卸载自己：过滤掉当前运行的 `.app` 本体。
    // dev 裸二进制不在 bundle 内（current_app_bundle 为 None），不会误过滤。
    let self_bundle = current_app_bundle();
    apps.retain(|app| match (&self_bundle, app.id.strip_prefix("mac:")) {
        (Some(self_bundle), Some(path)) => !same_path(self_bundle, Path::new(path)),
        _ => true,
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
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    mac_leftover_candidates_in(&home, bundle_id, name)
}

/// 与 `mac_leftover_candidates` 相同，但 home 由调用方传入（单测用临时目录）。
fn mac_leftover_candidates_in(home: &Path, bundle_id: &str, name: &str) -> Vec<LeftoverItem> {
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
    super::related::last_used_text_from(&raw, chrono::Local::now().fixed_offset())
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

/// 是否位于废纸篓（`~/.Trash` 前缀，或任一路径组件为 `.Trash`）。
///
/// 关联文件清单绝不列废纸篓条目：`trash_guard::trash_path` 对废纸篓内的路径走
/// `remove_permanently`（永久删除），而卸载窗口的「全选」会把这些条目送进删除
/// 命令 —— 等于顺手清空用户废纸篓，不属于「卸载」语义。
fn is_trash_path(path: &Path) -> bool {
    let under_home_trash = home_dir()
        .map(|home| path.starts_with(home.join(".Trash")))
        .unwrap_or(false);
    under_home_trash
        || path
            .components()
            .any(|component| component.as_os_str() == ".Trash")
}

/// 关联文件清单的废纸篓过滤：程序文件副本在 `filter_bundle_copies` 里已过滤，
/// 这里对支持/缓存（`mac_leftover_candidates` 会带 `~/.Trash/<应用名>` 这类 low
/// 条目）再兜一层，保证整个 `RelatedFiles` 里不会出现任何废纸篓路径。
fn retain_non_trash(items: &mut Vec<RelatedFileItem>) {
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
    let files = super::related::build_related_files(app_id, items, last_used);
    emit(CleanerProgress::new(
        "related_files",
        "done",
        files.item_count,
        files.item_count,
        format!("找到 {} 个关联文件", files.item_count),
    ));
    Ok(files)
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
    fn saved_state_uses_saved_state_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        let saved = paths
            .iter()
            .find(|item| item.path.contains("Saved Application State"))
            .expect("应该有 savedState 候选");
        assert_eq!(saved.kind, "app_data");
        assert_eq!(saved.confidence, "high");
        assert!(
            saved.path.ends_with("Library/Saved Application State/com.example.foo.savedState"),
            "实际路径 {}",
            saved.path
        );
    }

    #[test]
    fn byhost_and_crash_report_rules_match_real_names() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for rel in [
            "Library/Preferences/ByHost",
            "Library/Application Support/CrashReporter",
            "Library/Logs/DiagnosticReports",
        ] {
            std::fs::create_dir_all(home.join(rel)).unwrap();
        }
        std::fs::write(
            home.join("Library/Preferences/ByHost/com.example.foo.AAAA.plist"),
            b"x",
        )
        .unwrap();
        std::fs::write(
            home.join("Library/Application Support/CrashReporter/Foo_AAAA.plist"),
            b"x",
        )
        .unwrap();
        std::fs::write(
            home.join("Library/Logs/DiagnosticReports/Foo_2026-11-09.ips"),
            b"x",
        )
        .unwrap();
        std::fs::write(
            home.join("Library/Logs/DiagnosticReports/com.example.foo_2026.ips"),
            b"x",
        )
        .unwrap();

        let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("ByHost/com.example.foo.AAAA.plist")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("CrashReporter/Foo_AAAA.plist")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("DiagnosticReports/Foo_2026-11-09.ips")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("DiagnosticReports/com.example.foo_2026.ips")));
    }

    #[test]
    fn binarycookie_candidates_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("HTTPStorages/com.example.foo.binarycookies")));
        assert!(paths
            .iter()
            .any(|item| item.path.ends_with("Cookies/com.example.foo.binarycookies")));
    }

    #[test]
    fn deep_scan_finds_helper_containers_and_group_containers() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for rel in [
            "Library/Containers/com.example.foo.MiniProgram",
            "Library/Group Containers/TEAM.com.example.foo",
            "Library/Application Scripts/com.example.foo.MiniProgram",
        ] {
            std::fs::create_dir_all(home.join(rel)).unwrap();
        }
        let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        for expected in [
            "Library/Containers/com.example.foo.MiniProgram",
            "Library/Group Containers/TEAM.com.example.foo",
            "Library/Application Scripts/com.example.foo.MiniProgram",
        ] {
            let item = items
                .iter()
                .find(|item| item.path == home.join(expected).to_string_lossy())
                .unwrap_or_else(|| panic!("深扫漏了 {}", expected));
            // Group Containers 已有固定 high 规则；两个 helper 容器来自深扫
            if expected.contains("Group Containers") {
                assert_eq!(item.confidence, "high");
            } else {
                assert_eq!(item.confidence, "low");
                assert!(!item.selected);
            }
        }
    }

    #[test]
    fn deep_scan_finds_vendor_dirs_by_bundle_id_last_segment() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for rel in [
            "Library/Application Support/Google/Chrome",
            "Library/Caches/Google/Chrome",
        ] {
            std::fs::create_dir_all(home.join(rel)).unwrap();
        }
        let items = mac_leftover_candidates_in(home, "com.google.Chrome", "Google Chrome");
        for expected in [
            "Library/Application Support/Google/Chrome",
            "Library/Caches/Google/Chrome",
        ] {
            let item = items
                .iter()
                .find(|item| item.path == home.join(expected).to_string_lossy())
                .unwrap_or_else(|| panic!("深扫漏了 {}", expected));
            assert_eq!(item.confidence, "low");
            assert!(!item.selected);
        }
    }

    #[test]
    fn deep_scan_skips_hidden_dirs_and_dedupes_fixed_rules() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("Library/.hidden/com.example.foo")).unwrap();
        std::fs::create_dir_all(home.join("Library/Preferences/ByHost")).unwrap();
        std::fs::write(
            home.join("Library/Preferences/ByHost/com.example.foo.ShipIt.AAAA.plist"),
            b"x",
        )
        .unwrap();
        let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        assert!(!items.iter().any(|item| item.path.contains("/.hidden/")));
        let shipit: Vec<&LeftoverItem> = items
            .iter()
            .filter(|item| item.path.ends_with("com.example.foo.ShipIt.AAAA.plist"))
            .collect();
        assert_eq!(shipit.len(), 1, "固定规则与深扫不应重复");
        assert_eq!(shipit[0].confidence, "high");
        assert!(shipit[0].selected);
    }

    #[test]
    fn deep_scan_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for index in 0..250 {
            std::fs::create_dir_all(
                home.join("Library/Containers").join(format!("com.example.foo.ext{index}")),
            )
            .unwrap();
        }
        let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
        let deep = items
            .iter()
            .filter(|item| {
                item.confidence == "low" && item.path.contains("Library/Containers/")
            })
            .count();
        assert!(deep > 0, "应该扫到低置信度条目");
        assert!(deep <= 200, "深扫应截断到 200 条，实际 {deep}");
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

    // ============================== 自身识别 ==============================

    #[test]
    fn current_app_bundle_is_none_for_bare_binary() {
        // cargo test 的二进制在 target/debug/deps 下，不在 .app 内：
        // dev 构建必须返回 None，否则「不能卸载自己」会把整个列表清空
        assert!(
            current_app_bundle().is_none(),
            "测试二进制不应被识别成 .app 内的进程"
        );
    }

    #[test]
    fn app_bundle_from_exe_walks_up_to_dot_app() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Foo.app");
        let exe = bundle.join("Contents/MacOS/foo");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();

        let found = app_bundle_from_exe(&exe).expect("应能向上找到 .app");
        assert_eq!(found.file_name().unwrap(), "Foo.app");
        // canonicalize 之后应等于 bundle 的真实路径（/var → /private/var 也一致）
        assert_eq!(found, bundle.canonicalize().unwrap());

        // 不在 bundle 内的普通二进制返回 None
        let plain = dir.path().join("plain-bin");
        std::fs::write(&plain, b"x").unwrap();
        assert!(app_bundle_from_exe(&plain).is_none());
    }

    #[test]
    fn is_self_bundle_is_false_for_other_bundles() {
        // 测试进程不在 .app 内 → 任何 bundle 都不算自己
        assert!(!is_self_bundle(Path::new("/Applications/Foo.app")));
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Foo.app");
        std::fs::create_dir_all(&bundle).unwrap();
        assert!(!is_self_bundle(&bundle));
    }

    // ============================== 关联文件（卸载窗口） ==============================

    fn write_bundle(root: &Path, name: &str, bundle_id: &str, version: &str) -> PathBuf {
        let bundle = root.join(format!("{}.app", name));
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{bundle_id}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        bundle
    }

    fn related_item(path: &str, kind: &str, confidence: &str) -> RelatedFileItem {
        RelatedFileItem {
            path: path.to_string(),
            name: Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            kind: kind.to_string(),
            size_bytes: 1,
            is_dir: true,
            version: String::new(),
            confidence: confidence.to_string(),
            requires_elevation: false,
            exists: true,
            selected: false,
        }
    }

    #[test]
    fn is_trash_path_matches_home_trash_and_trash_component() {
        let home = home_dir().expect("测试环境应该有 HOME");
        assert!(is_trash_path(&home.join(".Trash/Foo.app")));
        assert!(is_trash_path(Path::new("/Volumes/Backup/.Trash/Foo.app")));
        assert!(!is_trash_path(Path::new("/Applications/Foo.app")));
        assert!(!is_trash_path(&home.join("Library/Caches/com.example.foo")));
        // 名字里带 .Trash 但不是路径组件的目录不算
        assert!(!is_trash_path(Path::new("/Applications/.Trashy/Foo.app")));
    }

    #[test]
    fn related_list_never_contains_trash_entries() {
        let home = home_dir().expect("测试环境应该有 HOME");
        let mut items = vec![
            related_item("/Applications/Foo.app", "app_bundle", "high"),
            // `mac_leftover_candidates` 会给的两条废纸篓 low 条目（支持/缓存也要过滤）
            related_item(
                &home.join(".Trash").join("Foo").to_string_lossy(),
                "cache",
                "low",
            ),
            related_item(
                &home.join(".Trash").join("com.example.foo").to_string_lossy(),
                "cache",
                "low",
            ),
            // 非废纸篓的 low 条目不受影响：照常列出、但默认不勾选
            related_item(
                "/Users/me/Library/Caches/com.example.foo/vendor",
                "cache",
                "low",
            ),
        ];
        retain_non_trash(&mut items);
        let files = super::super::related::build_related_files(
            "mac:/Applications/Foo.app",
            items,
            String::new(),
        );
        let paths: Vec<&str> = files
            .groups
            .iter()
            .flat_map(|group| group.items.iter().map(|item| item.path.as_str()))
            .collect();
        assert!(
            paths.iter().all(|path| !path.contains("/.Trash/")),
            "关联文件清单里不应出现废纸篓条目: {:?}",
            paths
        );
        assert!(
            paths.iter().any(|path| path.ends_with("vendor")),
            "非废纸篓的 low 条目照常列出: {:?}",
            paths
        );
        let low = files
            .groups
            .iter()
            .flat_map(|group| group.items.iter())
            .find(|item| item.confidence == "low")
            .expect("应有 low 条目");
        assert!(!low.selected, "low 默认不勾选；全选是用户显式动作");
    }

    #[test]
    fn app_bundle_copies_in_finds_copies_and_skips_trash() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("Applications");
        let trash = dir.path().join(".Trash");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&trash).unwrap();

        let main = write_bundle(&apps, "Main", "com.example.foo", "1.0.0");
        let _copy = write_bundle(&apps, "Copy", "com.example.foo", "1.1.0");
        // 不同 bundle id 的 .app 不能算副本
        let _other = write_bundle(&apps, "Other", "com.example.bar", "2.0.0");
        // 废纸篓里的同名副本不列（删它等于永久删除）
        let _trashed = write_bundle(&trash, "OldCopy", "com.example.foo", "0.9.0");

        let copies = app_bundle_copies_in(
            &main,
            "com.example.foo",
            &[apps.clone(), trash.clone()],
        );
        assert_eq!(copies.len(), 1, "实际副本: {:?}", copies);
        assert_eq!(
            copies[0].file_name().unwrap().to_string_lossy(),
            "Copy.app",
            "应只保留同 bundle id 且非废纸篓的副本"
        );
        // 主 bundle 自己不会出现在副本列表里
        assert!(!copies.iter().any(|path| path.file_name().unwrap() == "Main.app"));
        // 不同 id 的不会出现
        assert!(!copies.iter().any(|path| path.file_name().unwrap() == "Other.app"));
    }
}
