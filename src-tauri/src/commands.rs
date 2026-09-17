use std::path::{Path, PathBuf};
use tauri::AppHandle;
use crate::config;
use crate::packer;
use crate::models::*;

#[tauri::command]
pub fn get_config(app: AppHandle) -> AppConfig {
    config::load_config(&app)
}

#[tauri::command]
pub fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    config::save_config(&app, &config)
}

#[tauri::command]
pub fn get_config_path(app: AppHandle) -> Result<String, String> {
    config::get_config_path(&app)
}

#[tauri::command]
pub fn get_default_exclude_rules() -> Vec<String> {
    default_exclude_rules()
}

#[tauri::command]
pub async fn pack_project(
    app: AppHandle,
    project: ProjectConfig,
    default_exclude: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_project(&app, &project, &default_exclude)
}

#[tauri::command]
pub async fn pack_to_zip(
    app: AppHandle,
    project: ProjectConfig,
    default_exclude: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_to_zip(&app, &project, &default_exclude)
}

#[tauri::command]
pub fn validate_project(project: ProjectConfig) -> ProjectValidation {
    packer::validate_project(&project)
}

#[tauri::command]
pub fn detect_project_type(source_dir: String) -> ProjectType {
    packer::detect_project_type(&source_dir)
}

#[tauri::command]
pub fn detect_vcs(source_dir: String) -> VcsInfo {
    packer::detect_vcs(&source_dir)
}

// ===== 目录打开相关 =====

/// 查找最近的存在目录：若路径不存在，则逐级向上查找直至找到存在的父目录
fn nearest_existing_dir(path: &str) -> PathBuf {
    let mut cur = PathBuf::from(path);
    loop {
        if cur.exists() {
            return cur;
        }
        match cur.parent() {
            // 已到根目录仍不存在，返回原路径交由调用方报错
            Some(p) if p.as_os_str().is_empty() => return PathBuf::from(path),
            Some(p) => {
                let parent = p.to_path_buf();
                if parent == cur {
                    return PathBuf::from(path);
                }
                cur = parent;
            }
            None => return PathBuf::from(path),
        }
    }
}

fn open_path(path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(format!("路径不存在: {}", path));
    }
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";

    std::process::Command::new(cmd)
        .arg(path)
        .spawn()
        .map_err(|e| format!("打开失败: {}", e))?;
    Ok(())
}

// 各平台分支各自 return，末尾的 Ok(()) 只对 Linux 等其它 unix 平台可达，
// 在 macOS/Windows 上属于死代码，这里显式放行该 lint。
#[allow(unreachable_code)]
fn open_in_app(app_name: &str, path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(format!("路径不存在: {}", path));
    }
    #[cfg(target_os = "macos")]
    {
        // macOS: 用 open -a 打开，支持多个候选名称
        let candidates: Vec<&str> = match app_name {
            "Visual Studio Code" => vec!["Visual Studio Code"],
            "IDEA" => vec![
                "IntelliJ IDEA Ultimate",
                "IntelliJ IDEA",
                "IDEA",
            ],
            "Trae" => vec![
                "Trae CN",
                "TRAE SOLO CN",
                "Trae",
            ],
            _ => vec![app_name],
        };
        for name in &candidates {
            let result = std::process::Command::new("open")
                .arg("-a")
                .arg(name)
                .arg(path)
                .output();
            match result {
                Ok(output) => {
                    if output.status.success() {
                        return Ok(());
                    }
                    // 继续尝试下一个候选名称
                }
                Err(_) => {
                    // 继续尝试下一个候选名称
                }
            }
        }
        return Err(format!(
            "用 {} 打开失败，未找到对应应用，候选: {}",
            app_name,
            candidates.join(", ")
        ));
    }
    #[cfg(target_os = "windows")]
    {
        // Windows 上不能直接按名字启动：见 win_launcher 模块顶部注释
        let candidates = win_launcher::candidates(app_name);
        if candidates.is_empty() {
            return Err(format!(
                "未找到 {} 的启动程序。{}",
                win_launcher::display_name(app_name),
                win_launcher::hint(app_name)
            ));
        }
        let mut last_err = String::new();
        for launcher in &candidates {
            match win_launcher::spawn(launcher, path) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = format!("{}（{}）", e, launcher.path.display()),
            }
        }
        return Err(format!(
            "用 {} 打开失败：{}",
            win_launcher::display_name(app_name),
            last_err
        ));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let cmd = match app_name {
            "Visual Studio Code" => "code",
            "IDEA" => "idea.sh",
            "Trae" => "trae",
            _ => app_name,
        };
        std::process::Command::new(cmd)
            .arg(path)
            .spawn()
            .map_err(|e| format!("用 {} 打开失败: {}", app_name, e))?;
    }
    Ok(())
}

