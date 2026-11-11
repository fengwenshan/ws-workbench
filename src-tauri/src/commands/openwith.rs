//! 「用其它应用打开目录」的应用目录与启动。
//!
//! 目录表参照 DeepSeek Harness 的 open-in-app catalog：每个应用声明自己
//! 在各平台可能的安装位置，运行时按顺序取第一个存在的，一个都找不到就
//! 不进菜单（所以菜单里永远只出现真正装了的东西）。
//!
//! 图标来源按平台区分：
//! - macOS：`sips` 把包里的 `.icns` 转成 64px PNG
//! - Windows：`PrivateExtractIcons` 从可执行文件里取 64px 图标，转 PNG
//!
//! 两者都编码成 data URL 交给前端直接 `<img>` 显示；取不到图标时前端用
//! 首字母兜底。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
use base64::Engine as _;
use serde::Serialize;
use tauri::AppHandle;

/// 打开方式菜单里的一项
#[derive(Debug, Clone, Serialize)]
pub struct OpenWithApp {
    /// 稳定标识，前端记住用户上次的选择
    pub id: String,
    /// 展示名（中文优先）
    pub name: String,
    /// `data:image/png;base64,...`；取不到图标时为 None
    pub icon: Option<String>,
    /// 平台默认应用（访达 / 资源管理器），菜单里标注「（默认）」
    pub is_default: bool,
}

/// 扫描结果缓存：图标提取要起 `sips`，没必要每次打开菜单都重算
static CACHE: Mutex<Option<Vec<OpenWithApp>>> = Mutex::new(None);

// ============================== 命令 ==============================

/// 列出本机可用的打开方式（首次调用会扫描并缓存）
///
/// 扫描要读注册表、还要起 PowerShell 批量取图标，冷启动几秒起步。这里必须走
/// `async` + `spawn_blocking`：同步命令在 Tauri 里跑在主线程上，会把首屏
/// （`app_ready`）一直堵在后面，表现就是启动时窗口不出来、鼠标一直转圈。
#[tauri::command]
pub async fn list_open_with_apps(app: AppHandle) -> Vec<OpenWithApp> {
    tauri::async_runtime::spawn_blocking(move || cached_scan(app))
        .await
        .unwrap_or_default()
}

/// 重新扫描（用户刚装完软件时用）
#[tauri::command]
pub async fn refresh_open_with_apps(app: AppHandle) -> Vec<OpenWithApp> {
    tauri::async_runtime::spawn_blocking(move || {
        let apps = scan(&app);
        if let Ok(mut cache) = CACHE.lock() {
            *cache = Some(apps.clone());
        }
        apps
    })
    .await
    .unwrap_or_default()
}

fn cached_scan(app: AppHandle) -> Vec<OpenWithApp> {
    if let Ok(cache) = CACHE.lock() {
        if let Some(apps) = cache.as_ref() {
            return apps.clone();
        }
    }
    let apps = scan(&app);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(apps.clone());
    }
    apps
}

/// 用指定应用打开目录。
///
/// 目录不存在时退到最近存在的上级目录，和原来的「打开」行为一致，
/// 返回不再需要实际打开的路径 —— 前端只看成功/失败。
#[tauri::command]
pub fn open_with(app: AppHandle, path: String, app_id: String) -> Result<(), String> {
    let target = crate::commands::nearest_existing_dir(&path);
    if !target.is_dir() {
        return Err(format!("路径不存在: {}", path));
    }
    let dir = target.to_string_lossy().to_string();
    platform_open(&app, &app_id, &dir)
}

// ============================== 扫描 ==============================

fn scan(app: &AppHandle) -> Vec<OpenWithApp> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        mac_scan()
    }
    #[cfg(not(target_os = "macos"))]
    {
        other_scan(app)
    }
}

fn platform_open(app: &AppHandle, app_id: &str, dir: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        mac_open(app, app_id, dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        other_open(app, app_id, dir)
    }
}

// ============================== macOS ==============================

/// 一个可以被「用...打开」的 macOS 应用
#[cfg(target_os = "macos")]
struct MacApp {
    id: &'static str,
    name: &'static str,
    /// `.app` 包名候选，按顺序在下面几个目录里找
    bundles: &'static [&'static str],
    /// 不在常规目录里的（访达、终端）直接给绝对路径
    fixed: Option<&'static str>,
}

