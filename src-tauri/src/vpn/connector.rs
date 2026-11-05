//! 连接器：真正把隧道拉起来的那一层
//!
//! 只有一条通道 —— **openconnect**（Windows 用内置副本，macOS 用 Homebrew 装的）：
//! 用 `--passwd-on-stdin` 把密码通过管道喂进去，密码不会出现在命令行里，
//! 也不会落盘到任何配置文件；同时用 `--script` 换成我们自己生成的脚本，
//! 从而完全接管路由决策。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::vpn::models::VpnRequest;

/// Windows 上隐藏子进程控制台窗口
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 统一的子进程构造：隐藏窗口、管道化标准流
fn base_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 组装 openconnect 的参数列表（单独抽出来便于测试）
pub fn openconnect_args(req: &VpnRequest) -> Vec<String> {
    let mut args = vec![
        format!("--protocol={}", req.protocol),
        format!("--user={}", req.username),
        "--passwd-on-stdin".to_string(),
        format!("--script={}", req.script_path),
    ];
    for extra in &req.extra_args {
        let trimmed = extra.trim();
        if !trimmed.is_empty() {
            args.push(trimmed.to_string());
        }
    }
    args.push(req.server.clone());
    args
}

/// 启动 openconnect 隧道进程
pub fn spawn_openconnect(req: &VpnRequest) -> Result<Child, String> {
    let path = PathBuf::from(&req.connector_path);
    if !path.is_file() {
        return Err(format!("openconnect 不存在: {}", path.display()));
    }

    let args = openconnect_args(req);
    let mut child = base_command(&path)
        .args(&args)
        .spawn()
        .map_err(|e| format!("启动 openconnect 失败: {}", e))?;

    // 立刻把密码写进管道再关闭：之后任何交互式提问都会直接读到 EOF 而快速失败，
    // 不会让进程挂在那里等输入。
    if let Some(stdin) = child.stdin.as_mut() {
        let mut payload = req.password.clone();
        payload.push('\n');
        stdin
            .write_all(payload.as_bytes())
            .map_err(|e| format!("写入密码失败: {}", e))?;
        let _ = stdin.flush();
    }
    drop(child.stdin.take());
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::models::ConnectorKind;

    fn request() -> VpnRequest {
        VpnRequest {
            conn_identity: String::new(),
            proxy_mode: "rule".to_string(),
            profile_id: "p1".into(),
            profile_name: "公司 VPN".into(),
            server: "vpn.example.com:1218".into(),
            username: "13800138000".into(),
            password: "ExamplePass123".into(),
            protocol: "anyconnect".into(),
            connector: ConnectorKind::Auto,
            connector_path: r"C:\app\openconnect\openconnect.exe".into(),
            rules: vec![],
            default_policy: "DIRECT".into(),
            fake_ip_range: "198.18.0.0/16".into(),
            fake_ip_filter: vec![],
            learn: false,
            extra_args: vec![],
            script_path: r"C:\session\script\vpnc-script-win.js".into(),
            hook_exe: r"C:\app\ws-workbench.exe".into(),
            created_at: String::new(),
        }
    }

    #[test]
    fn openconnect_args_use_stdin_for_password() {
        let args = openconnect_args(&request());
        assert!(args.contains(&"--passwd-on-stdin".to_string()));
        // 密码绝不能出现在命令行里
        assert!(!args.iter().any(|a| a.contains("ExamplePass123")));
        assert!(args.contains(&"--protocol=anyconnect".to_string()));
        assert!(args.contains(&"--user=13800138000".to_string()));
        // 服务器地址必须在最后
        assert_eq!(args.last().unwrap(), "vpn.example.com:1218");
    }

    #[test]
    fn openconnect_args_point_script_at_our_own_copy() {
        let args = openconnect_args(&request());
        assert!(args
            .iter()
            .any(|a| a.starts_with("--script=") && a.ends_with("vpnc-script-win.js")));
    }

    #[test]
    fn extra_args_are_appended_before_server() {
        let mut req = request();
        req.extra_args = vec!["--servercert".into(), "pin-sha256:abc".into(), "  ".into()];
        let args = openconnect_args(&req);
        let server_pos = args.len() - 1;
        assert_eq!(args[server_pos - 1], "pin-sha256:abc");
        assert_eq!(args[server_pos - 2], "--servercert");
        // 空白项被丢弃
        assert!(!args.iter().any(|a| a.trim().is_empty()));
    }
}