/// 打开目录；若目录不存在，自动降级打开最近存在的上级目录
/// 返回实际打开的路径，供前端提示
#[tauri::command]
pub fn open_dir(path: String) -> Result<String, String> {
    let target = nearest_existing_dir(&path);
    let target_str = target.to_string_lossy().to_string();
    // 若最终仍不存在，给出明确错误
    if !target.is_dir() {
        return Err(format!("路径不存在: {}", path));
    }
    open_path(&target_str)?;
    Ok(target_str)
}

#[tauri::command]
pub fn open_parent_dir(path: String) -> Result<(), String> {
    let parent = Path::new(&path)
        .parent()
        .ok_or_else(|| format!("无法获取上级目录: {}", path))?
        .to_string_lossy()
        .to_string();
    open_path(&parent)
}

#[tauri::command]
pub fn open_in_vscode(path: String) -> Result<(), String> {
    open_in_app("Visual Studio Code", &path)
}

#[tauri::command]
pub fn open_in_idea(path: String) -> Result<(), String> {
    open_in_app("IDEA", &path)
}

#[tauri::command]
pub fn open_in_trae(path: String) -> Result<(), String> {
    open_in_app("Trae", &path)
}

/// 在终端中打开目录；若目录不存在，自动降级打开最近存在的上级目录
/// 返回实际打开的路径，供前端提示
#[tauri::command]
pub fn open_in_terminal(path: String) -> Result<String, String> {
    let target = nearest_existing_dir(&path);
    let target_str = target.to_string_lossy().to_string();
    // 若最终仍不存在，给出明确错误
    if !target.is_dir() {
        return Err(format!("路径不存在: {}", path));
    }

    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(&target_str)
            .output()
            .map_err(|e| format!("打开终端失败: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "打开终端失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .arg("/c")
            .arg("start")
            .arg("cmd")
            .arg("/K")
            .arg(format!("cd /d {}", target_str))
            .spawn()
            .map_err(|e| format!("打开终端失败: {}", e))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let candidates = [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ];
        let mut last_err = String::new();
        let mut opened = false;
        for cmd in candidates {
            match std::process::Command::new(cmd).current_dir(&target_str).spawn() {
                Ok(_) => {
                    opened = true;
                    break;
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        if !opened {
            return Err(format!("打开终端失败: {}", last_err));
        }
    }

    Ok(target_str)
}

// ===== Windows 启动器定位 =====
//
// 为什么需要这一层：
//   Windows 上 VSCode / IDEA / Trae 的命令行入口通常是安装目录里的
//   `code.cmd` / `idea.bat` 这类批处理包装脚本，而且安装时未必写进 PATH
//   （例如 IntelliJ IDEA 的 bin 目录默认就不在 PATH 里）。
//   而 Windows 的 CreateProcess 在 PATH 中只查找 `<名字>.exe`：
//   它既不做 PATHEXT 后缀扩展，也不能直接执行 .cmd/.bat，
//   于是 `Command::new("code")` / `Command::new("idea64")` 一律以
//   os error 2（系统找不到指定的文件）失败，表现为点按钮毫无反应。
//   这里先把启动器解析成确定的绝对路径，再决定用 exe 还是 cmd.exe 启动。
#[cfg(target_os = "windows")]
mod win_launcher {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    /// CREATE_NO_WINDOW：批处理包装脚本不必弹出控制台黑框
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// 解析完成的候选启动器
    pub struct Launcher {
        pub path: PathBuf,
        /// 是否为 .cmd/.bat 包装脚本（必须交给 cmd.exe 解释执行）
        script: bool,
    }

    impl Launcher {
        fn new(path: PathBuf) -> Self {
            let script = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
                .unwrap_or(false);
            Self { path, script }
        }
    }

    fn env_dir(name: &str) -> Option<PathBuf> {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    }

    /// 环境变量指定的启动器路径（用户自定义安装位置的逃生通道）
    fn env_override(name: &str) -> Option<PathBuf> {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_file())
    }

    /// PATHEXT：cmd.exe 用来补全可执行后缀的环境变量
    fn pathext() -> Vec<String> {
        let fallback = ".COM;.EXE;.BAT;.CMD";
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| fallback.to_string());
        let list: Vec<String> = raw
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if list.is_empty() {
            fallback.split(';').map(|s| s.to_string()).collect()
        } else {
            list
        }
    }

    /// 按 cmd.exe 的规则在 PATH 中查找：
    /// 名字自带后缀就直接找；不带后缀时再依次尝试 PATHEXT 里的每个后缀。
    fn search_path(names: &[&str]) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let Some(path) = std::env::var_os("PATH") else {
            return found;
        };
        let exts = pathext();
        for dir in std::env::split_paths(&path) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            for name in names.iter().copied() {
                let direct = dir.join(name);
                if direct.is_file() {
                    found.push(direct);
                    continue;
                }
                // 名字没写后缀：按 PATHEXT 逐个补全（"code" → "code.cmd"）
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

    /// 目录下名字包含 needle（忽略大小写）的子目录，按名字倒序，让新版本排在前面
    fn sub_dirs_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
        let needle = needle.to_lowercase();
        let mut dirs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let matched = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_lowercase().contains(&needle))
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

    /// 在 dir 下最多 max_depth 层内查找指定文件名（用于 JetBrains Toolbox 的深目录结构）
    fn find_file_below(dir: &Path, file_name: &str, max_depth: usize) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![(dir.to_path_buf(), 0usize)];
        while let Some((cur, depth)) = stack.pop() {
            if depth > max_depth {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&cur) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push((path, depth + 1));
                } else if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.eq_ignore_ascii_case(file_name))
                    .unwrap_or(false)
                {
                    found.push(path);
                }
            }
        }
        found.sort();
        found.reverse();
        found
    }

    fn push_file(out: &mut Vec<Launcher>, path: PathBuf) {
        if path.is_file() {
            out.push(Launcher::new(path));
        }
    }

    /// 去重（Windows 路径不区分大小写）并保持原有优先级
    fn dedup(list: Vec<Launcher>) -> Vec<Launcher> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for item in list {
            let key = item.path.to_string_lossy().to_lowercase();
            if seen.insert(key) {
                out.push(item);
            }
        }
        out
    }

    /// VSCode 及同构分支的常见安装根目录
    fn vscode_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for base in [
            "LOCALAPPDATA",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
        ] {
            let Some(dir) = env_dir(base) else { continue };
            for name in ["Microsoft VS Code", "VSCode", "Microsoft VS Code Insiders"] {
                roots.push(dir.join("Programs").join(name));
                roots.push(dir.join(name));
            }
        }
        roots
    }

    fn vscode_candidates() -> Vec<Launcher> {
        let mut out = Vec::new();
        if let Some(p) = env_override("WS_VSCODE_PATH") {
            out.push(Launcher::new(p));
        }
        // 由 PATH 里的 code.cmd 反推安装根目录，覆盖 D:\xxx 这类自定义安装位置
        for script in search_path(&["code.cmd", "code.bat"]) {
            if let Some(root) = script.parent().and_then(|bin| bin.parent()) {
                push_file(&mut out, root.join("Code.exe"));
            }
        }
        for root in vscode_roots() {
            push_file(&mut out, root.join("Code.exe"));
        }
        // 兜底：直接用 PATH 中的命令行包装脚本
        out.extend(
            search_path(&["code.cmd", "code.bat", "code.exe", "Code.exe"])
                .into_iter()
                .map(Launcher::new),
        );
        dedup(out)
    }

    fn idea_candidates() -> Vec<Launcher> {
        let mut out = Vec::new();
        if let Some(p) = env_override("WS_IDEA_PATH") {
            out.push(Launcher::new(p));
        }
        // 用户若手动把 IDE 的 bin 目录加进了 PATH，这里直接命中
        out.extend(
            search_path(&["idea64.exe", "idea.exe", "idea.cmd", "idea.bat"])
                .into_iter()
                .map(Launcher::new),
        );
        // 独立安装：%ProgramFiles%\JetBrains\IntelliJ IDEA <版本>\bin\idea64.exe
        for base in [
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "LOCALAPPDATA",
        ] {
            let Some(dir) = env_dir(base) else { continue };
            for jetbrains in [dir.join("JetBrains"), dir.join("Programs").join("JetBrains")] {
                for ide in sub_dirs_containing(&jetbrains, "intellij idea") {
                    push_file(&mut out, ide.join("bin").join("idea64.exe"));
                    push_file(&mut out, ide.join("bin").join("idea.exe"));
                }
            }
        }
        // JetBrains Toolbox：%LOCALAPPDATA%\JetBrains\Toolbox\apps\IDEA-*\ch-*\*\bin\idea64.exe
        if let Some(local) = env_dir("LOCALAPPDATA") {
            let toolbox = local.join("JetBrains").join("Toolbox");
            let apps = toolbox.join("apps");
            for app in sub_dirs_containing(&apps, "idea") {
                out.extend(
                    find_file_below(&app, "idea64.exe", 4)
                        .into_iter()
                        .map(Launcher::new),
                );
            }
            // Toolbox 生成的命令行脚本
            for script in ["idea.cmd", "idea.bat", "idea64.exe"] {
                push_file(&mut out, toolbox.join("scripts").join(script));
            }
        }
        dedup(out)
    }

    fn trae_candidates() -> Vec<Launcher> {
        let mut out = Vec::new();
        if let Some(p) = env_override("WS_TRAE_PATH") {
            out.push(Launcher::new(p));
        }
        // 用户若把安装目录的 bin 加进了 PATH，这里直接命中
        out.extend(
            search_path(&[
                "trae-solo-cn.cmd",
                "trae-solo-cn.bat",
                "trae.cmd",
                "trae.bat",
                "trae.exe",
                "Trae.exe",
            ])
            .into_iter()
            .map(Launcher::new),
        );
        // Trae 是 VS Code 分支：安装目录里 CLI 包装脚本叫 bin\trae-solo-cn.cmd，
        // 主程序是根目录的「TRAE SOLO CN.exe」
        for base in [
            "LOCALAPPDATA",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
        ] {
            let Some(dir) = env_dir(base) else { continue };
            for parent in [dir.join("Programs"), dir.clone()] {
                for name in ["TRAE SOLO CN", "Trae CN", "Trae Solo CN", "Trae"] {
                    let root = parent.join(name);
                    push_file(&mut out, root.join(format!("{}.exe", name)));
                    push_file(&mut out, root.join("Trae.exe"));
                    push_file(&mut out, root.join("Code.exe"));
                    push_file(&mut out, root.join("bin").join("trae-solo-cn.cmd"));
                    push_file(&mut out, root.join("bin").join("code.cmd"));
                }
            }
        }
        dedup(out)
    }

    fn generic_candidates(app_name: &str) -> Vec<Launcher> {
        let mut out = Vec::new();
        out.extend(
            search_path(&[app_name])
                .into_iter()
                .map(Launcher::new),
        );
        dedup(out)
    }

    /// 按优先级列出该应用所有可用的启动器
    pub fn candidates(app_name: &str) -> Vec<Launcher> {
        match app_name {
            "Visual Studio Code" => vscode_candidates(),
            "IDEA" => idea_candidates(),
            "Trae" => trae_candidates(),
            other => generic_candidates(other),
        }
    }

    /// 界面/日志里展示的应用名
    pub fn display_name(app_name: &str) -> &str {
        match app_name {
            "IDEA" => "IntelliJ IDEA",
            other => other,
        }
    }

    /// 找不到时的排查提示
    pub fn hint(app_name: &str) -> String {
        match app_name {
            "Visual Studio Code" => {
                "已查找 PATH 中的 code.cmd/Code.exe 及常见安装目录；\
                 可用环境变量 WS_VSCODE_PATH 指定 Code.exe 的完整路径"
                    .to_string()
            }
            "IDEA" => {
                "已查找 PATH、%ProgramFiles%\\JetBrains 及 JetBrains Toolbox；\
                 可用环境变量 WS_IDEA_PATH 指定 idea64.exe 的完整路径"
                    .to_string()
            }
            "Trae" => {
                "已查找 PATH 及 %LOCALAPPDATA%\\Programs 下的 Trae 安装目录；\
                 可用环境变量 WS_TRAE_PATH 指定启动程序的完整路径"
                    .to_string()
            }
            other => format!("已查找 PATH 中名为 {} 的可执行文件", other),
        }
    }

    /// 启动：.exe 直接拉起，.cmd/.bat 交给 cmd.exe 解释
    pub fn spawn(launcher: &Launcher, path: &str) -> std::io::Result<()> {
        use std::os::windows::process::CommandExt;

        let mut command = if launcher.script {
            let shell = std::env::var_os("COMSPEC")
                .unwrap_or_else(|| std::ffi::OsString::from("cmd.exe"));
            let mut c = Command::new(shell);
            c.arg("/C").arg(&launcher.path).arg(path);
            c
        } else {
            let mut c = Command::new(&launcher.path);
            c.arg(path);
            c
        };

        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
    }
}