/// 菜单顺序：文件管理器 → 编辑器 / IDE → 终端。
/// 只写常见的安装拼写；装在别处或改了名的包不参与探测。
#[cfg(target_os = "macos")]
const MAC_CATALOG: &[MacApp] = &[
    MacApp {
        id: "finder",
        name: "访达",
        bundles: &[],
        fixed: Some("/System/Library/CoreServices/Finder.app"),
    },
    MacApp {
        id: "deepseek",
        name: "DeepSeek",
        bundles: &["DeepSeek Harness.app"],
        fixed: None,
    },
    MacApp {
        id: "vscode",
        name: "VS Code",
        bundles: &["Visual Studio Code.app", "Visual Studio Code - Insiders.app"],
        fixed: None,
    },
    MacApp {
        id: "trae",
        name: "Trae",
        bundles: &["Trae CN.app", "Trae.app", "TRAE SOLO CN.app"],
        fixed: None,
    },
    MacApp {
        id: "cursor",
        name: "Cursor",
        bundles: &["Cursor.app"],
        fixed: None,
    },
    MacApp {
        id: "xcode",
        name: "Xcode",
        bundles: &["Xcode.app"],
        fixed: None,
    },
    MacApp {
        id: "intellij",
        name: "IntelliJ IDEA",
        bundles: &[
            "IntelliJ IDEA.app",
            "IntelliJ IDEA Ultimate.app",
            "IntelliJ IDEA CE.app",
            "IntelliJ IDEA Community Edition.app",
        ],
        fixed: None,
    },
    MacApp {
        id: "pycharm",
        name: "PyCharm",
        bundles: &[
            "PyCharm.app",
            "PyCharm Professional.app",
            "PyCharm CE.app",
            "PyCharm Community.app",
        ],
        fixed: None,
    },
    MacApp {
        id: "webstorm",
        name: "WebStorm",
        bundles: &["WebStorm.app"],
        fixed: None,
    },
    MacApp {
        id: "goland",
        name: "GoLand",
        bundles: &["GoLand.app"],
        fixed: None,
    },
    MacApp {
        id: "iterm",
        name: "iTerm",
        bundles: &["iTerm.app"],
        fixed: None,
    },
    MacApp {
        id: "terminal",
        name: "终端",
        bundles: &["Terminal.app"],
        fixed: Some("/System/Applications/Utilities/Terminal.app"),
    },
];

