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

#[cfg(target_os = "windows")]
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

/// 候选来源的优先级：数字小的先算数，同来源内部保持原有顺序（稳定排序）。
///
/// `managed` 排在 `system` 后面是有意的。「指定位置」登记的那份是给**自定义前缀兜底**的；
/// 一旦标准安装位置里出现了能跑的 openconnect（用户后来把 brew 装到 /opt/homebrew，
/// 或者干脆从自定义前缀换成了标准安装），就该改用系统那份、并把它当系统默认 ——
/// 不能让旧登记一直顶在前面。现场就遇到过：先手动指定 `~/hb`，后来装到标准前缀，
/// 点「检测」仍然显示「已指定 …/.hb/bin/openconnect」。
fn source_priority(source: &str) -> u8 {
    match source {
        "custom" => 0,
        // 注册表条目和固定安装位置是同一档「系统里装好的」；同档按收集顺序排（稳定排序）
        "system" | "registry" => 1,
        "managed" => 2,
        "bundled" => 3,
        // path 以及将来的其它来源
        _ => 4,
    }
}

/// 按给定的文件名列表，把六条来源的候选汇总起来（已去重，按来源优先级排序）
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

    // 上面是「按来源分批收集」，这里统一按来源优先级排一次：分批的顺序不等于最终优先级，
    // 尤其「已登记」必须排在标准安装位置之后，否则系统装好之后旧登记还会被优先探测到。
    collected.sort_by_key(|(_, source)| source_priority(source));

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
    // 开发模式下资源还没被拷进 target 目录，指回源码树；release 不能带这条，
    // 否则安装版可能优先拿构建机上的绝对路径，反而掩盖安装包漏资源的问题。
    #[cfg(debug_assertions)]
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

/// 只取构建期内置的副本，不触发注册表 / PATH / 安装目录扫描。
///
/// Windows 的 VPN 连接器现在固定用安装包自带的那份，不再去找用户机器上装的
/// openconnect；这个函数就是那条「只看内置」的入口。
pub fn bundled_binary(id: AppId, ctx: &AppContext) -> Option<PathBuf> {
    bundled_candidates(id, ctx, id.executable_names())
        .into_iter()
        .map(|(path, _)| path)
        .find(|path| path.is_file())
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
/// `extra_override` 是调用方自带的专用路径（例如某个 VPN 配置里单独指定的连接器）。
pub fn locate(id: AppId, ctx: &AppContext, extra_override: &str) -> AppLocation {
    let override_text = extra_override.trim().to_string();

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
            note: if source == "custom" {
                "使用调用方指定的路径".to_string()
            } else {
                String::new()
            },
            builtin_path: builtin_text,
            candidates,
        },
        None => {
            let mut missing = AppLocation::missing(id, missing_note(&override_text));
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
        format!("指定的路径不存在：{}", override_path)
    } else {
        "已查找：手动指定、程序托管目录、内置资源、注册表、常见安装目录、PATH".to_string()
    }
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

/// 把用户「选中」的路径解析成真正的可执行文件。
///
/// macOS/Linux 上的「导入」就是**指定已有 openconnect 的运行位置**，不复制文件。
/// 用户会怎么选都得认，现场就遇到三种：
/// - 直接选中可执行文件本身（`.../bin/openconnect`）；
/// - 选中它所在的目录（`.../bin`）；
/// - 选中 Homebrew Cellar 的版本目录（`.../Cellar/openconnect/9.21`，可执行文件在 `bin/` 里）。
///
/// 只在这些形态里找，不做全盘搜索：选错目录宁可报错，也不要随手指到一个八竿子打不着的文件。
#[cfg(unix)]
pub fn resolve_binary(id: AppId, source: &Path) -> Result<PathBuf, String> {
    // 用 metadata（跟随软链）：Homebrew 的 `<prefix>/bin/openconnect` 本身就是软链，
    // 用 symlink_metadata 会把软链判成「既不是文件也不是目录」。
    let metadata = match std::fs::metadata(source) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("路径不存在: {}（可能是指向已卸载版本的断链）", source.display()))
        }
        Err(err) => {
            return Err(format!(
                "无法访问 {}: {}（若被 macOS 隐私保护拦截，到「系统设置 → 隐私与安全性 → \
                 完全磁盘访问权限」给本程序授权后重试）",
                source.display(),
                err
            ))
        }
    };

    let known_names = id.executable_names();
    if metadata.is_file() {
        let name = source.file_name().and_then(|value| value.to_str()).unwrap_or("");
        if !known_names.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            return Err(format!(
                "选中的不是 {} 可执行文件: {}（应选 {} 本身，或它所在的目录）",
                id.as_str(),
                source.display(),
                known_names.join(" / ")
            ));
        }
        return Ok(source.to_path_buf());
    }

    if !metadata.is_dir() {
        return Err(format!("既不是文件也不是目录: {}", source.display()));
    }

    // 目录：直接子项 → bin/ 子目录 → 再往下浅搜一层（版本目录里常见 bin/openconnect）
    for name in known_names {
        let direct = source.join(name);
        if direct.is_file() {
            return Ok(direct);
        }
        let in_bin = source.join("bin").join(name);
        if in_bin.is_file() {
            return Ok(in_bin);
        }
    }
    for name in known_names {
        if let Some(found) = find_file_below(source, name, 2)
            .into_iter()
            .find(|path| path.is_file())
        {
            return Ok(found);
        }
    }

    Err(format!(
        "没有找到 {}：请选可执行文件本身、它所在的目录，或安装目录（例如 Homebrew 的 9.21 版本目录）",
        known_names.join(" / ")
    ))
}

