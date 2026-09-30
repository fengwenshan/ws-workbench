//! 应用定位：按优先级把可执行文件找出来
//!
//! 查找顺序（先命中先用）：
//! 1. `custom`   —— 用户在设置页手动指定的路径
//! 2. `managed`  —— 程序自己托管的副本（应用数据目录 `apps/<id>/`）
//! 3. `bundled`  —— 构建期内置在资源目录里的副本
//! 4. `registry` —— Windows 注册表的卸载信息与 App Paths
//! 5. `system`   —— 已知安装目录（Program Files / LOCALAPPDATA / /Applications 等）
//! 6. `path`     —— PATH 环境变量，Windows 上按 PATHEXT 补全后缀

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::apps::models::{AppCandidate, AppContext, AppId, AppLocation};
use crate::apps::registry;
use crate::platform::Platform;

/// 内置/托管副本在应用数据目录与资源目录下的子目录名
pub const APPS_DIR: &str = "apps";

/// 程序托管的副本所在目录
pub fn managed_dir(id: AppId, data_dir: &Path) -> PathBuf {
    data_dir.join(APPS_DIR).join(id.as_str())
}

/// 是否为需要交给 cmd.exe 解释的批处理包装脚本
pub fn is_script(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_lowercase();
            ext == "cmd" || ext == "bat"
        })
        .unwrap_or(false)
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
}

/// PATH 里的目录
fn path_dirs() -> Vec<PathBuf> {
    match std::env::var_os("PATH") {
        Some(value) => std::env::split_paths(&value)
            .filter(|dir| !dir.as_os_str().is_empty())
            .collect(),
        None => Vec::new(),
    }
}

/// Windows 的 PATHEXT：cmd.exe 用来补全可执行后缀
#[cfg(target_os = "windows")]
fn pathext() -> Vec<String> {
    const FALLBACK: &str = ".COM;.EXE;.BAT;.CMD";
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| FALLBACK.to_string());
    let list: Vec<String> = raw
        .split(';')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if list.is_empty() {
        FALLBACK.split(';').map(|item| item.to_string()).collect()
    } else {
        list
    }
}

/// 按 cmd.exe 的规则在 PATH 里找：
/// 名字自带后缀就直接找，不带后缀时依次尝试 PATHEXT 里的每个后缀
fn search_path(names: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let dirs = path_dirs();

    #[cfg(target_os = "windows")]
    let exts = pathext();
    #[cfg(not(target_os = "windows"))]
    let exts: Vec<String> = Vec::new();

    for dir in &dirs {
        for name in names.iter().copied() {
            let direct = dir.join(name);
            if direct.is_file() {
                found.push(direct);
                continue;
            }
            // 名字没写后缀：按 PATHEXT 逐个补全（"code" -> "code.cmd"）
            if Path::new(name).extension().is_none() {
                for ext in &exts {
                    let with_ext = dir.join(format!("{}{}", name, ext));
                    if with_ext.is_file() {
                        found.push(with_ext);
                        break;
                    }
                }
            }
        }
    }
    found
}