/// 在 `/Applications`、`~/Applications`、系统目录里找 `spec` 对应的包
#[cfg(target_os = "macos")]
fn mac_bundle_path(spec: &MacApp) -> Option<PathBuf> {
    if let Some(fixed) = spec.fixed {
        let path = PathBuf::from(fixed);
        return path.exists().then_some(path);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots.push(PathBuf::from("/Applications"));
    roots.push(PathBuf::from("/System/Applications"));
    roots.push(PathBuf::from("/System/Applications/Utilities"));
    spec.bundles.iter().find_map(|name| {
        roots
            .iter()
            .map(|root| root.join(name))
            .find(|path| path.exists())
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn deepseek_bundle_path() -> Option<PathBuf> {
    MAC_CATALOG
        .iter()
        .find(|spec| spec.id == "deepseek")
        .and_then(mac_bundle_path)
}

/// 找出一个 `.app` 真正该用的图标文件。
///
/// 顺序和 DeepSeek Harness 一致：先读 `Info.plist` 的 `CFBundleIconFile`
/// （没有扩展名时补 `.icns`），再退化成和包同名的 `.icns`，最后才扫描
/// `Resources` 里第一个 `.icns`。
///
/// 这个顺序不能省：VS Code 的 `Resources` 里有 29 个 icns，语言文件图标
/// （`javascript.icns` 之类）和它混在一起，直接扫目录会把 JS 图标当成应用图标。
#[cfg(target_os = "macos")]
fn mac_bundle_icon_path(bundle: &Path) -> Option<PathBuf> {
    let resources = bundle.join("Contents").join("Resources");
    let plist = bundle.join("Contents").join("Info.plist");

    if let Ok(output) = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&plist)
        .output()
    {
        if output.status.success() {
            if let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(name) = meta.get("CFBundleIconFile").and_then(|value| value.as_str()) {
                    if !name.is_empty() {
                        let file = if name.to_ascii_lowercase().ends_with(".icns") {
                            name.to_string()
                        } else {
                            format!("{}.icns", name)
                        };
                        let candidate = resources.join(file);
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }

    if let Some(stem) = bundle.file_stem().and_then(|stem| stem.to_str()) {
        let candidate = resources.join(format!("{}.icns", stem));
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    std::fs::read_dir(&resources)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("icns"))
        })
}

/// 把一个 `.app` 的图标转成 64px PNG 的 data URL
#[cfg(target_os = "macos")]
pub(crate) fn mac_icon(bundle: &Path) -> Option<String> {
    let icns = mac_bundle_icon_path(bundle)?;

    let png = std::env::temp_dir().join(format!("ws-workbench-ow-{}.png", uuid::Uuid::new_v4()));
    let converted = Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", "64"])
        .arg(&icns)
        .arg("--out")
        .arg(&png)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    let bytes = if converted {
        std::fs::read(&png).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&png);
    let bytes = bytes?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(target_os = "macos")]
fn mac_scan() -> Vec<OpenWithApp> {
    MAC_CATALOG
        .iter()
        .filter_map(|spec| {
            let bundle = mac_bundle_path(spec)?;
            Some(OpenWithApp {
                id: spec.id.to_string(),
                name: spec.name.to_string(),
                icon: mac_icon(&bundle),
                is_default: spec.id == "finder",
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn mac_open(app: &AppHandle, app_id: &str, dir: &str) -> Result<(), String> {
    let spec = MAC_CATALOG
        .iter()
        .find(|spec| spec.id == app_id)
        .ok_or_else(|| format!("未知的打开方式: {}", app_id))?;
    let bundle = mac_bundle_path(spec).ok_or_else(|| format!("{} 没有安装", spec.name))?;

    // 访达就用 shell open，等价于双击目录
    if spec.id == "finder" {
        return crate::commands::open_path(dir);
    }

    // DeepSeek 不能把目录当参数传：写请求文件后唤起，工作区由 DSH 插件打开
    if spec.id == "deepseek" {
        return crate::commands::deepseek::open_dir_workspace(app, dir);
    }

    let output = Command::new("/usr/bin/open")
        .arg("-a")
        .arg(&bundle)
        .arg(dir)
        .output()
        .map_err(|err| format!("打开失败: {}", err))?;
    if output.status.success() {
        Ok(())
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "用 {} 打开失败{}",
            spec.name,
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {}", reason)
            }
        ))
    }
}

// ============================== Windows / Linux ==============================

/// 非 macOS 的菜单顺序：文件管理器 → 编辑器 / IDE → 终端。
/// 顺序和 DeepSeek Harness 的 open-in-app 一致，装了哪个才出现哪个。
#[cfg(not(target_os = "macos"))]
const DESKTOP_EDITORS: &[(&str, &str, crate::apps::AppId)] = &[
    ("vscode", "VS Code", crate::apps::AppId::Vscode),
    ("idea", "IntelliJ IDEA", crate::apps::AppId::Idea),
    ("pycharm", "PyCharm", crate::apps::AppId::Pycharm),
    ("webstorm", "WebStorm", crate::apps::AppId::Webstorm),
    ("goland", "GoLand", crate::apps::AppId::Goland),
    ("rustrover", "RustRover", crate::apps::AppId::Rustrover),
    ("cursor", "Cursor", crate::apps::AppId::Cursor),
    ("trae", "Trae", crate::apps::AppId::Trae),
    ("deepseek", "DeepSeek", crate::apps::AppId::DeepSeek),
];

/// 非 macOS 菜单里的一项。
///
/// 只带取图标用的路径：启动时按 `id` 重新定位（`open_with` 和扫描走同一套
/// [`crate::apps::locate`]，保证「菜单里看到的」就是「点下去启动的」）。
/// `icon_from` 大多数时候就是启动文件本身，但启动器可能是 `.cmd` 包装脚本
/// （Trae 的 `bin\trae.cmd`）或 0 字节的应用别名（`WindowsApps\wt.exe`），
/// 从它们身上只能抽到脚本 / 快捷方式的通用图标，这时图标要从背后的真正的
/// exe 上取。
#[cfg(not(target_os = "macos"))]
struct Entry {
    id: &'static str,
    name: &'static str,
    icon_from: Option<PathBuf>,
}

#[cfg(not(target_os = "macos"))]
impl Entry {
    /// 启动文件与图标来源相同的普通项（编辑器、资源管理器）
    fn simple(id: &'static str, name: &'static str, executable: Option<PathBuf>) -> Self {
        Self {
            id,
            name,
            icon_from: executable,
        }
    }
}

/// 非 macOS 的菜单：文件管理器 + 本机装了的外部应用 + 终端 + PowerShell。
///
/// `apps::locate` 会走注册表 / 常见安装目录 / PATH 六条来源，找不到的编辑器
/// 不进菜单，所以菜单里永远是点得开的项。图标从各自的可执行文件里取。
#[cfg(not(target_os = "macos"))]
fn other_scan(app: &AppHandle) -> Vec<OpenWithApp> {
    let ctx = crate::commands::app_context(app);
    collect(desktop_entries(&ctx))
}

/// 菜单条目：文件管理器 → 本机装了的外编辑器 → 终端 / PowerShell。
///
/// 和 `AppHandle` 无关，单独拆出来是为了能在测试里直接看菜单内容。
#[cfg(not(target_os = "macos"))]
fn desktop_entries(ctx: &crate::apps::AppContext) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();

    #[cfg(target_os = "windows")]
    entries.push(Entry::simple(
        "explorer",
        "资源管理器",
        system_exe("explorer.exe"),
    ));
    #[cfg(all(unix, not(target_os = "macos")))]
    entries.push(Entry::simple("filemanager", "文件管理器", None));

    for &(id, name, app_id) in DESKTOP_EDITORS {
        let location = crate::apps::locate(app_id, ctx, "");
        if location.available {
            // 找到的是 `.cmd` 启动器时换成它背后的 exe，图标才对
            let exec = real_executable(Path::new(&location.path));
            entries.push(Entry::simple(id, name, Some(exec)));
        }
    }

    #[cfg(target_os = "windows")]
    {
        // 终端：启动走 cmd（Windows 11 下控制台默认由 Windows Terminal 承载），
        // 图标从 WindowsTerminal.exe 取
        let icon_from = crate::commands::windows_terminal_icon().or_else(|| system_exe("cmd.exe"));
        entries.push(Entry {
            id: "terminal",
            name: "终端",
            icon_from,
        });

        let powershell = crate::commands::powershell_exe();
        entries.push(Entry::simple("powershell", "PowerShell", powershell));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    entries.push(Entry::simple("terminal", "终端", None));

    entries
}

/// 取图标并转成前端用的 `OpenWithApp`
#[cfg(not(target_os = "macos"))]
#[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
fn collect(mut entries: Vec<Entry>) -> Vec<OpenWithApp> {
    #[cfg(target_os = "windows")]
    let mut icons = {
        let paths = entries
            .iter()
            .filter_map(|entry| entry.icon_from.as_deref())
            .collect::<Vec<_>>();
        windows_icons(&paths)
    };
    #[cfg(target_os = "windows")]
    {
        // 受限环境里 PowerShell 可能读不了 WindowsApps 目录，这时终端图标
        // 退回 cmd.exe 的，别让菜单里空一格
        let terminal_icon_missing = entries
            .iter()
            .find(|entry| entry.id == "terminal")
            .and_then(|entry| entry.icon_from.as_deref())
            .is_some_and(|path| !icons.contains_key(path));
        if terminal_icon_missing {
            if let Some(cmd) = system_exe("cmd.exe") {
                if let Some(entry) = entries.iter_mut().find(|entry| entry.id == "terminal") {
                    entry.icon_from = Some(cmd.clone());
                }
                icons.extend(windows_icons(&[cmd.as_path()]));
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let icons: std::collections::HashMap<PathBuf, String> = std::collections::HashMap::new();

    entries
        .into_iter()
        .map(|entry| OpenWithApp {
            id: entry.id.to_string(),
            name: entry.name.to_string(),
            icon: entry
                .icon_from
                .as_deref()
                .and_then(|path| icons.get(path).cloned()),
            is_default: entry.id == "explorer" || entry.id == "filemanager",
        })
        .collect()
}

/// 系统目录里的可执行文件。
///
/// explorer.exe 在 `%SystemRoot%` 下，cmd.exe 在 `%SystemRoot%\System32`，
/// 两个位置都找一遍。
#[cfg(target_os = "windows")]
fn system_exe(name: &str) -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from)?;
    for candidate in [root.join(name), root.join("System32").join(name)] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 从 `.cmd` / `.bat` 启动器里找出它真正启动的 exe。
///
/// 这类包装脚本不长：Trae 的 `bin\trae.cmd` 里就写着
/// `"%~dp0..\Trae CN.exe" "%~dp0..\resources\app\out\cli.js" %*`。
/// 用它起程序的同时也用它的图标，否则菜单里显示的是 cmd 脚本的通用图标。
#[cfg(target_os = "windows")]
fn real_executable(path: &Path) -> PathBuf {
    let is_shim = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        .unwrap_or(false);
    if !is_shim {
        return path.to_path_buf();
    }
    shim_target(path).unwrap_or_else(|| path.to_path_buf())
}

#[cfg(target_os = "windows")]
fn shim_target(shim: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?;
    for raw in text.split(['"', '\r', '\n']) {
        let token = raw.trim();
        if !token.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        let candidate = if let Some(rest) = token.strip_prefix("%~dp0") {
            dir.join(rest.trim_start_matches(['\\', '/']))
        } else if Path::new(token).is_absolute() {
            PathBuf::from(token)
        } else {
            continue;
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 非 Windows 上没有 `.cmd` 包装脚本，原样返回
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn real_executable(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// 从可执行文件里顺带读到的展示信息：文件描述 + 图标。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
pub(crate) struct WindowsAppMeta {
    pub name: String,
    pub icon: Option<String>,
}

/// 批量取「可执行文件路径 → 展示名 + 64px 图标」，弹窗拦截的规则/记录列表用。
///
/// 和 [`windows_icons`] 共用同一段脚本，多读一个 `FileDescription`（资源管理器里
/// 显示的名字，比如 `WPS Office`）；没有版本信息时退回文件名。
#[cfg(target_os = "windows")]
pub(crate) fn windows_app_meta(
    paths: &[&Path],
) -> std::collections::HashMap<PathBuf, WindowsAppMeta> {
    use std::collections::HashMap;

    let mut metas: HashMap<PathBuf, WindowsAppMeta> = HashMap::new();
    if paths.is_empty() {
        return metas;
    }
    let list = paths
        .iter()
        .map(|path| format!("'{}'", path.to_string_lossy().replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let script = WINDOWS_ICON_SCRIPT.replace("{LIST}", &list);

    #[derive(serde::Deserialize)]
    struct MetaEntry {
        path: String,
        #[serde(default)]
        png: String,
        #[serde(default)]
        desc: String,
    }

    let Ok(value) = crate::winps::run_json(&script) else {
        return metas;
    };
    for entry in crate::winps::parse_list::<MetaEntry>(value) {
        let path = PathBuf::from(&entry.path);
        let fallback = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.path.clone());
        let name = entry.desc.trim().to_string();
        metas.insert(
            path,
            WindowsAppMeta {
                name: if name.is_empty() { fallback } else { name },
                icon: if entry.png.is_empty() {
                    None
                } else {
                    Some(format!("data:image/png;base64,{}", entry.png))
                },
            },
        );
    }
    metas
}

/// 从一组可执行文件里批量取 64px 图标，返回「可执行文件路径 → data URL」。
///
/// 一次 PowerShell 调用处理整张菜单：`PrivateExtractIcons` 直接向 shell 要
/// 指定尺寸的图标（比 `ExtractAssociatedIcon` 的 32px 清楚），取不到再退回
/// `ExtractAssociatedIcon`。结果由 [`CACHE`] 缓存，每次启动只算一次。
#[cfg(target_os = "windows")]
pub(crate) fn windows_icons(paths: &[&Path]) -> std::collections::HashMap<PathBuf, String> {
    use std::collections::HashMap;

    let mut icons: HashMap<PathBuf, String> = HashMap::new();
    if paths.is_empty() {
        return icons;
    }

    let list = paths
        .iter()
        .map(|path| format!("'{}'", path.to_string_lossy().replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");

    let script = WINDOWS_ICON_SCRIPT.replace("{LIST}", &list);

    #[derive(serde::Deserialize)]
    struct IconEntry {
        path: String,
        png: String,
    }

    let Ok(value) = crate::winps::run_json(&script) else {
        return icons;
    };
    for entry in crate::winps::parse_list::<IconEntry>(value) {
        if entry.png.is_empty() {
            continue;
        }
        icons.insert(
            PathBuf::from(&entry.path),
            format!("data:image/png;base64,{}", entry.png),
        );
    }
    icons
}

/// 批量提取图标的 PowerShell 脚本，`{LIST}` 会被替换成 `@('C:\a.exe','C:\b.exe')`
///
/// `PrivateExtractIcons` 能取到 64px 图标，但需要 `Add-Type` 现场编译，编译要往
/// `%TEMP%` 写临时文件 —— 受限环境（沙箱、组策略禁用临时目录）里会失败。
/// 所以编译放在 try 里：成功就用 64px，失败就退回 `ExtractAssociatedIcon` 的 32px，
/// 图标质量差一点，但不至于整个菜单没图标。
#[cfg(target_os = "windows")]
const WINDOWS_ICON_SCRIPT: &str = r#"Add-Type -AssemblyName System.Drawing
$shellReady = $false
try {
  Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public static class WsIconShell{[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern uint PrivateExtractIcons(string file,int index,int cx,int cy,IntPtr[] icons,uint[] ids,uint count,uint flags);[DllImport("user32.dll")] public static extern bool DestroyIcon(IntPtr icon);}'
  $shellReady = $true
} catch { }
$paths = @({LIST})
$out = @()
foreach ($path in $paths) {
  if (-not (Test-Path -LiteralPath $path)) { continue }
  $desc = ''
  try { $desc = ([System.Diagnostics.FileVersionInfo]::GetVersionInfo($path)).FileDescription } catch { }
  $icon = $null
  $handle = [IntPtr]::Zero
  if ($shellReady) {
    try {
      $handles = New-Object IntPtr[] 1
      $ids = New-Object uint32[] 1
      $count = [WsIconShell]::PrivateExtractIcons($path, 0, 64, 64, $handles, $ids, 1, 0)
      if ($count -gt 0 -and $handles[0] -ne [IntPtr]::Zero) {
        $handle = $handles[0]
        $icon = [System.Drawing.Icon]::FromHandle($handle)
      }
    } catch { }
  }
  if ($null -eq $icon) {
    try { $icon = [System.Drawing.Icon]::ExtractAssociatedIcon($path) } catch { }
  }
  if ($null -eq $icon) { continue }
  try {
    $bitmap = $icon.ToBitmap()
    $stream = New-Object System.IO.MemoryStream
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $out += [pscustomobject]@{ path = $path; png = [Convert]::ToBase64String($stream.ToArray()); desc = [string]$desc }
    $stream.Dispose()
    $bitmap.Dispose()
  } catch { }
  finally {
    if ($handle -ne [IntPtr]::Zero) { [void][WsIconShell]::DestroyIcon($handle) }
    $icon.Dispose()
  }
}
ConvertTo-Json -InputObject @($out) -Compress"#;

#[cfg(not(target_os = "macos"))]
fn other_open(app: &AppHandle, app_id: &str, dir: &str) -> Result<(), String> {
    if app_id == "explorer" || app_id == "filemanager" {
        return crate::commands::open_path(dir);
    }
    if app_id == "terminal" {
        return crate::commands::open_terminal_dir(dir);
    }
    #[cfg(target_os = "windows")]
    if app_id == "powershell" {
        return crate::commands::open_powershell_dir(dir);
    }
    if app_id == "deepseek" {
        return crate::commands::deepseek::open_dir_workspace(app, dir);
    }
    let app_id = match app_id {
        "vscode" => crate::apps::AppId::Vscode,
        "idea" => crate::apps::AppId::Idea,
        "pycharm" => crate::apps::AppId::Pycharm,
        "webstorm" => crate::apps::AppId::Webstorm,
        "goland" => crate::apps::AppId::Goland,
        "rustrover" => crate::apps::AppId::Rustrover,
        "cursor" => crate::apps::AppId::Cursor,
        "trae" => crate::apps::AppId::Trae,
        other => return Err(format!("未知的打开方式: {}", other)),
    };
    let ctx = crate::commands::app_context(app);
    let location = crate::apps::locate(app_id, &ctx, "");
    if !location.available {
        return Err(format!("未找到 {}。{}", app_id.display_name(), location.note));
    }
    // 和菜单里的图标保持一致：启动器是 .cmd 时用背后的 exe
    let executable = real_executable(Path::new(&location.path));
    crate::apps::finder::spawn_with_target(&executable, dir)
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;

    /// 真机验证：Windows 上能从可执行文件里取到 PNG 图标
    #[test]
    fn icons_are_extracted_from_executables() {
        let exe = system_exe("explorer.exe").expect("系统里应该有 explorer.exe");
        let icons = windows_icons(&[exe.as_path()]);
        let icon = icons
            .get(exe.as_path())
            .expect("explorer.exe 应该能取到图标");
        assert!(icon.starts_with("data:image/png;base64,"), "实际: {}", icon);
        assert!(icon.len() > 200, "图标数据太短: {}", icon.len());
    }

    /// 弹窗拦截用同一段脚本多取一个 `FileDescription` 当作应用展示名
    #[test]
    fn app_meta_reads_display_name_and_icon() {
        let exe = system_exe("explorer.exe").expect("系统里应该有 explorer.exe");
        let metas = windows_app_meta(&[exe.as_path()]);
        let meta = metas
            .get(exe.as_path())
            .expect("explorer.exe 应该能取到展示信息");
        assert!(!meta.name.trim().is_empty(), "展示名不能为空");
        let icon = meta.icon.as_deref().expect("explorer.exe 应该有图标");
        assert!(icon.starts_with("data:image/png;base64,"), "实际: {icon}");
    }

    /// 取不到图标时安静返回空表，不影响菜单
    #[test]
    fn missing_executable_has_no_icon() {
        let icons = windows_icons(&[Path::new(r"C:\definitely\not\here.exe")]);
        assert!(icons.is_empty());
    }

    /// 测试目录：放 `target/test-tmp` 下，避免受限环境里 %TEMP% 不可写
    fn test_dir(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `.cmd` 启动器要换成它背后真正的 exe，否则图标是脚本的通用图标
    #[test]
    fn cmd_shim_resolves_to_the_real_executable() {
        let dir = test_dir("trae-shim");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = dir.join("Trae CN.exe");
        std::fs::write(&exe, b"stub").unwrap();
        let shim = bin.join("trae.cmd");
        std::fs::write(
            &shim,
            "@echo off\r\n\"%~dp0..\\Trae CN.exe\" \"%~dp0..\\resources\\app\\out\\cli.js\" %*\r\n",
        )
        .unwrap();

        let resolved = real_executable(&shim);
        assert_eq!(
            resolved.canonicalize().unwrap(),
            exe.canonicalize().unwrap(),
            "实际解析到: {}",
            resolved.display()
        );

        // 本来就是 exe 的不动
        assert_eq!(real_executable(&exe), exe);
    }

    /// 真机自检：菜单里每一项都该有图标（除了确实没有图标的项）
    #[test]
    fn desktop_menu_has_icons_for_every_entry() {
        let entries = desktop_entries(&crate::apps::AppContext::default());
        let apps = collect(entries);
        let mut missing = Vec::new();
        for app in &apps {
            match &app.icon {
                Some(icon) => assert!(
                    icon.starts_with("data:image/png;base64,"),
                    "{} 的图标不是 data URL",
                    app.id
                ),
                None => missing.push(app.id.clone()),
            }
        }
        // 至少这些必须有图标
        for required in ["terminal", "powershell", "vscode"] {
            assert!(
                apps.iter()
                    .any(|app| app.id == required && app.icon.is_some()),
                "{} 没有图标，缺失项: {:?}",
                required,
                missing
            );
        }
    }

    /// 真机：终端图标来自 WindowsTerminal.exe（不是 0 字节的 wt.exe 别名），
    /// PowerShell 也有图标
    #[test]
    fn terminal_and_powershell_icons_are_extracted() {
        if let Some(icon_path) = crate::commands::windows_terminal_icon() {
            let icons = windows_icons(&[icon_path.as_path()]);
            let icon = icons
                .get(&icon_path)
                .expect("WindowsTerminal.exe 应该能取到图标");
            assert!(icon.starts_with("data:image/png;base64,"));
            assert!(icon.len() > 500, "图标数据太短: {}", icon.len());
        }

        let powershell = crate::commands::powershell_exe().expect("系统里应该有 PowerShell");
        let icons = windows_icons(&[powershell.as_path()]);
        assert!(
            icons.contains_key(&powershell),
            "PowerShell 没取到图标: {}",
            powershell.display()
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// 真机自检：目录表能探测到访达；能取到的图标一定是合法 PNG
    #[test]
    fn mac_scan_lists_installed_apps_with_valid_icons() {
        let apps = mac_scan();
        assert!(!apps.is_empty(), "连访达都没探测到");
        assert_eq!(apps[0].id, "finder");
        assert!(apps[0].is_default, "访达要标成默认项");
        for app in &apps {
            if let Some(icon) = &app.icon {
                let encoded = icon
                    .strip_prefix("data:image/png;base64,")
                    .expect("图标必须是 data URL");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .expect("图标 base64 解不开");
                assert_eq!(&bytes[..4], b"\x89PNG", "{} 的图标不是 PNG", app.name);
            }
        }
    }

    /// 「用 DeepSeek 打开」排在访达之后，名字就叫 DeepSeek
    #[test]
    fn mac_catalog_puts_deepseek_after_finder() {
        let ids: Vec<&str> = MAC_CATALOG.iter().map(|spec| spec.id).collect();
        let finder = ids.iter().position(|id| *id == "finder").unwrap();
        let deepseek = ids.iter().position(|id| *id == "deepseek").unwrap();
        assert_eq!(deepseek, finder + 1);
        let spec = MAC_CATALOG
            .iter()
            .find(|spec| spec.id == "deepseek")
            .unwrap();
        assert_eq!(spec.name, "DeepSeek");
        assert_eq!(spec.bundles, &["DeepSeek Harness.app"]);
    }

    /// 回归：带 `CFBundleIconFile` 的包必须用声明的那张 icns，不能拿 Resources 里第一张
    /// （VS Code 就是典型：29 张 icns 里混着语言文件图标）
    #[test]
    fn declared_bundle_icon_wins_over_directory_scan() {
        for spec in MAC_CATALOG {
            let Some(bundle) = mac_bundle_path(spec) else {
                continue;
            };
            let plist = bundle.join("Contents").join("Info.plist");
            let Ok(output) = Command::new("/usr/bin/plutil")
                .args(["-convert", "json", "-o", "-"])
                .arg(&plist)
                .output()
            else {
                continue;
            };
            if !output.status.success() {
                continue;
            }
            let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
                continue;
            };
            let Some(declared) = meta.get("CFBundleIconFile").and_then(|value| value.as_str()) else {
                continue;
            };
            if declared.is_empty() {
                continue;
            }
            let expected = if declared.to_ascii_lowercase().ends_with(".icns") {
                declared.to_string()
            } else {
                format!("{}.icns", declared)
            };
            let chosen = mac_bundle_icon_path(&bundle).expect("声明了图标却找不到文件");
            assert_eq!(
                chosen.file_name().and_then(|name| name.to_str()),
                Some(expected.as_str()),
                "{} 用了错误的 icns",
                spec.name
            );
        }
    }

    /// 目录表里每一项都要能落回一个真实存在的包，否则菜单会列出打不开的项
    #[test]
    fn every_listed_app_resolves_to_a_bundle() {
        for app in mac_scan() {
            let spec = MAC_CATALOG
                .iter()
                .find(|spec| spec.id == app.id)
                .expect("菜单项必须来自目录表");
            assert!(mac_bundle_path(spec).is_some(), "{} 找不到包", app.name);
        }
    }
}
