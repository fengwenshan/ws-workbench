use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use crate::apps::{self, AppContext, AppId, AppLocation, AppPathOverride};
use crate::config;
use crate::packer;
use crate::platform::PlatformInfo;
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

/// 加密任意字符串（供前端加密本地键值使用，复用后端主密钥）
#[tauri::command]
pub fn vault_encrypt(app: AppHandle, plain: String) -> Result<String, String> {
    crate::crypto::encrypt_str(&app, &plain)
}

/// 解密由 `vault_encrypt` 产生的密文
#[tauri::command]
pub fn vault_decrypt(app: AppHandle, cipher: String) -> Result<String, String> {
    crate::crypto::decrypt_str(&app, &cipher)
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

/// 用指定应用打开目录。
///
/// 可执行文件的解析全部交给 [`apps::locate`]：
/// 设置页指定的路径 > 程序托管副本 > 内置资源 > Windows 注册表 > 常见安装目录 > PATH。
/// 六条路都找不到时，macOS 上再用 `open -a` 按应用名兜一次
/// （那里应用是 .app 包，很多没有可执行入口）。
pub fn open_with_app(app: &AppHandle, id: AppId, path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(format!("路径不存在: {}", path));
    }

    let ctx = app_context(app);
    let location = apps::locate(id, &ctx, "");

    if location.available {
        return apps::finder::spawn_with_target(Path::new(&location.path), path);
    }

    #[cfg(target_os = "macos")]
    {
        for name in macos_app_names(id) {
            let result = std::process::Command::new("/usr/bin/open")
                .arg("-a")
                .arg(name)
                .arg(path)
                .output();
            if let Ok(output) = result {
                if output.status.success() {
                    return Ok(());
                }
            }
        }
    }

    Err(format!("未找到 {}。{}", id.display_name(), location.note))
}

/// 组装查找上下文：应用数据目录 / 资源目录 / 可执行文件目录 + 设置页里的路径覆盖。
///
/// 覆盖项放进来之后，项目打包页、VPN 页、设置页三处共享同一份设置。
pub fn app_context(app: &AppHandle) -> AppContext {
    let config = config::load_config(app);
    AppContext {
        data_dir: app.path().app_data_dir().ok(),
        resource_dir: app.path().resource_dir().ok(),
        exe_dir: std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf())),
        overrides: config.app_paths,
    }
}

/// macOS 上用 open -a 时按顺序尝试的应用名
#[cfg(target_os = "macos")]
fn macos_app_names(id: AppId) -> Vec<&'static str> {
    match id {
        AppId::Vscode => vec!["Visual Studio Code", "Visual Studio Code - Insiders"],
        AppId::Idea => vec![
            "IntelliJ IDEA Ultimate",
            "IntelliJ IDEA",
            "IntelliJ IDEA CE",
        ],
        AppId::Trae => vec!["Trae CN", "TRAE SOLO CN", "Trae"],
        AppId::Openconnect => vec![],
    }
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
pub fn open_in_vscode(app: AppHandle, path: String) -> Result<(), String> {
    open_with_app(&app, AppId::Vscode, &path)
}

#[tauri::command]
pub fn open_in_idea(app: AppHandle, path: String) -> Result<(), String> {
    open_with_app(&app, AppId::Idea, &path)
}

#[tauri::command]
pub fn open_in_trae(app: AppHandle, path: String) -> Result<(), String> {
    open_with_app(&app, AppId::Trae, &path)
}

/// 通用入口：按 app_id 打开目录，设置页的「测试启动」用它
#[tauri::command]
pub fn open_with_app_id(app: AppHandle, app_id: String, path: String) -> Result<(), String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    open_with_app(&app, id, &path)
}

// ============================== 应用定位与路径设置 ==============================

/// 平台信息：前端据此决定是否展示 Windows 专有的设置项
#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    PlatformInfo::current()
}

/// 列出所有外部应用的定位结果
#[tauri::command]
pub fn list_app_locations(app: AppHandle) -> Vec<AppLocation> {
    apps::locate_all(&app_context(&app))
}

/// 保存某个应用的手动指定路径；传空字符串表示清除，回到自动查找
#[tauri::command]
pub fn set_app_path(
    app: AppHandle,
    app_id: String,
    path: String,
) -> Result<Vec<AppLocation>, String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    let mut config = config::load_config(&app);
    let trimmed = path.trim().to_string();

    config.app_paths.retain(|item| item.id != id.as_str());
    if !trimmed.is_empty() {
        config
            .app_paths
            .push(AppPathOverride::new(id.as_str(), &trimmed));
    }
    config::save_config(&app, &config)?;
    Ok(apps::locate_all(&app_context(&app)))
}

/// 清除手动指定的路径
#[tauri::command]
pub fn clear_app_path(app: AppHandle, app_id: String) -> Result<Vec<AppLocation>, String> {
    set_app_path(app, app_id, String::new())
}

/// 把一个已有的应用目录复制到程序数据目录并托管
#[tauri::command]
pub fn import_app(app: AppHandle, app_id: String, source_dir: String) -> Result<String, String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {}", e))?;
    apps::finder::import_from_dir(id, Path::new(&source_dir), &data_dir)?;

    let location = apps::locate(id, &app_context(&app), "");
    if location.available {
        Ok(format!("已导入并托管 {}", id.display_name()))
    } else {
        Err(format!(
            "文件已复制，但仍未能定位到可执行文件，请在设置页手动指定。{}",
            location.note
        ))
    }
}

/// 移除程序托管的副本
#[tauri::command]
pub fn remove_managed_app(app: AppHandle, app_id: String) -> Result<String, String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {}", e))?;
    apps::finder::remove_managed(id, &data_dir)?;
    Ok(format!("已移除托管的 {}", id.display_name()))
}

/// 清空注册表缓存后重新查找。刚装完软件、注册表还没被缓存到时要调它。
#[tauri::command]
pub fn refresh_app_search(app: AppHandle) -> Vec<AppLocation> {
    apps::registry::clear_cache();
    list_app_locations(app)
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
