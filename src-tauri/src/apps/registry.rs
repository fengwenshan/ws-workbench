//! Windows 注册表线索
//!
//! 这是「明明装了却找不到」最常见的解法。安装程序几乎都会写注册表，
//! 而命令行程序未必进 PATH、也未必装在默认目录，光靠扫目录会漏。
//!
//! 两个来源：
//! - **Uninstall**：`DisplayName` + `InstallLocation` / `DisplayIcon`
//! - **App Paths**：系统给「运行对话框里敲名字就能启动」准备的表，直接给出 exe 路径
//!
//! 一次查询把全部条目取回来并缓存，避免每个应用都起一次 PowerShell。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

use crate::apps::models::AppId;

/// 卸载信息里的一条。
///
/// 字段一律用 `null_as_default`：PowerShell 会为注册表里不存在的值输出 `null`，
/// 而这种「缺一半字段」的记录非常常见（同一款软件可能有两条记录，
/// 一条只有 InstallLocation，另一条只有 DisplayIcon），
/// 不兼容 null 就会把它们整条丢掉，表现成「明明装了却找不到」。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UninstallEntry {
    #[serde(
        rename = "DisplayName",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    pub display_name: String,
    #[serde(
        rename = "DisplayIcon",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    pub display_icon: String,
    #[serde(
        rename = "InstallLocation",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    pub install_location: String,
}

/// App Paths 里的一条
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppPathEntry {
    #[serde(
        rename = "Name",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    pub name: String,
    #[serde(
        rename = "Path",
        default,
        deserialize_with = "crate::winps::null_as_default"
    )]
    pub path: String,
}

/// 注册表全量线索
#[derive(Debug, Clone, Default)]
pub struct RegistryIndex {
    pub uninstall: Vec<UninstallEntry>,
    pub app_paths: Vec<AppPathEntry>,
}

static CACHE: Mutex<Option<RegistryIndex>> = Mutex::new(None);

/// 清空缓存，下次查询时重新扫
pub fn clear_cache() {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = None;
    }
}

/// 扫描注册表（带缓存）
pub fn index() -> RegistryIndex {
    if let Ok(cache) = CACHE.lock() {
        if let Some(index) = cache.as_ref() {
            return index.clone();
        }
    }
    let scanned = scan();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(scanned.clone());
    }
    scanned
}

/// 直接扫描，不使用缓存
pub fn scan() -> RegistryIndex {
    #[cfg(target_os = "windows")]
    {
        match crate::winps::run_json(SCAN_SCRIPT) {
            Ok(value) => {
                #[derive(Deserialize, Default)]
                struct Payload {
                    #[serde(default)]
                    uninstall: serde_json::Value,
                    #[serde(rename = "appPaths", default)]
                    app_paths: serde_json::Value,
                }
                match serde_json::from_value::<Payload>(value) {
                    Ok(payload) => RegistryIndex {
                        uninstall: crate::winps::parse_list(payload.uninstall),
                        app_paths: crate::winps::parse_list(payload.app_paths),
                    },
                    Err(_) => RegistryIndex::default(),
                }
            }
            // 注册表读不到不是致命错误：后面的目录扫描与 PATH 查找仍然有效
            Err(_) => RegistryIndex::default(),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        RegistryIndex::default()
    }
}

#[cfg(target_os = "windows")]
const SCAN_SCRIPT: &str = r#"
$uninstallKeys = @(
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
$appPathKeys = @(
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\*',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\*',
  'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\*'
)
$u = Get-ItemProperty $uninstallKeys -ErrorAction SilentlyContinue |
  Where-Object { $_.DisplayName } |
  Select-Object DisplayName, DisplayIcon, InstallLocation
$a = Get-ChildItem $appPathKeys -ErrorAction SilentlyContinue | ForEach-Object {
  $value = (Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).'(default)'
  if ($value) { [pscustomobject]@{ Name = $_.PSChildName; Path = $value } }
}
[pscustomobject]@{ uninstall = @($u); appPaths = @($a) } | ConvertTo-Json -Depth 4 -Compress
"#;

/// 从 DisplayIcon 里抠出可执行文件路径。
/// 常见形式：`"C:\a b\app.exe",0`、`C:\app.exe,0`、`C:\app.exe`
pub fn parse_display_icon(raw: &str) -> Option<PathBuf> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    // 去掉结尾的 ,索引
    let without_index = match text.rfind(',') {
        Some(index) => {
            let tail = &text[index + 1..];
            if tail.trim().chars().all(|c| c.is_ascii_digit()) {
                &text[..index]
            } else {
                text
            }
        }
        None => text,
    };
    let cleaned = without_index.trim().trim_matches('"').trim();
    if cleaned.is_empty() {
        return None;
    }
    let path = PathBuf::from(cleaned);
    // 只关心可执行文件/脚本
    let is_executable = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_lowercase();
            ext == "exe" || ext == "cmd" || ext == "bat" || ext == "com"
        })
        .unwrap_or(false);
    if is_executable {
        Some(path)
    } else {
        None
    }
}

