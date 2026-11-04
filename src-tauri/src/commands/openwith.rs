//! 「用其它应用打开目录」的应用目录与启动。
//!
//! 目录表参照 DeepSeek Harness 的 open-in-app catalog：每个应用声明自己
//! 在各平台可能的安装位置，运行时按顺序取第一个存在的，一个都找不到就
//! 不进菜单（所以菜单里永远只出现真正装了的东西）。
//!
//! 图标只在 macOS 上提供：`sips` 把包里的 `.icns` 转成 64px PNG，编码成
//! data URL 交给前端，前端直接 `<img>` 显示；其它平台先不给图标，前端用
//! 首字母兜底。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

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
#[tauri::command]
pub fn list_open_with_apps(app: AppHandle) -> Vec<OpenWithApp> {
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

/// 重新扫描（用户刚装完软件时用）
#[tauri::command]
pub fn refresh_open_with_apps(app: AppHandle) -> Vec<OpenWithApp> {
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
        let _ = app;
        mac_open(app_id, dir)
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
fn mac_icon(bundle: &Path) -> Option<String> {
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
fn mac_open(app_id: &str, dir: &str) -> Result<(), String> {
    let spec = MAC_CATALOG
        .iter()
        .find(|spec| spec.id == app_id)
        .ok_or_else(|| format!("未知的打开方式: {}", app_id))?;
    let bundle = mac_bundle_path(spec).ok_or_else(|| format!("{} 没有安装", spec.name))?;

    // 访达就用 shell open，等价于双击目录
    if spec.id == "finder" {
        return crate::commands::open_path(dir);
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

/// 非 macOS 上能用的打开方式少一些：文件管理器、终端，外加 `apps` 模块能定位到的编辑器
#[cfg(not(target_os = "macos"))]
fn other_scan(app: &AppHandle) -> Vec<OpenWithApp> {
    #[cfg(target_os = "windows")]
    let shell = ("explorer", "资源管理器");
    #[cfg(all(unix, not(target_os = "macos")))]
    let shell = ("filemanager", "文件管理器");

    let mut apps = vec![OpenWithApp {
        id: shell.0.to_string(),
        name: shell.1.to_string(),
        icon: None,
        is_default: true,
    }];

    // 编辑器：复用 `apps::locate` 的六条探测路径，找不到就不出现
    let ctx = crate::commands::app_context(app);
    let editors = [
        ("vscode", "VS Code", crate::apps::AppId::Vscode),
        ("idea", "IntelliJ IDEA", crate::apps::AppId::Idea),
        ("trae", "Trae", crate::apps::AppId::Trae),
    ];
    for (id, name, app_id) in editors {
        if crate::apps::locate(app_id, &ctx, "").available {
            apps.push(OpenWithApp {
                id: id.to_string(),
                name: name.to_string(),
                icon: None,
                is_default: false,
            });
        }
    }

    apps.push(OpenWithApp {
        id: "terminal".to_string(),
        name: "终端".to_string(),
        icon: None,
        is_default: false,
    });
    apps
}

#[cfg(not(target_os = "macos"))]
fn other_open(app: &AppHandle, app_id: &str, dir: &str) -> Result<(), String> {
    if app_id == "explorer" || app_id == "filemanager" {
        return crate::commands::open_path(dir);
    }
    if app_id == "terminal" {
        return crate::commands::open_terminal_dir(dir);
    }
    let app_id = match app_id {
        "vscode" => crate::apps::AppId::Vscode,
        "idea" => crate::apps::AppId::Idea,
        "trae" => crate::apps::AppId::Trae,
        other => return Err(format!("未知的打开方式: {}", other)),
    };
    let ctx = crate::commands::app_context(app);
    let location = crate::apps::locate(app_id, &ctx, "");
    if !location.available {
        return Err(format!("未找到 {}。{}", app_id.display_name(), location.note));
    }
    crate::apps::finder::spawn_with_target(Path::new(&location.path), dir)
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
