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
        let cmd = match app_name {
            "Visual Studio Code" => "code",
            "IDEA" => "idea64",
            "Trae" => "trae",
            _ => app_name,
        };
        std::process::Command::new(cmd)
            .arg(path)
            .spawn()
            .map_err(|e| format!("用 {} 打开失败: {}", app_name, e))?;
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