/// 安装根目录：Windows 看 Program Files 系列与 LOCALAPPDATA，macOS/Linux 看常见前缀
fn install_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "windows")]
    {
        for name in [
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "LOCALAPPDATA",
            "PROGRAMDATA",
        ] {
            if let Some(dir) = env_dir(name) {
                roots.push(dir);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for dir in ["/Applications", "/opt/homebrew/bin", "/opt/homebrew/opt", "/usr/local/bin"] {
            let path = PathBuf::from(dir);
            if path.is_dir() {
                roots.push(path);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        for dir in [
            "/usr/bin",
            "/usr/local/bin",
            "/opt",
            "/snap/bin",
            "/var/lib/flatpak/exports/bin",
        ] {
            let path = PathBuf::from(dir);
            if path.is_dir() {
                roots.push(path);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".local/bin"));
        }
    }

    roots
}

/// 名字含关键词的子目录，按名字倒序（让新版本排在前面）
fn sub_dirs_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let needle = needle.to_lowercase();
    let mut dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let matched = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_lowercase().contains(&needle))
                .unwrap_or(false);
            if matched && path.is_dir() {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    dirs.reverse();
    dirs
}

/// 在 dir 下有限深度内查找指定文件名
fn find_file_below(dir: &Path, file_name: &str, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((current, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push((path, depth + 1));
                } else if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.eq_ignore_ascii_case(file_name))
                    .unwrap_or(false)
                {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    found.reverse();
    found
}

/// 已知安装目录里的候选（含 bin 子目录）
fn known_install_candidates(id: AppId, names: &[&str]) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let roots = install_roots();

    // macOS / Linux 的固定路径
    if !Platform::is_windows(&Platform) {
        for raw in id.macos_paths() {
            let path = PathBuf::from(raw);
            if path.is_file() {
                out.push((path, "system".to_string()));
            }
        }
    }

    for root in roots {
        for relative in id.windows_install_dirs() {
            let dir = root.join(relative);
            if !dir.is_dir() {
                continue;
            }
            for name in names {
                for candidate in [dir.join(name), dir.join("bin").join(name)] {
                    if candidate.is_file() {
                        out.push((candidate, "system".to_string()));
                    }
                }
            }
        }

        // 带版本号的目录（JetBrains 那类），往下找几层
        for keyword in id.windows_versioned_dirs() {
            for dir in sub_dirs_containing(&root, keyword) {
                for name in names {
                    for found in find_file_below(&dir, name, id.windows_search_depth()) {
                        out.push((found, "system".to_string()));
                    }
                }
            }
        }
    }

    out
}

/// 按给定的文件名列表，把六条来源的候选汇总起来（已去重，保持优先级顺序）
fn collect(id: AppId, ctx: &AppContext, names: &[&str]) -> Vec<(PathBuf, String)> {
    let mut collected: Vec<(PathBuf, String)> = Vec::new();

    collected.extend(managed_candidates(id, ctx, names));
    collected.extend(bundled_candidates(id, ctx, names));

    // 注册表只在 Windows 上有意义
    if Platform::is_windows(&Platform) {
        collected.extend(registry::candidates_for(id, &registry::index(), names));
    }

    collected.extend(known_install_candidates(id, names));

    for found in search_path(names) {
        collected.push((found, "path".to_string()));
    }

    dedup(collected)
}

/// 托管副本的候选位置
fn managed_candidates(id: AppId, ctx: &AppContext, names: &[&str]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    if let Some(data_dir) = ctx.data_dir.as_ref() {
        for name in names {
            out.push((managed_dir(id, data_dir).join(name), "managed".to_string()));
        }
    }
    out
}

/// 构建期内置的候选位置
fn bundled_candidates(id: AppId, ctx: &AppContext, names: &[&str]) -> Vec<(PathBuf, String)> {
    let mut bases: Vec<PathBuf> = Vec::new();
    for base in [ctx.resource_dir.clone(), ctx.exe_dir.clone()]
        .into_iter()
        .flatten()
    {
        bases.push(base.join(APPS_DIR).join(id.as_str()));
    }
    // 开发模式下资源还没被拷进 target 目录，指回源码树
    bases.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(APPS_DIR)
            .join(id.as_str()),
    );

    let mut out = Vec::new();
    for base in bases {
        for name in names {
            out.push((base.join(name), "bundled".to_string()));
        }
    }
    out
}

/// 去重（Windows 路径不区分大小写），保持顺序
fn dedup(items: Vec<(PathBuf, String)>) -> Vec<(PathBuf, String)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (path, source) in items {
        let key = if Platform::is_windows(&Platform) {
            path.to_string_lossy().to_lowercase()
        } else {
            path.to_string_lossy().to_string()
        };
        if seen.insert(key) {
            out.push((path, source));
        }
    }
    out
}

