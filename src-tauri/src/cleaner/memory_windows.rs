//! Windows 侧内存占用的平台实现：产品目录归属、图标、结束进程、深度释放。
//!
//! 「按应用聚合」在 Windows 上没有 bundle 概念，只能按安装目录猜：
//! exe 位于 `Program Files*` / `%LOCALAPPDATA%\Programs` 下时取它的**产品目录**
//! 作为身份（Electron 应用的 helper、更新器等都在同一目录下）；父目录是
//! `bin` / 版本号时再往上一层。其余进程按 exe 路径单独成项。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::models::{MemoryApp, QuitOutcome};
use super::memory::ProcSample;

/// 安装根目录（顺序即匹配优先级）
fn install_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["PROGRAMFILES", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(value) = std::env::var_os(var) {
            roots.push(PathBuf::from(value));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Programs"));
    }
    roots
}

/// 目录名是否像版本号：`v1.2.3` / `1.2.3` / `2.0`
fn is_version_like(name: &str) -> bool {
    let trimmed = name.trim_start_matches(['v', 'V']);
    if trimmed.is_empty() || !trimmed.contains('.') {
        return false;
    }
    trimmed
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|char| char.is_ascii_digit()))
}

/// exe → 产品目录（找不到返回 None，调用方退化成按 exe 单独成项）
fn product_dir_of(exe: &Path) -> Option<PathBuf> {
    for root in install_roots() {
        let Ok(relative) = exe.strip_prefix(&root) else {
            continue;
        };
        // exe 直接躺在安装根下（如 Program Files\foo.exe）没有产品目录可言
        if relative.components().count() <= 1 {
            return None;
        }
        let mut product = exe.parent()?.to_path_buf();
        if product == root {
            return None;
        }
        // bin / Bin / bin64 这类目录是启动器所在层，产品目录在上层
        let last = product
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let lowered = last.to_ascii_lowercase();
        if lowered == "bin" || lowered == "bin32" || lowered == "bin64" || is_version_like(&last) {
            if let Some(parent) = product.parent() {
                if parent != root {
                    product = parent.to_path_buf();
                }
            }
        }
        return Some(product);
    }
    None
}

/// 进程 → 应用身份（id, 展示名）
pub fn identity(sample: &ProcSample) -> (String, String) {
    let exe = Path::new(&sample.exe);
    match product_dir_of(exe) {
        Some(product) => {
            let name = product
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| sample.name.clone());
            (product.to_string_lossy().to_string(), name)
        }
        None => (sample.exe.clone(), sample.name.clone()),
    }
}

/// 批量取应用图标：`id → data URL`。
///
/// Windows 上一次 PowerShell 调用批量取（`windows_icons`），比按应用逐个起
/// PowerShell 快一个数量级；图标源用聚合时记录的代表性 exe。
pub fn icons_for(apps: &[MemoryApp]) -> HashMap<String, Option<String>> {
    let mut result: HashMap<String, Option<String>> = HashMap::new();
    let sources: Vec<PathBuf> = apps
        .iter()
        .filter(|app| !app.exe.is_empty())
        .map(|app| PathBuf::from(&app.exe))
        .collect();
    let source_refs: Vec<&Path> = sources.iter().map(|path| path.as_path()).collect();
    let icons = crate::commands::openwith::windows_icons(&source_refs);
    for app in apps {
        let icon = icons
            .get(Path::new(&app.exe))
            .cloned();
        result.insert(app.id.clone(), icon);
    }
    result
}

/// 结束所选应用：优雅退出走 `CloseMainWindow`，强制结束走 `Stop-Process -Force`。
///
/// `CloseMainWindow` 只对有主窗口的进程有效（helper / 后台进程会被跳过），
/// 这是 Windows 上「发送关闭请求」的唯一通用做法；用户要立刻结束还是得强制。
pub fn quit_apps(apps: &[MemoryApp], force: bool) -> QuitOutcome {
    let pids: Vec<u32> = apps.iter().flat_map(|app| app.pids.iter().copied()).collect();
    if pids.is_empty() {
        return QuitOutcome::default();
    }
    let id_list = pids
        .iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let script = if force {
        format!(
            "$ids = @({}); Stop-Process -Id $ids -Force -ErrorAction SilentlyContinue; $ids.Count",
            id_list
        )
    } else {
        format!(
            "$ids = @({}); $found = @(Get-Process -Id $ids -ErrorAction SilentlyContinue); \
             $found | ForEach-Object {{ [void]$_.CloseMainWindow() }}; $found.Count",
            id_list
        )
    };

    match crate::winps::run(&script) {
        Ok(text) => {
            let count = text
                .lines()
                .last()
                .and_then(|line| line.trim().parse::<usize>().ok())
                .unwrap_or(pids.len());
            let mut outcome = QuitOutcome::default();
            if force {
                outcome.forced = count;
            } else {
                outcome.quit = count;
            }
            outcome
        }
        Err(error) => QuitOutcome {
            quit: 0,
            forced: 0,
            failed: vec![error],
        },
    }
}

/// 深度释放：一次 UAC 提权，先清各进程工作集，再清系统待机列表。
///
/// 两步都尽力而为：待机列表清理（`NtSetSystemInformation` 未文档化）失败不影响
/// 工作集那一步。用 `Start-Process -Verb RunAs` 触发一次 UAC，内层脚本写到临时
/// `.ps1` 再执行，避免嵌套引号把命令拆坏。
pub fn deep_release() -> Result<(), String> {
    let script_path = std::env::temp_dir().join(format!(
        "ws-workbench-cleaner-release-{}.ps1",
        uuid::Uuid::new_v4()
    ));
    let inner = r#"$ErrorActionPreference = 'Continue'
try {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class WsMem {
  [DllImport("psapi.dll", SetLastError = true)]
  public static extern bool EmptyWorkingSet(IntPtr hProcess);
  [DllImport("ntdll.dll")]
  public static extern int NtSetSystemInformation(int infoClass, ref int info, int length);
}
'@
} catch { }
foreach ($p in Get-Process) {
  try { [void][WsMem]::EmptyWorkingSet($p.Handle) } catch { }
}
try { $cmd = 4; [void][WsMem]::NtSetSystemInformation(0x50, [ref]$cmd, 4) } catch { }
exit 0
"#;
    std::fs::write(&script_path, inner).map_err(|err| format!("写入提权脚本失败: {}", err))?;

    let powershell = crate::commands::powershell_exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "powershell.exe".to_string());
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let launcher = format!(
        "$p = Start-Process -FilePath {} -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',{}) -Verb RunAs -Wait -PassThru; \
         if ($null -ne $p) {{ $p.ExitCode }} else {{ 0 }}",
        quote(&powershell),
        quote(&script_path.to_string_lossy()),
    );
    let (text, ok) = match crate::winps::run_with_timeout_ms(
        &launcher,
        std::time::Duration::from_secs(5 * 60),
    ) {
        Ok(text) => (text, true),
        Err(error) => (error, false),
    };
    let _ = std::fs::remove_file(&script_path);

    if !ok {
        let lowered = text.to_ascii_lowercase();
        if lowered.contains("cancel") || text.contains("已取消") || text.contains("1223") {
            return Err("已取消授权".to_string());
        }
        return Err(format!("深度释放失败: {}", text.trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_like_directory_names() {
        assert!(is_version_like("1.2.3"));
        assert!(is_version_like("v2.0"));
        assert!(!is_version_like("bin"));
        assert!(!is_version_like("1.2.3-beta"));
        assert!(!is_version_like("Google Chrome"));
    }
}