/// 目录名是否命中关键词（不区分大小写）
fn matches_keyword(text: &str, keywords: &[&str]) -> bool {
    let lower = text.to_lowercase();
    keywords.iter().any(|keyword| lower.contains(&keyword.to_lowercase()))
}

/// DisplayIcon 指向的文件名是否在我们的目标列表里。
///
/// 必须过滤：同一个软件家族里往往有好几个可执行文件，
/// 指向别的那个（比如图形界面主程序）拿过来会直接弹出一个窗口。
fn display_icon_matches(names: &[&str], path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            names
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(name))
        })
        .unwrap_or(false)
}

/// 在目录里按给定的可执行文件名找一层（含 bin 子目录）
fn exe_in_dir(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in names {
        let direct = dir.join(name);
        if direct.is_file() {
            out.push(direct);
        }
        let in_bin = dir.join("bin").join(name);
        if in_bin.is_file() {
            out.push(in_bin);
        }
    }
    out
}

/// 在目录下有限深度内查找给定的可执行文件名
fn find_below(dir: &Path, names: &[&str], max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((current, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }
        // 先看当前层
        out.extend(exe_in_dir(&current, names));
        if out.len() >= 4 {
            break;
        }
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            }
        }
    }
    out
}

/// 从注册表线索里推导出「文件名在该列表内」的候选路径。
///
/// `names` 同时用于匹配 App Paths 的键名和筛选 DisplayIcon 的文件名，
/// 所以既能查真正的可执行文件，也能按文件名挡掉同家族里的其它程序
/// （例如图形界面主程序，拿它当命令行工具会直接弹出一个窗口）。
pub fn candidates_for(
    id: AppId,
    index: &RegistryIndex,
    names: &[&str],
) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let keywords = id.registry_keywords();

    // 1. App Paths：直接给出 exe 路径，最准
    for entry in &index.app_paths {
        if !matches_keyword(&entry.name, names) {
            continue;
        }
        if let Some(path) = parse_display_icon(&entry.path) {
            if display_icon_matches(names, &path) {
                out.push((path, "registry".to_string()));
            }
        } else {
            let path = PathBuf::from(entry.path.trim().trim_matches('"'));
            if path.is_file() {
                out.push((path, "registry".to_string()));
            }
        }
    }

    // 2. Uninstall：DisplayIcon 与 InstallLocation 都可能指向安装目录
    for entry in &index.uninstall {
        if !matches_keyword(&entry.display_name, keywords) {
            continue;
        }

        let icon = parse_display_icon(&entry.display_icon);

        if let Some(path) = icon.as_ref() {
            // 只接受文件名与目标一致的，否则会把同一家族里的别的程序当成命令行工具
            if display_icon_matches(names, path) {
                out.push((path.clone(), "registry".to_string()));
            }
        }

        // 搜索目录有两个来源，缺一不可：
        // - InstallLocation：正规安装器都会写，但并非所有软件都写
        // - DisplayIcon 的父目录：很多安装器只写这个
        //   （手工解压出来的 openconnect 就只写了 DisplayIcon，装在非默认目录下）
        let mut search_dirs: Vec<PathBuf> = Vec::new();
        let install_dir = entry.install_location.trim().trim_matches('"');
        if !install_dir.is_empty() {
            search_dirs.push(PathBuf::from(install_dir));
        }
        if let Some(icon_path) = icon.as_ref() {
            if let Some(parent) = icon_path.parent() {
                search_dirs.push(parent.to_path_buf());
            }
        }

        for dir in search_dirs {
            if !dir.is_dir() {
                continue;
            }
            let mut found = exe_in_dir(&dir, names);
            if found.is_empty() {
                found = find_below(&dir, names, id.windows_search_depth());
            }
            for path in found {
                out.push((path, "registry".to_string()));
            }
        }
    }

    out
}

