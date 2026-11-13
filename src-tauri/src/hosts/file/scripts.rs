//! macOS / Windows 提权脚本的纯字符串逻辑：路径转义与脚本文本生成。
//!
//! 本文件不含任何平台系统调用，所有平台都编译：macOS 上也能跑 Windows 脚本的
//! 单测，反之亦然。平台实现文件（`macos.rs` / `windows.rs`）只调用这里的函数
//! 拼脚本，不自己重复实现。
//!
//! 脚本里所有插值路径都必须转义：macOS 用 POSIX 单引号，Windows 用 PowerShell 的 `''`。
//! 这两条都有单测。
//!
//! 每对函数只被对应平台实现和单测使用，非本平台的非 test 构建里会显示为未使用，
//! 因此统一放行 dead_code。
#![allow(dead_code)]

use std::path::Path;

/// macOS sh 的单引号转义：`it's` → `'it'\''s'`。
pub(super) fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// PowerShell 单引号字符串转义：`'` → `''`。
///
/// 与 POSIX 的 `'\''` 完全不同：PowerShell 单引号字符串里写 `'\''` 会提前闭合字符串，
/// 路径里的单引号于是能变成命令分隔符。
pub(super) fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// 生成 macOS 提权 sh 脚本（纯函数，单测检查路径转义与步骤顺序）。
pub(super) fn macos_write_script(
    content_file: &Path,
    destination: &Path,
    staging: &Path,
) -> String {
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

/// 生成 Windows 提权 ps1 脚本（纯函数，单测检查 `''` 转义与步骤）。
pub(super) fn windows_write_script(
    content_file: &Path,
    destination: &Path,
    result_file: &Path,
) -> String {
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
           if ($LASTEXITCODE -eq 0) {{ $lines += 'WS_FLUSH_OK' }} else {{ $lines += 'WS_FLUSH_FAIL' }}\n\
         }} catch {{\n\
           $lines += 'WS_FLUSH_FAIL'\n\
         }}\n\
         Set-Content -LiteralPath {result_file} -Value $lines -Encoding UTF8\n"
    )
}

// ============================== 单测 ==============================

#[cfg(test)]
mod tests {
    use super::*;

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
    fn windows_script_checks_flush_exit_code() {
        let script = windows_write_script(
            Path::new(r"C:\temp\hosts.txt"),
            Path::new(r"C:\Windows\System32\drivers\etc\hosts"),
            Path::new(r"C:\temp\result.log"),
        );
        // 原生命令的非零退出码不会进 catch，必须显式看 $LASTEXITCODE
        assert!(script.contains("ipconfig /flushdns"));
        assert!(script.contains("$LASTEXITCODE -eq 0"), "{script}");
        assert!(script.contains("WS_FLUSH_FAIL"));
    }
}
