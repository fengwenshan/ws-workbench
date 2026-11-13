//! Windows hosts 路径与提权写入实现。
//!
//! 平台实现文件：只在 Windows target 编译，禁止引用 `file::macos`。
//!
//! 路径取 `%SystemRoot%\System32\drivers\etc\hosts`（取不到 `SystemRoot` 时用
//! `C:\Windows` 兜底）。写入走一次系统授权（设计 4.3）：生成 ps1 脚本（文本生成在
//! `scripts.rs`）→ `Start-Process -Verb RunAs -Wait` 提权 PowerShell `Copy-Item`
//! 覆盖 hosts → `ipconfig /flushdns`。

use std::path::PathBuf;

use super::scripts::{powershell_quote, windows_write_script};
use super::{is_canceled, unique_temp_path, write_temp_file, WriteOutcome};

/// 系统 hosts 文件路径（平台固定）。
pub(super) fn hosts_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32")
        .join("drivers")
        .join("etc")
        .join("hosts")
}

pub(super) fn write_hosts(content: &str) -> Result<WriteOutcome, String> {
    let destination = hosts_path();
    let content_file = unique_temp_path("hosts", "txt");
    let result_file = unique_temp_path("hosts-result", "log");
    let inner_path = unique_temp_path("hosts-elevated", "ps1");
    write_temp_file(&content_file, content, 0o644)?;

    let script = windows_write_script(&content_file, &destination, &result_file);
    if let Err(err) = write_temp_file(&inner_path, &script, 0o644) {
        let _ = std::fs::remove_file(&content_file);
        return Err(err);
    }

    let powershell = crate::commands::powershell_exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "powershell.exe".to_string());
    let launcher = format!(
        "$p = Start-Process -FilePath {} -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',{}) -Verb RunAs -Wait -PassThru; if ($null -ne $p) {{ $p.ExitCode }} else {{ 0 }}",
        powershell_quote(&powershell),
        powershell_quote(&inner_path.to_string_lossy()),
    );
    let (text, ok) =
        match crate::winps::run_with_timeout_ms(&launcher, std::time::Duration::from_secs(10 * 60))
        {
            Ok(text) => (text, true),
            Err(error) => (error, false),
        };
    // 提权进程的输出抓不到，脚本把结果写到临时文件回传
    let parsed = std::fs::read_to_string(&result_file).unwrap_or_default();
    let _ = std::fs::remove_file(&inner_path);
    let _ = std::fs::remove_file(&content_file);
    let _ = std::fs::remove_file(&result_file);

    if !ok {
        return Err(if is_canceled(&text) {
            "已取消授权".to_string()
        } else {
            format!("写入 hosts 失败：{}", text.trim())
        });
    }
    if !parsed.contains("WS_WRITE_OK") {
        return Err(format!(
            "写入 hosts 失败：{}",
            if parsed.trim().is_empty() {
                "提权脚本没有回传成功标记（可能被安全软件拦截）"
            } else {
                parsed.trim()
            }
        ));
    }
    Ok(WriteOutcome {
        flushed_dns: parsed.contains("WS_FLUSH_OK"),
    })
}
