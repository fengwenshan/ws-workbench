//! 系统集成命令：打开目录/文件、目录校验、平台信息与外部应用定位。
//!
//! 「用哪个应用打开」在 [`crate::commands::openwith`] 里，这里只留通用的 shell 打开。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::apps::AppContext;
use crate::netinfo::NetworkInfo;
use crate::platform::PlatformInfo;

/// 查找最近的存在目录：若路径不存在，则逐级向上查找直至找到存在的父目录
pub(crate) fn nearest_existing_dir(path: &str) -> PathBuf {
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

pub(crate) fn open_path(path: &str) -> Result<(), String> {
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

/// 组装查找上下文：应用数据目录 / 资源目录 / 可执行文件目录。
///
/// 项目打包页、VPN 页共用这一份上下文，保证「用编辑器打开」与连接器探测看到同一批目录。
pub fn app_context(app: &AppHandle) -> AppContext {
    AppContext {
        data_dir: app.path().app_data_dir().ok(),
        resource_dir: app.path().resource_dir().ok(),
        exe_dir: std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf())),
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

/// 用系统默认程序打开文件（附件解密出来的临时文件等）。
///
/// 不走前端 plugin-shell 的 `open`：那条命令受 `shell:allow-open` 的 scope 正则限制，
/// 只放行 http(s)/mailto/tel，本地路径会被拒绝
/// （现场表现：点「打开」报「Scoped command argument at position 0 ... failed regex validation」）。
/// 这里复用 Rust 侧的 `open_path`，和 open_dir 一样不受前端 scope 约束。
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    if !Path::new(&path).is_file() {
        return Err(format!("文件不存在: {}", path));
    }
    open_path(&path)
}

// ============================== 平台与网络信息 ==============================

/// 平台信息：前端据此决定是否展示 Windows 专有的设置项
#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    PlatformInfo::current()
}

/// 本机网络接口（MAC / IP）：设置页「运行环境」展示用
#[tauri::command]
pub fn get_network_info() -> Result<NetworkInfo, String> {
    NetworkInfo::current()
}

/// 在终端里打开一个「已经确认存在」的目录。
///
/// 只有 `open_with` 的非 macOS 分支会用到：macOS 那边直接 `open -a Terminal.app`，
/// 走的是 `openwith` 里同一个启动函数，不需要这份平台分支。
#[cfg(not(target_os = "macos"))]
pub(crate) fn open_terminal_dir(target_str: &str) -> Result<(), String> {
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
            match std::process::Command::new(cmd)
                .current_dir(&target_str)
                .spawn()
            {
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

    Ok(())
}

/// 目录检测结果：判断记录里保存的目录是否还在
#[derive(serde::Serialize)]
pub struct DirCheck {
    pub exists: bool,
    pub is_dir: bool,
}

/// 只读检测路径状态，不创建、不修改任何东西。
///
/// 供前端在「分组上传目录 / 项目源目录」失效时给出提示；记录本身留在数据库里，
/// 用户重新选一个目录即可恢复（按实体 id 更新，不动笔记与附件元信息）。
#[tauri::command]
pub fn check_dir(path: String) -> DirCheck {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return DirCheck {
            exists: false,
            is_dir: false,
        };
    }
    let p = Path::new(trimmed);
    DirCheck {
        exists: p.exists(),
        is_dir: p.is_dir(),
    }
}