/// 把 Homebrew Cellar 里的版本路径改指到稳定的 `prefix/bin/`。
///
/// `.../Cellar/openconnect/9.21/bin/openconnect` 里的版本号会随 `brew upgrade` 变，
/// 直接软链过去升级一次就断链；`prefix/bin/openconnect` 是 brew 自己维护的稳定软链。
/// 不是 Cellar 结构（自定义目录、手工编译的二进制）就原样返回。
#[cfg(unix)]
pub fn stable_binary_path(binary: &Path) -> PathBuf {
    let name = match binary.file_name() {
        Some(name) => name,
        None => return binary.to_path_buf(),
    };
    let mut dir = binary.parent();
    while let Some(current) = dir {
        let is_cellar = current
            .file_name()
            .and_then(|part| part.to_str())
            .map(|part| part == "Cellar")
            .unwrap_or(false);
        if is_cellar {
            if let Some(prefix) = current.parent() {
                let stable = prefix.join("bin").join(name);
                if stable.is_file() {
                    return stable;
                }
            }
            break;
        }
        dir = current.parent();
    }
    binary.to_path_buf()
}

/// 在托管目录里登记一个指向现有二进制的软链（macOS/Linux 不复制文件）。
///
/// 为什么不学 Windows 那样复制：
/// - Homebrew 的 openconnect 通过绝对路径加载自己前缀里的动态库，拷出来的副本只是
///   碰巧能跑，`brew upgrade` 之后依赖就失配；
/// - 复制要先读用户目录里的文件，而自定义前缀恰好容易被权限/隐私策略挡住 ——
///   现场报的就是 `复制 openconnect 失败: Permission denied (os error 13)`。
///
/// 软链只写不读源文件，登记完再做一次 `--version` 校验，断链和权限问题当场暴露。
#[cfg(unix)]
pub fn link_binary(id: AppId, binary: &Path, data_dir: &Path) -> Result<PathBuf, String> {
    let name = binary
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| {
            id.executable_names()
                .iter()
                .any(|known| known.eq_ignore_ascii_case(value))
        })
        .ok_or_else(|| format!("不是 {} 的可执行文件: {}", id.as_str(), binary.display()))?;

    let target_dir = managed_dir(id, data_dir);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("创建目标目录失败 {}: {}", target_dir.display(), e))?;

    let link = target_dir.join(name);
    // 重复登记要能覆盖：symlink 已存在时再建会 EEXIST
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(&link)
            .map_err(|e| format!("清理旧的托管副本失败 {}: {}", link.display(), e))?;
    }

    std::os::unix::fs::symlink(binary, &link)
        .map_err(|e| format!("登记软链失败 {}: {}", link.display(), e))?;
    Ok(link)
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

    // Electron 系的编辑器（VS Code / Trae / Cursor）直接用 <应用>.exe <目录>
    // 在部分机器上什么都不发生，走它们自带的 CLI（bin\code.cmd 之类）才稳，
    // CLI 负责单实例转发和「在哪个窗口打开」。CLI 是隐藏控制台跑的，
    // 编辑器窗口由它自己拉起，所以这里用 spawn_detached。
    #[cfg(target_os = "windows")]
    if let Some(shim) = cli_shim_for(executable) {
        let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
        return crate::process::spawn_detached(
            Path::new(&shell),
            &[
                "/C",
                &shim.to_string_lossy(),
                &quote_launch_argument(target),
            ],
        )
        .map_err(|e| format!("启动失败: {}", e));
    }

    // 其余程序交给 ShellExecuteW：和资源管理器里双击是同一套激活逻辑，
    // 工作目录、控制台、单实例转发都由 shell 处理。直接用 CreateProcess
    // 会带上 CREATE_NO_WINDOW，控制台程序拿不到窗口（终端、PowerShell 点了
    // 没反应就是这个原因）。
    #[cfg(target_os = "windows")]
    {
        if is_script(executable) {
            // .cmd/.bat 交给 cmd 解释；CreateProcess 不做 PATHEXT 扩展
            let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
            return crate::process::shell_launch(
                Path::new(&shell),
                &[
                    "/C",
                    &executable.to_string_lossy(),
                    &quote_launch_argument(target),
                ],
                Some(Path::new(target)),
            )
            .map_err(|e| format!("启动失败: {}", e));
        }

        return crate::process::shell_launch(executable, &[target], Some(Path::new(target)))
            .map_err(|e| format!("启动失败: {}", e));
    }

    #[cfg(not(target_os = "windows"))]
    crate::process::spawn_detached(executable, &[target]).map_err(|e| format!("启动失败: {}", e))
}