/// 定位一个应用。
///
/// `extra_override` 是调用方自带的专用路径（例如某个 VPN 配置里单独指定的连接器），
/// 它优先于设置页里的全局设置。
pub fn locate(id: AppId, ctx: &AppContext, extra_override: &str) -> AppLocation {
    // 调用方专用路径 > 设置页全局路径
    let extra = extra_override.trim();
    let override_text = if extra.is_empty() {
        ctx.override_for(id).trim().to_string()
    } else {
        extra.to_string()
    };

    let mut collected: Vec<(PathBuf, String)> = Vec::new();
    // 用户手动指定。即使不存在也保留，好让界面明确告诉用户「你填的路径不对」
    if !override_text.is_empty() {
        collected.push((PathBuf::from(&override_text), "custom".to_string()));
    }

    let discovered = collect(id, ctx, id.executable_names());

    // 内置副本单独拎出来：它的位置在编译期就确定了，不是「找出来的结果」。
    // 混进候选列表会让界面看起来像还在搜自己的安装目录。
    let builtin = discovered
        .iter()
        .find(|(path, source)| source == "bundled" && path.is_file())
        .map(|(path, _)| path.clone());
    collected.extend(
        discovered
            .into_iter()
            .filter(|(_, source)| source != "bundled"),
    );

    let deduped = dedup(collected);
    let candidates: Vec<AppCandidate> = deduped
        .iter()
        .map(|(path, source)| AppCandidate {
            path: path.to_string_lossy().to_string(),
            source: source.clone(),
            script: is_script(path),
            exists: path.is_file(),
        })
        .collect();

    // 用户自己的那份优先；一份都没有才退回内置副本
    let own = deduped.iter().find(|(path, _)| path.is_file());
    let chosen: Option<(PathBuf, String)> = match own {
        Some((path, source)) => Some((path.clone(), source.clone())),
        None => builtin.clone().map(|path| (path, "bundled".to_string())),
    };

    let builtin_text = builtin
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    match chosen {
        Some((path, source)) => AppLocation {
            id: id.as_str().to_string(),
            name: id.display_name().to_string(),
            description: id.description().to_string(),
            path: path.to_string_lossy().to_string(),
            available: true,
            source: source.clone(),
            override_path: override_text,
            note: if source == "custom" {
                "使用设置页手动指定的路径".to_string()
            } else {
                String::new()
            },
            builtin_path: builtin_text,
            supports_import: id.supports_import(),
            supports_override: Platform::is_windows(&Platform),
            expected_names: id
                .executable_names()
                .iter()
                .map(|name| name.to_string())
                .collect(),
            candidates,
        },
        None => {
            let mut missing = AppLocation::missing(id, missing_note(&override_text));
            missing.override_path = override_text;
            missing.builtin_path = builtin_text;
            missing.candidates = candidates;
            missing
        }
    }
}

/// 找不到时给出的排查说明。
/// 单独抽出来是为了能确定性地测到「手动指定路径不存在」这条分支 ——
/// 直接在 `locate` 里测会受这台机器装了哪些软件影响。
fn missing_note(override_path: &str) -> String {
    if !override_path.is_empty() && !Path::new(override_path).is_file() {
        format!("设置里指定的路径不存在：{}", override_path)
    } else {
        "已查找：手动指定、程序托管目录、内置资源、注册表、常见安装目录、PATH".to_string()
    }
}

/// 一次性定位所有应用（只按设置页里的全局配置，不带调用方专用路径）
pub fn locate_all(ctx: &AppContext) -> Vec<AppLocation> {
    AppId::ALL
        .iter()
        .map(|id| locate(*id, ctx, ""))
        .collect()
}

/// 把一个已有的应用目录复制到程序托管目录。
///
/// Windows 下连同同目录的 DLL 一起复制 —— openconnect 是一堆 exe + lib*.dll，
/// 少一个都跑不起来。只复制可执行文件与 DLL，不碰安装目录里的其它东西。
pub fn import_from_dir(id: AppId, source_dir: &Path, data_dir: &Path) -> Result<String, String> {
    if !source_dir.is_dir() {
        return Err(format!("源目录不存在: {}", source_dir.display()));
    }

    let primary = id
        .executable_names()
        .iter()
        .map(|name| source_dir.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "源目录里没有找到 {}，请选可执行文件所在的目录",
                id.executable_names().join(" / ")
            )
        })?;

    let target_dir = managed_dir(id, data_dir);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("创建目标目录失败 {}: {}", target_dir.display(), e))?;

    let mut copied = 0usize;
    let entries = std::fs::read_dir(source_dir)
        .map_err(|e| format!("读取源目录失败: {}", e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let lower = name.to_lowercase();
        // 复制可执行文件本身、DLL、以及 vpnc-script（openconnect 用得到）
        let keep = id
            .executable_names()
            .iter()
            .any(|candidate| candidate.to_lowercase() == lower)
            || lower.ends_with(".dll")
            || lower.ends_with(".exe")
            || lower.starts_with("vpnc-script");
        if !keep {
            continue;
        }
        std::fs::copy(&path, target_dir.join(name))
            .map_err(|e| format!("复制 {} 失败: {}", name, e))?;
        copied += 1;
    }

    if copied == 0 {
        return Err("没有复制到任何文件".to_string());
    }

    Ok(primary.to_string_lossy().to_string())
}

/// 删除程序托管的副本
pub fn remove_managed(id: AppId, data_dir: &Path) -> Result<(), String> {
    let target = managed_dir(id, data_dir);
    if !target.exists() {
        return Ok(());
    }
    // 只删 data_dir 的直接子目录，防止路径被拼歪时误删
    let expected_parent = data_dir.join(APPS_DIR);
    if target.parent() != Some(expected_parent.as_path()) {
        return Err("目标目录不在应用数据目录下，已拒绝删除".to_string());
    }
    std::fs::remove_dir_all(&target).map_err(|e| format!("删除失败: {}", e))
}

