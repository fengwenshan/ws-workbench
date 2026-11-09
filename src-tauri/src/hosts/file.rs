//! 系统 hosts 文件：路径、读取、提权写入、刷新 DNS（在线编辑保存 / 还原共用）。
//!
//! 路径固定（设计第 5 节，不接受前端传路径）：
//! - macOS：`/etc/hosts`
//! - Windows：`%SystemRoot%\System32\drivers\etc\hosts`（取不到 `SystemRoot` 时用 `C:\Windows` 兜底）
//!
//! 写入走一次系统授权（设计 4.3）：
//! - macOS：生成 sh 脚本 → `osascript -e 'do shell script "sh <quoted>" with administrator privileges'`
//!   脚本内 `cp` 到 `/etc/hosts.ws-workbench.tmp` → `chmod 644` → `mv -f` 原子替换 → 刷新 DNS；
//! - Windows：生成 ps1 脚本 → `Start-Process -Verb RunAs -Wait` 提权 PowerShell
//!   `Copy-Item` 覆盖 hosts → `ipconfig /flushdns`。
//!
//! 脚本里所有插值路径都必须转义：macOS 用 POSIX 单引号，Windows 用 PowerShell 的 `''`。
//! 这两条都有单测。

use std::path::{Path, PathBuf};

use super::MAX_CONTENT_LEN;

/// 提权写入的结果
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteOutcome {
    /// DNS 缓存是否刷新成功；失败不回滚 hosts（内容已生效），只如实报告
    pub flushed_dns: bool,
}

/// 系统 hosts 文件路径（平台固定）。
pub fn hosts_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("SystemRoot")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        return root
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts");
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/etc/hosts")
    }
}

/// hosts 文件所在目录（`hosts_open_file_dir` 用）。
pub fn hosts_dir() -> PathBuf {
    hosts_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/etc"))
}

/// hosts 文件是否存在。
pub fn hosts_exists() -> bool {
    hosts_path().exists()
}

/// 按原始字节读取当前 hosts；文件不存在返回空字节（首次应用时从空文件开始）。
pub fn read_hosts_bytes() -> Result<Vec<u8>, String> {
    match std::fs::read(hosts_path()) {
        Ok(bytes) => Ok(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("读取 hosts 失败：{err}")),
    }
}

/// 读当前 hosts 文本。
///
/// 非 UTF-8 时直接报错而不是 lossy 替换：预览 / 导入可以降级展示，但**写入**路径
/// 必须拿原文做「块外内容原样保留」的合并，替换字符会把块外内容写坏，所以宁可
/// 让用户先手动转码，也不能静默改坏系统文件。
pub fn read_hosts() -> Result<String, String> {
    let bytes = read_hosts_bytes()?;
    String::from_utf8(bytes).map_err(|_| {
        "hosts 文件不是 UTF-8 编码，暂不支持自动编辑（避免写坏原有内容）；请先手动把文件转成 UTF-8"
            .to_string()
    })
}

/// 提权把 `content` 写入系统 hosts 并刷新 DNS。
pub fn write_hosts(content: &str) -> Result<WriteOutcome, String> {
    if content.len() > MAX_CONTENT_LEN {
        return Err(format!(
            "生成内容超过 1 MiB 上限（当前 {} 字节）",
            content.len()
        ));
    }
    #[cfg(target_os = "macos")]
    {
        write_hosts_macos(content)
    }
    #[cfg(target_os = "windows")]
    {
        write_hosts_windows(content)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = content;
        Err("当前平台暂不支持写入 hosts".to_string())
    }
}

// ============================== 平台无关小工具 ==============================

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ws-workbench-{prefix}-{}.{extension}",
        uuid::Uuid::new_v4()
    ))
}