/// 找 Electron 编辑器自带的命令行入口：`<安装目录>\bin\<名字>.cmd`
///
/// 只有这些「exe 直接传目录不灵」的编辑器才需要，其它程序返回 `None`。
#[cfg(target_os = "windows")]
pub fn cli_shim_for(executable: &Path) -> Option<PathBuf> {
    let stem = executable.file_stem()?.to_string_lossy().to_lowercase();
    let shim_name = match stem.as_str() {
        "code" | "code - insiders" => "code.cmd",
        "trae cn" | "trae" => "trae.cmd",
        "trae-solo-cn" => "trae-solo-cn.cmd",
        "cursor" => "cursor.cmd",
        _ => return None,
    };
    let shim = executable.parent()?.join("bin").join(shim_name);
    shim.is_file().then_some(shim)
}

/// 交给 `cmd /C "<脚本>" <参数>` 的路径：去掉引号再加引号，防止被二次解析
#[cfg(target_os = "windows")]
fn quote_launch_argument(value: &str) -> String {
    format!("\"{}\"", value.replace('"', ""))
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

    /// VS Code / Trae 这类编辑器要优先用自带的 CLI（bin\code.cmd），
    /// 直接给 exe 传目录在部分机器上什么都没发生
    #[cfg(target_os = "windows")]
    #[test]
    fn electron_editors_use_their_cli_shim() {
        // 不用 temp_dir()：这台机器上 %TEMP% 建目录会被拒（权限问题），
        // 放在 target 里更省事
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join("cli-shim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();

        let exe = dir.join("Code.exe");
        std::fs::write(&exe, b"stub").unwrap();
        assert!(cli_shim_for(&exe).is_none(), "没有 bin\\code.cmd 时不该瞎猜");

        std::fs::write(bin.join("code.cmd"), b"@echo off").unwrap();
        assert_eq!(cli_shim_for(&exe), Some(bin.join("code.cmd")));

        // JetBrains 不走这套，给了 shim 也不该用
        assert!(cli_shim_for(&dir.join("idea64.exe")).is_none());

        let _ = std::fs::remove_dir_all(&dir);
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

    /// 「指定位置」登记的那份只在系统里没有标准安装时才顶上。
    ///
    /// 反过来也成立：系统路径里已经有一份（用户后来把 openconnect 装回去了），就该用
    /// 系统那份，界面上的「指定路径」也随之不再显示。
    #[test]
    fn manual_registration_is_used_only_without_standard_install() {
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

        // 本机标准路径里有没有装：有就断言系统优先，没有才轮到登记的那份
        let standard = location
            .candidates
            .iter()
            .find(|candidate| candidate.source == "system" && candidate.exists);
        match standard {
            Some(candidate) => {
                assert_eq!(location.source, "system", "标准安装要优先于手动登记");
                assert_eq!(location.path, candidate.path);
            }
            None => {
                assert_eq!(location.source, "managed");
                assert_eq!(location.path, exe.to_string_lossy().to_string());
            }
        }

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

    /// 优先级：专用路径 > 标准安装位置 > 「指定位置」登记 > PATH
    ///
    /// 现场场景：先手动指定 `~/hb`，后来 openconnect 换到标准前缀，再点「检测」必须
    /// 变成系统那份、界面上不再显示「已指定」的路径。顺序反了就会一直用旧登记。
    #[test]
    fn standard_install_outranks_manual_registration() {
        assert!(source_priority("custom") < source_priority("system"));
        assert!(source_priority("system") < source_priority("managed"));
        assert!(source_priority("managed") < source_priority("path"));
        // 注册表是 Windows 上的「系统里装好的」，和固定安装位置同档，别被排到最后
        assert_eq!(source_priority("registry"), source_priority("system"));
    }

    /// collect 的输出必须真的按上面的优先级排好（稳定排序，同来源内部不串位）
    #[test]
    fn candidates_are_ordered_by_source_priority() {
        let root = temp_dir("collect-order");
        let ctx = AppContext {
            data_dir: Some(root.join("data")),
            ..AppContext::default()
        };
        let collected = collect(
            AppId::Openconnect,
            &ctx,
            AppId::Openconnect.executable_names(),
        );

        let ranks: Vec<u8> = collected
            .iter()
            .map(|(_, source)| source_priority(source))
            .collect();
        assert!(
            ranks.windows(2).all(|pair| pair[0] <= pair[1]),
            "候选没按来源优先级排好: {:?}",
            collected
        );

        // 本机装了标准版时，它必须排在登记的那份前面（没装就跳过这半段断言）
        let system_at = collected.iter().position(|(_, source)| source == "system");
        let managed_at = collected.iter().position(|(_, source)| source == "managed");
        if let (Some(system_at), Some(managed_at)) = (system_at, managed_at) {
            assert!(
                system_at < managed_at,
                "标准安装位置要排在「指定位置」登记之前: {:?}",
                collected
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 用户可能直接选二进制、选 bin 目录、选 Cellar 的版本目录 —— 三种都得认
    #[cfg(unix)]
    #[test]
    fn resolve_binary_accepts_file_bin_dir_and_version_dir() {
        let root = temp_dir("resolve-source");
        let version_dir = root.join("hb/Cellar/openconnect/9.21");
        let bin = version_dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let binary = bin.join("openconnect");
        std::fs::write(&binary, b"stub").unwrap();

        assert_eq!(
            resolve_binary(AppId::Openconnect, &binary).unwrap(),
            binary,
            "选中可执行文件本身"
        );
        assert_eq!(
            resolve_binary(AppId::Openconnect, &bin).unwrap(),
            binary,
            "选中 bin 目录"
        );
        assert_eq!(
            resolve_binary(AppId::Openconnect, &version_dir).unwrap(),
            binary,
            "选中 Cellar 的版本目录（可执行文件在 bin/ 里）"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 选错文件不能硬塞：名字对不上就直接报错
    #[cfg(unix)]
    #[test]
    fn resolve_binary_rejects_unrelated_file() {
        let root = temp_dir("resolve-wrong");
        let other = root.join("TODO");
        std::fs::write(&other, b"stub").unwrap();
        let error = resolve_binary(AppId::Openconnect, &other).unwrap_err();
        assert!(error.contains("不是 openconnect"), "实际错误: {}", error);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Homebrew 的 `<prefix>/bin/openconnect` 是软链：选它必须当成可执行文件，
    /// 不能因为 lstat 看到的是 symlink 就判成「既不是文件也不是目录」
    #[cfg(unix)]
    #[test]
    fn resolve_binary_follows_symlink() {
        let root = temp_dir("resolve-symlink");
        let real = root.join("Cellar/openconnect/9.21/bin/openconnect");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, b"stub").unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let link = root.join("bin/openconnect");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(
            resolve_binary(AppId::Openconnect, &link).unwrap(),
            link,
            "brew 的 prefix/bin 入口是软链，选它要能识别成可执行文件"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Cellar 里的版本目录会随 brew 升级消失，软链必须优先指 prefix/bin
    #[cfg(unix)]
    #[test]
    fn cellar_binary_prefers_stable_prefix_bin() {
        let root = temp_dir("stable-path");
        let prefix = root.join("hb");
        let binary = prefix.join("Cellar/openconnect/9.21/bin/openconnect");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"stub").unwrap();

        // prefix/bin 还没有时原样返回
        assert_eq!(stable_binary_path(&binary), binary);

        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::os::unix::fs::symlink(&binary, prefix.join("bin/openconnect")).unwrap();
        assert_eq!(
            stable_binary_path(&binary),
            prefix.join("bin/openconnect"),
            "brew 升级后 Cellar 版本目录会变，软链要指向稳定的 prefix/bin"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 托管副本必须是软链：不能复制二进制，也不能动源文件
    #[cfg(unix)]
    #[test]
    fn link_binary_registers_symlink_instead_of_copying() {
        let root = temp_dir("link-binary");
        let data = root.join("data");
        let binary = root.join("hb/bin/openconnect");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"stub").unwrap();

        let link = link_binary(AppId::Openconnect, &binary, &data).unwrap();
        assert_eq!(
            link,
            managed_dir(AppId::Openconnect, &data).join("openconnect")
        );
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink(), "托管副本必须是软链");
        assert_eq!(std::fs::read_link(&link).unwrap(), binary);
        assert!(binary.is_file(), "源文件必须原样保留");

        // 重复登记要能覆盖，不能 EEXIST
        link_binary(AppId::Openconnect, &binary, &data).unwrap();

        let _ = std::fs::remove_dir_all(&root);
    }
}