/// 用指定的目标路径启动应用
pub fn spawn_with_target(executable: &Path, target: &str) -> Result<(), String> {
    if !executable.is_file() {
        return Err(format!("启动程序不存在: {}", executable.display()));
    }

    #[cfg(target_os = "windows")]
    {
        if is_script(executable) {
            // .cmd/.bat 必须交给 cmd.exe 解释；CreateProcess 不会做 PATHEXT 扩展
            let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
            return crate::process::spawn_detached(
                Path::new(&shell),
                &["/C", &executable.to_string_lossy(), target],
            )
            .map_err(|e| format!("启动失败: {}", e));
        }
    }

    crate::process::spawn_detached(executable, &[target])
        .map_err(|e| format!("启动失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ws-apps-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_script_wrappers() {
        assert!(is_script(Path::new(r"C:\x\code.cmd")));
        assert!(is_script(Path::new(r"C:\x\code.BAT")));
        assert!(!is_script(Path::new(r"C:\x\Code.exe")));
        assert!(!is_script(Path::new("/usr/bin/code")));
    }

    #[test]
    fn managed_dir_is_under_apps_folder() {
        let data = PathBuf::from("/data");
        assert_eq!(
            managed_dir(AppId::Openconnect, &data),
            PathBuf::from("/data/apps/openconnect")
        );
    }

    #[test]
    fn missing_override_is_reported_not_swallowed() {
        let ctx = AppContext::default();
        let location = locate(AppId::Openconnect, &ctx, r"Z:\nope\openconnect.exe");
        // 手动指定的路径必须原样保留在结果里，界面才能显示「当前填的是这个」
        assert_eq!(location.override_path, r"Z:\nope\openconnect.exe");
        let custom = location
            .candidates
            .iter()
            .find(|item| item.source == "custom")
            .expect("候选里应该保留手动指定的记录");
        assert!(!custom.exists, "Z: 盘这个路径不该存在");
    }

    #[test]
    fn missing_note_points_at_the_bad_override() {
        // 抽出来单独测：不受这台机器装了哪些软件影响
        let note = missing_note(r"Z:\nope\openconnect.exe");
        assert!(note.contains("不存在"), "实际: {}", note);
        assert!(note.contains(r"Z:\nope\openconnect.exe"), "实际: {}", note);

        let fallback = missing_note("");
        assert!(fallback.contains("已查找"), "实际: {}", fallback);
    }

    #[test]
    fn missing_note_ignores_override_that_exists() {
        let dir = temp_dir("note-exists");
        let exe = dir.join(if cfg!(target_os = "windows") {
            "openconnect.exe"
        } else {
            "openconnect"
        });
        std::fs::write(&exe, b"stub").unwrap();
        // 路径存在时不该说它不存在
        assert!(missing_note(&exe.to_string_lossy()).contains("已查找"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// openconnect 那条绝不能把同家族里的图形界面主程序当成连接器返回。
    ///
    /// 机器上可能残留着老版本 OpenConnect-GUI 的注册表记录，
    /// 它的 DisplayName 同样含「OpenConnect」，光看关键词会串。
    #[test]
    fn cli_entry_never_returns_the_gui_binary() {
        let cli = locate(AppId::Openconnect, &AppContext::default(), "");
        assert!(
            !cli.path.to_lowercase().contains("openconnect-gui"),
            "命令行条目的路径不能是 GUI 主程序，实际: {}",
            cli.path
        );
        assert!(
            !cli
                .candidates
                .iter()
                .any(|candidate| candidate.path.to_lowercase().contains("openconnect-gui")),
            "GUI 主程序不该出现在命令行条目的候选里: {:?}",
            cli.candidates
        );
    }

    /// 内置副本必须走独立字段，不能混进「找出来的候选」。
    /// 用户的原话是「应用程序里面的不用找啊」—— 位置编译期就定了，列成候选很荒唐。
    #[test]
    fn builtin_copy_is_not_listed_as_a_search_candidate() {
        let data = temp_dir("builtin-data");
        let ctx = AppContext {
            data_dir: Some(data.clone()),
            ..Default::default()
        };
        let location = locate(AppId::Openconnect, &ctx, "");

        if location.source == "bundled" || !location.builtin_path.is_empty() {
            // 只有仓库里确实放了内置副本时才验证
            assert!(
                !location.builtin_path.is_empty(),
                "内置副本存在时必须填 builtin_path"
            );
            assert!(
                !location
                    .candidates
                    .iter()
                    .any(|candidate| candidate.source == "bundled"),
                "内置副本不该出现在候选列表里: {:?}",
                location.candidates
            );
        }

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn managed_copy_is_preferred_over_nothing() {
        let data = temp_dir("managed");
        let dir = managed_dir(AppId::Openconnect, &data);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(if cfg!(target_os = "windows") {
            "openconnect.exe"
        } else {
            "openconnect"
        });
        std::fs::write(&exe, b"stub").unwrap();

        let ctx = AppContext {
            data_dir: Some(data.clone()),
            ..Default::default()
        };
        let location = locate(AppId::Openconnect, &ctx, "");
        assert!(location.available);
        assert_eq!(location.source, "managed");
        assert_eq!(location.path, exe.to_string_lossy());

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn custom_override_wins_over_managed() {
        let data = temp_dir("override");
        let dir = managed_dir(AppId::Openconnect, &data);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("openconnect.exe"), b"stub").unwrap();

        let custom = temp_dir("custom");
        let custom_exe = custom.join("openconnect.exe");
        std::fs::write(&custom_exe, b"stub").unwrap();

        let ctx = AppContext {
            data_dir: Some(data.clone()),
            ..Default::default()
        };
        let location = locate(AppId::Openconnect, &ctx, &custom_exe.to_string_lossy());
        assert!(location.available);
        assert_eq!(location.source, "custom");
        assert_eq!(location.path, custom_exe.to_string_lossy());

        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&custom);
    }

    #[test]
    fn locate_all_returns_every_app() {
        let ctx = AppContext::default();
        let all = locate_all(&ctx);
        assert_eq!(all.len(), AppId::ALL.len());
        for location in &all {
            assert!(!location.name.is_empty());
        }
    }

    #[test]
    fn locate_all_applies_settings_overrides() {
        let custom = temp_dir("all-override");
        let exe = custom.join(if cfg!(target_os = "windows") {
            "openconnect.exe"
        } else {
            "openconnect"
        });
        std::fs::write(&exe, b"stub").unwrap();

        let ctx = AppContext {
            overrides: vec![crate::apps::models::AppPathOverride::new(
                "openconnect",
                &exe.to_string_lossy(),
            )],
            ..Default::default()
        };
        let all = locate_all(&ctx);
        let openconnect = all
            .iter()
            .find(|item| item.id == "openconnect")
            .expect("应该有 openconnect");
        assert_eq!(openconnect.source, "custom");
        assert_eq!(openconnect.path, exe.to_string_lossy());

        let _ = std::fs::remove_dir_all(&custom);
    }

    #[test]
    fn per_call_override_beats_settings_override() {
        let settings = temp_dir("settings-override");
        let settings_exe = settings.join(if cfg!(target_os = "windows") {
            "openconnect.exe"
        } else {
            "openconnect"
        });
        std::fs::write(&settings_exe, b"stub").unwrap();

        let per_call = temp_dir("percall-override");
        let per_call_exe = per_call.join(if cfg!(target_os = "windows") {
            "openconnect.exe"
        } else {
            "openconnect"
        });
        std::fs::write(&per_call_exe, b"stub").unwrap();

        let ctx = AppContext {
            overrides: vec![crate::apps::models::AppPathOverride::new(
                "openconnect",
                &settings_exe.to_string_lossy(),
            )],
            ..Default::default()
        };
        // VPN 配置里自带的路径应该压过设置页里的
        let location = locate(AppId::Openconnect, &ctx, &per_call_exe.to_string_lossy());
        assert_eq!(location.path, per_call_exe.to_string_lossy());

        // 不带专用路径时用设置页的
        let fallback = locate(AppId::Openconnect, &ctx, "");
        assert_eq!(fallback.path, settings_exe.to_string_lossy());

        let _ = std::fs::remove_dir_all(&settings);
        let _ = std::fs::remove_dir_all(&per_call);
    }

    #[test]
    fn bundled_candidates_live_under_apps_dir() {
        let res = PathBuf::from("/res");
        let ctx = AppContext {
            resource_dir: Some(res.clone()),
            ..Default::default()
        };
        let found = bundled_candidates(
            AppId::Openconnect,
            &ctx,
            AppId::Openconnect.executable_names(),
        );
        assert!(!found.is_empty());
        assert!(found.iter().all(|(_, source)| source == "bundled"));
        assert!(
            found
                .iter()
                .any(|(path, _)| path.starts_with(res.join(APPS_DIR).join("openconnect"))),
            "内置候选必须落在 <资源目录>/{}/openconnect 下，实际: {:?}",
            APPS_DIR,
            found
        );
    }

    /// 打包配置里的资源映射必须和代码里查找的目录一致。
    ///
    /// 这两处一旦不一致，本地开发完全测不出来（开发时走的是源码树路径），
    /// 只在打包后才暴露成「资源明明在包里，程序却报找不到」。
    #[test]
    fn tauri_bundle_resources_match_search_dir() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(manifest.join("tauri.conf.json"))
            .expect("读取 tauri.conf.json 失败");
        let config: serde_json::Value =
            serde_json::from_str(&text).expect("解析 tauri.conf.json 失败");

        let resources = config
            .get("bundle")
            .and_then(|bundle| bundle.get("resources"))
            .expect("tauri.conf.json 缺少 bundle.resources");

        let expected_source = format!("resources/{}", APPS_DIR);
        let mapped_correctly = match resources {
            serde_json::Value::Object(map) => map.iter().any(|(source, target)| {
                source == &expected_source && target.as_str() == Some(APPS_DIR)
            }),
            serde_json::Value::Array(list) => list
                .iter()
                .any(|item| item.as_str() == Some(expected_source.as_str())),
            _ => false,
        };

        assert!(
            mapped_correctly,
            "bundle.resources 必须把 {} 映射到 {}，否则打包后找不到内置应用。当前配置: {}",
            expected_source, APPS_DIR, resources
        );
    }

    /// 内置目录确实存在且放好了可执行文件时，locate 要能报出 bundled 来源
    #[test]
    fn bundled_copy_is_detected_when_present() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let bundled_dir = manifest
            .join("resources")
            .join(APPS_DIR)
            .join("openconnect");
        let binary = bundled_dir.join(if cfg!(target_os = "windows") {
            "openconnect.exe"
        } else {
            "openconnect"
        });
        if !binary.is_file() {
            // 仓库里没放内置副本时跳过（不强制要求内置）
            return;
        }

        let ctx = AppContext::default();
        let location = locate(AppId::Openconnect, &ctx, "");
        assert!(
            location.available,
            "内置目录里有 openconnect，locate 应该能发现它"
        );
        assert_eq!(
            location.source, "bundled",
            "没有任何手动指定时，应该选中内置副本，实际: {}",
            location.path
        );
    }

    #[test]
    fn import_copies_executable_and_dlls() {
        let source = temp_dir("import-src");
        let data = temp_dir("import-data");
        let exe_name = if cfg!(target_os = "windows") {
            "openconnect.exe"
        } else {
            "openconnect"
        };
        std::fs::write(source.join(exe_name), b"stub").unwrap();
        std::fs::write(source.join("libopenconnect-5.dll"), b"stub").unwrap();
        std::fs::write(source.join("readme.txt"), b"should not be copied").unwrap();

        let primary = import_from_dir(AppId::Openconnect, &source, &data).unwrap();
        assert!(primary.ends_with(exe_name));

        let target = managed_dir(AppId::Openconnect, &data);
        assert!(target.join(exe_name).is_file());
        assert!(target.join("libopenconnect-5.dll").is_file());
        assert!(!target.join("readme.txt").exists());

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn import_rejects_directory_without_executable() {
        let source = temp_dir("import-empty");
        let data = temp_dir("import-data2");
        let error = import_from_dir(AppId::Openconnect, &source, &data).unwrap_err();
        assert!(error.contains("没有找到"), "实际错误: {}", error);
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn remove_managed_only_touches_own_dir() {
        let data = temp_dir("remove");
        let dir = managed_dir(AppId::Vscode, &data);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Code.exe"), b"stub").unwrap();

        remove_managed(AppId::Vscode, &data).unwrap();
        assert!(!dir.exists());
        // 父目录本身要保留
        assert!(data.join(APPS_DIR).exists());

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn locate_is_available_when_path_itself_matches() {
        // PATH 里一定有可执行文件；用它验证 search_path 分支没写坏
        #[cfg(target_os = "windows")]
        let probe = "cmd.exe";
        #[cfg(not(target_os = "windows"))]
        let probe = "sh";
        let found = search_path(&[probe]);
        assert!(!found.is_empty(), "PATH 里应该能找到 {}", probe);
    }
}