/// 查找该应用真正的可执行文件
pub fn candidates(id: AppId, index: &RegistryIndex) -> Vec<(PathBuf, String)> {
    candidates_for(id, index, id.executable_names())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_display_icon_with_index() {
        assert_eq!(
            parse_display_icon(r"C:\app\code.exe,0").unwrap(),
            PathBuf::from(r"C:\app\code.exe")
        );
    }

    #[test]
    fn parses_display_icon_with_quotes_and_spaces() {
        assert_eq!(
            parse_display_icon(r#""C:\Program Files\App\app.exe",0"#).unwrap(),
            PathBuf::from(r"C:\Program Files\App\app.exe")
        );
    }

    #[test]
    fn parses_plain_display_icon() {
        assert_eq!(
            parse_display_icon(r"C:\app\app.exe").unwrap(),
            PathBuf::from(r"C:\app\app.exe")
        );
    }

    #[test]
    fn accepts_command_wrappers() {
        assert_eq!(
            parse_display_icon(r"C:\app\code.cmd").unwrap(),
            PathBuf::from(r"C:\app\code.cmd")
        );
    }

    #[test]
    fn rejects_non_executables() {
        assert!(parse_display_icon("").is_none());
        assert!(parse_display_icon(r"C:\app\readme.txt").is_none());
        assert!(parse_display_icon(r"C:\app\icons").is_none());
    }

    #[test]
    fn keyword_match_is_case_insensitive() {
        assert!(matches_keyword("IntelliJ IDEA Ultimate", &["intellij idea"]));
        assert!(matches_keyword("openconnect 9.21", &["OpenConnect"]));
        assert!(!matches_keyword("7-Zip", &["OpenConnect"]));
    }

    #[test]
    fn app_paths_are_matched_by_exe_name() {
        let index = RegistryIndex {
            uninstall: vec![],
            app_paths: vec![AppPathEntry {
                name: "Code.exe".to_string(),
                path: r"C:\Users\demo\AppData\Local\Programs\Microsoft VS Code\Code.exe".to_string(),
            }],
        };
        let found = candidates(AppId::Vscode, &index);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "registry");
    }

    #[test]
    fn uninstall_install_location_is_searched_for_exe() {
        // 这里用一个真实存在的临时目录，验证 InstallLocation 分支能落地
        let dir = std::env::temp_dir().join(format!("ws-apps-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let exe = dir.join("openconnect.exe");
        std::fs::write(&exe, b"stub").unwrap();

        let index = RegistryIndex {
            uninstall: vec![UninstallEntry {
                display_name: "OpenConnect 9.21".to_string(),
                display_icon: String::new(),
                install_location: dir.to_string_lossy().to_string(),
            }],
            app_paths: vec![],
        };
        let found = candidates(AppId::Openconnect, &index);
        assert!(found.iter().any(|(path, _)| path == &exe), "应该能找到 {:?}", exe);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unrelated_uninstall_entries_are_ignored() {
        let index = RegistryIndex {
            uninstall: vec![UninstallEntry {
                display_name: "7-Zip 24.09".to_string(),
                display_icon: r"C:\Program Files\7-Zip\7zFM.exe,0".to_string(),
                install_location: String::new(),
            }],
            app_paths: vec![],
        };
        assert!(candidates(AppId::Vscode, &index).is_empty());
    }

    /// 关键回归点：同一家族里的图形界面主程序不能当命令行工具用
    #[test]
    fn display_icon_of_gui_binary_is_rejected() {
        let cli = AppId::Openconnect.executable_names();
        assert!(!display_icon_matches(
            cli,
            Path::new(r"C:\app\OpenConnect-GUI\openconnect-gui.exe")
        ));
        assert!(display_icon_matches(
            cli,
            Path::new(r"C:\app\openconnect\openconnect.exe")
        ));
    }

    #[test]
    fn gui_display_icon_is_not_offered_as_connector() {
        let index = RegistryIndex {
            uninstall: vec![UninstallEntry {
                display_name: "OpenConnect-GUI 1.6.2".to_string(),
                display_icon: r"C:\app\OpenConnect-GUI\openconnect-gui.exe".to_string(),
                install_location: String::new(),
            }],
            app_paths: vec![],
        };
        let found = candidates(AppId::Openconnect, &index);
        assert!(
            !found
                .iter()
                .any(|(path, _)| path.to_string_lossy().contains("openconnect-gui")),
            "GUI 主程序不该被当成连接器候选: {:?}",
            found
        );
    }

    #[test]
    fn display_icon_accepts_the_real_entry_point() {
        assert!(display_icon_matches(
            AppId::Vscode.executable_names(),
            Path::new(r"C:\Program Files\Microsoft VS Code\Code.exe")
        ));
        // 装了但没写 InstallLocation 时，靠 DisplayIcon 指到哪就找哪
        assert!(display_icon_matches(
            AppId::Openconnect.executable_names(),
            Path::new(r"C:\Program Files\openconnect\openconnect.exe")
        ));
        // 指向自带的安装助手，不是我们要的入口
        assert!(!display_icon_matches(
            AppId::Openconnect.executable_names(),
            Path::new(r"C:\Program Files\openconnect\InstallHelper.exe")
        ));
    }

    /// 只写 DisplayIcon、不写 InstallLocation，且装在非默认目录下。
    /// 必须能从 DisplayIcon 的父目录反推出安装位置。
    #[test]
    fn finds_directory_from_display_icon_parent() {
        let root = std::env::temp_dir().join(format!("ws-ocdir-{}", std::process::id()));
        let dir = root.join("openconnect-9.21");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("openconnect.exe"), b"stub").unwrap();

        let index = RegistryIndex {
            uninstall: vec![UninstallEntry {
                display_name: "OpenConnect 9.21".to_string(),
                display_icon: dir
                    .join("openconnect.exe")
                    .to_string_lossy()
                    .to_string(),
                // 关键：没有 InstallLocation
                install_location: String::new(),
            }],
            app_paths: vec![],
        };

        let found = candidates(AppId::Openconnect, &index);
        assert!(
            found
                .iter()
                .any(|(path, _)| path.ends_with("openconnect.exe")),
            "应该能从 DisplayIcon 反推出安装目录，实际: {:?}",
            found
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// DisplayIcon 父目录里如果真有命令行程序，也应该被找到
    #[test]
    fn finds_cli_beside_display_icon() {
        let root = std::env::temp_dir().join(format!("ws-ocbeside-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("openconnect-gui.exe"), b"stub").unwrap();
        std::fs::write(root.join("openconnect.exe"), b"stub").unwrap();

        let index = RegistryIndex {
            uninstall: vec![UninstallEntry {
                display_name: "OpenConnect 9.21".to_string(),
                display_icon: root.join("openconnect-gui.exe").to_string_lossy().to_string(),
                install_location: String::new(),
            }],
            app_paths: vec![],
        };

        let found = candidates(AppId::Openconnect, &index);
        assert!(
            found.iter().any(|(path, _)| path.ends_with("openconnect.exe")),
            "DisplayIcon 旁边的命令行程序应该被发现，实际: {:?}",
            found
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn scan_returns_uninstall_entries_from_this_machine() {
        // 真机验证：注册表扫描至少应该能读回一堆卸载项，
        // 否则说明 PowerShell 那条通道没跑通
        let index = scan();
        assert!(
            !index.uninstall.is_empty(),
            "注册表扫描应该能返回卸载项"
        );
    }
}