/// 写临时文件并尽量收紧权限：脚本里含 root 操作路径，不该让其它用户改。
fn write_temp_file(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    std::fs::write(path, content).map_err(|err| format!("写入临时文件失败：{err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    Ok(())
}

/// macOS sh 的单引号转义：`it's` → `'it'\''s'`。
#[cfg(any(target_os = "macos", test))]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// PowerShell 单引号字符串转义：`'` → `''`。
///
/// 与 POSIX 的 `'\''` 完全不同：PowerShell 单引号字符串里写 `'\''` 会提前闭合字符串，
/// 路径里的单引号于是能变成命令分隔符。
#[cfg(any(target_os = "windows", test))]
fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// 判断 osascript / UAC 的错误输出是否为「用户取消授权」。
fn is_canceled(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("cancel")
        || lower.contains("user canceled")
        || text.contains("已取消")
        || text.contains("-128")
        // Windows ERROR_CANCELLED：用户点了 UAC 的「否」
        || lower.contains("1223")
}

// ============================== macOS ==============================

/// 生成 macOS 提权 sh 脚本（纯函数，单测检查路径转义与步骤顺序）。
#[cfg(any(target_os = "macos", test))]
fn macos_write_script(content_file: &Path, destination: &Path, staging: &Path) -> String {
    let content_file = shell_quote(&content_file.to_string_lossy());
    let destination = shell_quote(&destination.to_string_lossy());
    let staging = shell_quote(&staging.to_string_lossy());
    format!(
        "#!/bin/sh\n\
         set -u\n\
         if /bin/cp {content_file} {staging} && /bin/chmod 644 {staging} && /bin/mv -f {staging} {destination}; then\n\
           echo WS_WRITE_OK\n\
         else\n\
           echo WS_WRITE_FAIL\n\
           /bin/rm -f {staging}\n\
           exit 1\n\
         fi\n\
         if /usr/bin/dscacheutil -flushcache >/dev/null 2>&1 && /usr/bin/killall -HUP mDNSResponder >/dev/null 2>&1; then\n\
           echo WS_FLUSH_OK\n\
         else\n\
           echo WS_FLUSH_FAIL\n\
         fi\n\
         /bin/rm -f {content_file}\n"
    )
}

#[cfg(target_os = "macos")]
fn write_hosts_macos(content: &str) -> Result<WriteOutcome, String> {
    let destination = hosts_path();
    let content_file = unique_temp_path("hosts", "txt");
    let staging = destination.with_file_name("hosts.ws-workbench.tmp");
    write_temp_file(&content_file, content, 0o644)?;

    let script_path = unique_temp_path("hosts-elevated", "sh");
    let script = macos_write_script(&content_file, &destination, &staging);
    if let Err(err) = write_temp_file(&script_path, &script, 0o700) {
        let _ = std::fs::remove_file(&content_file);
        return Err(err);
    }

    let command = format!("sh {}", shell_quote(&script_path.to_string_lossy()));
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let (text, ok) = crate::process::run_with_timeout(
        Path::new("/usr/bin/osascript"),
        &["-e", &apple_script],
        std::time::Duration::from_secs(10 * 60),
    );
    let _ = std::fs::remove_file(&script_path);
    let _ = std::fs::remove_file(&content_file);

    if !ok {
        return Err(if is_canceled(&text) {
            "已取消授权".to_string()
        } else {
            format!("写入 hosts 失败：{}", text.trim())
        });
    }
    if !text.contains("WS_WRITE_OK") {
        return Err(format!(
            "写入 hosts 失败：{}",
            if text.trim().is_empty() {
                "提权脚本没有回显成功标记"
            } else {
                text.trim()
            }
        ));
    }
    Ok(WriteOutcome {
        flushed_dns: text.contains("WS_FLUSH_OK"),
    })
}

// ============================== Windows ==============================

/// 生成 Windows 提权 ps1 脚本（纯函数，单测检查 `''` 转义与步骤）。
#[cfg(any(target_os = "windows", test))]
fn windows_write_script(content_file: &Path, destination: &Path, result_file: &Path) -> String {
    let content_file = powershell_quote(&content_file.to_string_lossy());
    let destination = powershell_quote(&destination.to_string_lossy());
    let result_file = powershell_quote(&result_file.to_string_lossy());
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $lines = @()\n\
         try {{\n\
           Copy-Item -LiteralPath {content_file} -Destination {destination} -Force\n\
           $lines += 'WS_WRITE_OK'\n\
         }} catch {{\n\
           $lines += 'WS_WRITE_FAIL'\n\
           Set-Content -LiteralPath {result_file} -Value $lines -Encoding UTF8\n\
           exit 1\n\
         }}\n\
         try {{\n\
           ipconfig /flushdns | Out-Null\n\
           $lines += 'WS_FLUSH_OK'\n\
         }} catch {{\n\
           $lines += 'WS_FLUSH_FAIL'\n\
         }}\n\
         Set-Content -LiteralPath {result_file} -Value $lines -Encoding UTF8\n"
    )
}

#[cfg(target_os = "windows")]
fn write_hosts_windows(content: &str) -> Result<WriteOutcome, String> {
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

// ============================== 单测 ==============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_path_is_fixed_per_platform() {
        #[cfg(target_os = "macos")]
        assert_eq!(hosts_path(), PathBuf::from("/etc/hosts"));
        #[cfg(target_os = "windows")]
        {
            let path = hosts_path();
            let text = path.to_string_lossy().to_ascii_lowercase();
            assert!(text.ends_with(r"\system32\drivers\etc\hosts"), "{text}");
        }
        // 目录用于 hosts_open_file_dir，必须是 hosts 的父目录
        assert_eq!(hosts_dir(), hosts_path().parent().expect("parent"));
    }

    #[test]
    fn shell_quote_wraps_and_escapes_single_quotes() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        // 常见宿主机路径：不能出现未转义的裸单引号
        let quoted = shell_quote("/Users/o'brien/Library/hosts");
        assert_eq!(quoted, "'/Users/o'\\''brien/Library/hosts'");
    }

    #[test]
    fn powershell_quote_doubles_single_quotes() {
        assert_eq!(
            powershell_quote("C:\\Windows\\hosts"),
            "'C:\\Windows\\hosts'"
        );
        assert_eq!(powershell_quote("C:\\it's\\hosts"), "'C:\\it''s\\hosts'");
        // 绝不能沿用 POSIX 的 '\''（在 PowerShell 里会提前闭合字符串）
        assert!(!powershell_quote("C:\\it's").contains("'\\''"));
    }

    #[test]
    fn macos_script_quotes_every_interpolated_path() {
        let script = macos_write_script(
            Path::new("/tmp/ws-workbench-hosts-1.txt"),
            Path::new("/etc/hosts"),
            Path::new("/etc/hosts.ws-workbench.tmp"),
        );
        assert!(script
            .contains("/bin/cp '/tmp/ws-workbench-hosts-1.txt' '/etc/hosts.ws-workbench.tmp'"));
        assert!(script.contains("/bin/mv -f '/etc/hosts.ws-workbench.tmp' '/etc/hosts'"));
        assert!(script.contains("WS_WRITE_OK"));
        assert!(script.contains("dscacheutil -flushcache"));
        assert!(script.contains("killall -HUP mDNSResponder"));
        assert!(script.contains("WS_FLUSH_OK"));
        // 先写临时文件 + chmod + 原子替换，再刷新 DNS
        let cp = script.find("/bin/cp").expect("cp");
        let flush = script.find("dscacheutil").expect("flush");
        assert!(cp < flush);
    }

    #[test]
    fn macos_script_escapes_hostile_path() {
        let script = macos_write_script(
            Path::new("/tmp/a b/it's tmp.txt"),
            Path::new("/etc/hosts"),
            Path::new("/etc/hosts.ws-workbench.tmp"),
        );
        // 单引号被 POSIX 转义成 '\''，不会截断命令
        assert!(script.contains("'/tmp/a b/it'\\''s tmp.txt'"));
    }

    #[test]
    fn windows_script_quotes_every_interpolated_path() {
        let script = windows_write_script(
            Path::new(r"C:\Users\demo\AppData\Local\Temp\hosts-1.txt"),
            Path::new(r"C:\Windows\System32\drivers\etc\hosts"),
            Path::new(r"C:\Users\demo\AppData\Local\Temp\result.log"),
        );
        assert!(script.contains(
            "Copy-Item -LiteralPath 'C:\\Users\\demo\\AppData\\Local\\Temp\\hosts-1.txt'"
        ));
        assert!(script.contains("-Destination 'C:\\Windows\\System32\\drivers\\etc\\hosts'"));
        assert!(script.contains(
            "Set-Content -LiteralPath 'C:\\Users\\demo\\AppData\\Local\\Temp\\result.log'"
        ));
        assert!(script.contains("ipconfig /flushdns"));
        assert!(script.contains("WS_WRITE_OK"));
        assert!(script.contains("WS_FLUSH_OK"));
    }

    #[test]
    fn windows_script_escapes_hostile_path() {
        let script = windows_write_script(
            Path::new(r"C:\it's\hosts.txt"),
            Path::new(r"C:\Windows\System32\drivers\etc\hosts"),
            Path::new(r"C:\temp\result.log"),
        );
        assert!(script.contains("'C:\\it''s\\hosts.txt'"));
    }

    #[test]
    fn cancel_detection_covers_osascript_and_uac() {
        assert!(is_canceled("execution error: User canceled. (-128)"));
        assert!(is_canceled("用户已取消"));
        assert!(is_canceled("ERROR_CANCELLED 1223"));
        assert!(!is_canceled("Permission denied"));
    }

    #[test]
    fn limits_are_sane() {
        assert_eq!(MAX_CONTENT_LEN, 1024 * 1024);
    }
}
