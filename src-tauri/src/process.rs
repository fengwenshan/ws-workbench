//! 子进程调用工具
//!
//! 从 `vpn::detect` 里提取出来的通用能力：带超时地跑一个外部命令并把
//! stdout + stderr 一起收回来。Windows 上同时抑制控制台黑框。

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Windows 上隐藏子进程控制台窗口
#[cfg(target_os = "windows")]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 带超时地执行一个命令，返回 (stdout+stderr, 是否正常退出)
pub fn run_with_timeout(program: &Path, args: &[&str], timeout: Duration) -> (String, bool) {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return (format!("启动失败: {}", e), false),
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return ("执行超时".to_string(), false);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return (format!("等待进程失败: {}", e), false),
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => return (format!("读取输出失败: {}", e), false),
    };

    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (text, output.status.success())
}

/// 启动一个外部程序（不等待），返回值用于日志
pub fn spawn_detached(program: &Path, args: &[&str]) -> std::io::Result<()> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout() {
        #[cfg(target_os = "windows")]
        let (text, ok) = run_with_timeout(
            Path::new("cmd.exe"),
            &["/c", "echo hello"],
            Duration::from_secs(10),
        );
        #[cfg(not(target_os = "windows"))]
        let (text, ok) = run_with_timeout(
            Path::new("/bin/echo"),
            &["hello"],
            Duration::from_secs(10),
        );

        assert!(ok);
        assert!(text.contains("hello"), "实际输出: {}", text);
    }

    #[test]
    fn reports_missing_program() {
        let (text, ok) = run_with_timeout(
            Path::new("definitely-not-a-real-program-xyz"),
            &[],
            Duration::from_secs(5),
        );
        assert!(!ok);
        assert!(text.contains("启动失败"), "实际输出: {}", text);
    }

    #[test]
    fn kills_on_timeout() {
        #[cfg(target_os = "windows")]
        let (text, ok) = run_with_timeout(
            Path::new("cmd.exe"),
            &["/c", "ping -n 20 127.0.0.1 > nul"],
            Duration::from_millis(600),
        );
        #[cfg(not(target_os = "windows"))]
        let (text, ok) = run_with_timeout(
            Path::new("/bin/sleep"),
            &["20"],
            Duration::from_millis(600),
        );

        assert!(!ok);
        assert_eq!(text, "执行超时");
    }
}
