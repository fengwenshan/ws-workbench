//! 子进程调用工具
//!
//! 从 `vpn::platform::common` 里提取出来的通用能力：带超时地跑一个外部命令并把
//! stdout + stderr 一起收回来。Windows 上同时抑制控制台黑框。

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Windows 上隐藏子进程控制台窗口
#[cfg(target_os = "windows")]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 给子进程钉死 C locale。
///
/// openconnect 9.x 会根据系统语言翻译自己的输出：中文系统上 `openconnect --version`
/// 打的是「OpenConnect 版本 v9.21」。而探测靠英文前缀 `OpenConnect version ` 解析
/// 版本号，worker 还靠 `Login failed` / `CSTP connected` / `Set up tun device failed`
/// 这些英文行判断认证失败和隧道状态——输出一翻译就全失效。
///
/// 现场就是这么炸的：终端里跑（`LANG=C.UTF-8`）一切正常，打包成 .app 从 Finder
/// 启动时环境里没有 `LANG` / `LC_ALL`，gettext 退回系统语言，界面报
/// 「未找到 openconnect，请先安装」，可 `which openconnect` 明明指向
/// `.../bin/openconnect`。GUI 进程拿不到稳定的 `LANG`，不能靠环境碰运气，
/// 凡是输出要被解析的子进程都统一钉死。
///
/// 注意只给「输出要被解析」的命令用；拉起用户的编辑器 / 终端这类可见程序
/// 不要带，否则会把它们也变成英文界面。
pub fn apply_c_locale(command: &mut Command) {
    command
        .env("LC_ALL", "C")
        .env("LANG", "C")
        // GNU gettext 里 LANGUAGE 的优先级高于 LC_ALL，不删掉等于没设；
        // LC_MESSAGES 也可能被程序单独优先读取
        .env_remove("LANGUAGE")
        .env_remove("LC_MESSAGES");
}

/// 把当前进程（连同它拉起的全部子进程）钉到 C locale。
///
/// 只给 VPN worker 用：它是独立进程，只跑隧道和系统命令，不会拉起用户可见的
/// GUI 程序，整棵进程树钉死没有副作用（openconnect 本身和它执行的 vpnc-script
/// 都会继承）。主界面进程不要调用——它会拉起 VS Code / 终端这类应用，
/// 那些得保留用户自己的语言设置。
pub fn pin_child_locale() {
    // edition 2021 下 std::env::set_var 仍是安全函数
    std::env::set_var("LC_ALL", "C");
    std::env::set_var("LANG", "C");
    std::env::remove_var("LANGUAGE");
    std::env::remove_var("LC_MESSAGES");
}

/// 起一个**后台**控制台程序（curl / taskkill / cscript 这类）时用它。
///
/// 直接 `Command::new` 从 GUI 进程里拉控制台程序，Windows 会为它新建一个
/// 控制台窗口——用户看到的就是「启动时闪两个黑框」。这里统一带上
/// `CREATE_NO_WINDOW`，输出仍然走管道。
pub fn hidden(program: &str) -> Command {
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    apply_c_locale(&mut command);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 带超时地执行一个命令，返回 (stdout+stderr, 是否正常退出)
pub fn run_with_timeout(program: &Path, args: &[&str], timeout: Duration) -> (String, bool) {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_c_locale(&mut command);

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
///
/// Windows 上启动用户可见的 GUI/控制台程序请用 [`shell_launch`]；
/// 这里的 `CREATE_NO_WINDOW` 只适合纯后台调用。
#[cfg_attr(target_os = "windows", allow(dead_code))]
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

/// 交给 shell 打开一个路径（Windows 上是 `ShellExecuteW`），等价于资源管理器里双击。
///
/// 目录由资源管理器打开，文件按关联程序打开。
///
/// 注意两点：
/// 1. 目录不能直接作为 `lpFile` 交给 `ShellExecuteW` —— 实测会稳定返回
///    `SE_ERR_ACCESSDENIED(5)`，所以目录一律显式启动 `explorer.exe`。
/// 2. 有些受限环境（沙箱、安全软件）会拦掉 shell 激活，这时退回
///    `cmd /C start "" <路径>`，由 cmd 去激活，能成的话至少能打开。
#[cfg(target_os = "windows")]
pub fn shell_open(target: &Path) -> Result<(), String> {
    if !target.is_dir() {
        return shell_execute(target.as_os_str(), "", None);
    }

    let path = target.to_string_lossy().to_string();
    let explorer = std::env::var_os("SystemRoot")
        .map(|root| std::path::PathBuf::from(root).join("explorer.exe"))
        .filter(|exe| exe.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from("explorer.exe"));

    match shell_launch(&explorer, &[&path], None) {
        Ok(()) => Ok(()),
        Err(first) => {
            let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
            let mut command = std::process::Command::new(shell);
            command
                .arg("/C")
                .arg("start")
                .arg("")
                .arg(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                // CREATE_NO_WINDOW：别闪黑框
                command.creation_flags(0x0800_0000);
            }
            command
                .spawn()
                .map(|_| ())
                .map_err(|_| format!("{}（退回 cmd start 也失败）", first))
        }
    }
}

/// 交给 shell 启动一个程序并传参（Windows 上是 `ShellExecuteW`）。
///
/// 为什么不用 `CreateProcess`（[`spawn_detached`]）：那条路会继承本进程的
/// stdio 句柄，还得带 `CREATE_NO_WINDOW` 才不闪黑框 —— 控制台程序因此拿不到
/// 自己的窗口（终端、PowerShell 点了没反应），Electron 应用也可能因为环境不完整
/// 直接退出。`ShellExecuteW` 走的是「双击」那条激活路径，窗口、控制台、工作
/// 目录都交给 shell 安排，最接近用户手动打开。
#[cfg(target_os = "windows")]
pub fn shell_launch(program: &Path, args: &[&str], cwd: Option<&Path>) -> Result<(), String> {
    let parameters = args
        .iter()
        .map(|arg| quote_parameter(arg))
        .collect::<Vec<_>>()
        .join(" ");
    shell_execute(program.as_os_str(), &parameters, cwd)
}

/// 传给 `ShellExecuteW` 的参数：带空格的路径要加引号
#[cfg(target_os = "windows")]
fn quote_parameter(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }
    if value.contains(' ') || value.contains('\t') {
        format!("\"{}\"", value)
    } else {
        value.to_string()
    }
}

#[cfg(target_os = "windows")]
fn shell_execute(
    program: &std::ffi::OsStr,
    parameters: &str,
    cwd: Option<&Path>,
) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut core::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut core::ffi::c_void;
    }

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    let operation = wide(OsStr::new("open"));
    let file = wide(program);
    let parameters = wide(OsStr::new(parameters));
    let directory = cwd.map(|dir| wide(dir.as_os_str()));

    // SW_SHOWNORMAL = 1
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            1,
        )
    };

    // ShellExecuteW 的返回值 <= 32 表示错误码
    let code = result as isize;
    if code > 32 {
        return Ok(());
    }
    Err(match code {
        2 => "找不到要打开的文件".to_string(),
        3 => "找不到路径".to_string(),
        5 => "系统拒绝访问（权限不足）".to_string(),
        31 => "没有能打开它的程序".to_string(),
        other => format!("系统返回错误码 {}", other),
    })
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

    /// 解析类子进程必须看到 C locale：openconnect 的版本行会跟着系统语言翻译，
    /// 从 Finder 启动的 .app 没有 LANG，不钉死就会把中文行当成「没有版本号」。
    #[cfg(unix)]
    fn locale_echo_script(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ws-locale-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("echo-locale.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf 'LC_ALL=%s LANGUAGE=%s\\n' \"${LC_ALL:-unset}\" \"${LANGUAGE:-unset}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, script)
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_pins_c_locale_for_child() {
        let (dir, script) = locale_echo_script("rt");
        let (text, ok) = run_with_timeout(&script, &[], Duration::from_secs(5));
        assert!(ok, "{}", text);
        assert!(
            text.contains("LC_ALL=C"),
            "子进程必须看到 LC_ALL=C: {}",
            text
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 父进程环境里已有的 zh_CN / LANGUAGE 要被显式覆盖/删掉，
    /// 不能指望「子进程碰巧没继承」。
    #[cfg(unix)]
    #[test]
    fn apply_c_locale_overrides_inherited_values() {
        let (dir, script) = locale_echo_script("apply");
        let mut command = Command::new(&script);
        command
            .env("LC_ALL", "zh_CN.UTF-8")
            .env("LANGUAGE", "zh_CN");
        apply_c_locale(&mut command);
        let output = command.output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(output.status.success(), "{}", text);
        assert!(text.contains("LC_ALL=C"), "实际输出: {}", text);
        assert!(text.contains("LANGUAGE=unset"), "实际输出: {}", text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ShellExecuteW 的参数里，带空格的路径必须自己加引号，否则会被拆成多个参数
    #[cfg(target_os = "windows")]
    #[test]
    fn shell_parameters_quote_paths_with_spaces() {
        assert_eq!(quote_parameter(r"C:\plain\path"), r"C:\plain\path");
        assert_eq!(
            quote_parameter(r"C:\app\Trae CN"),
            "\"C:\\app\\Trae CN\""
        );
        assert_eq!(quote_parameter(""), "\"\"");
    }

    /// `shell_launch` 的 cwd 必须真的传给子进程：终端 / PowerShell 就靠它定位目录
    #[cfg(target_os = "windows")]
    #[test]
    fn shell_launch_runs_in_the_given_directory() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join("shell-cwd");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("cwd.txt");
        let _ = std::fs::remove_file(&marker);

        let command = format!("cd > {}", marker.display());
        shell_launch(Path::new("cmd.exe"), &["/C", &command], Some(&dir)).unwrap();

        let mut content = String::new();
        for _ in 0..50 {
            if let Ok(text) = std::fs::read_to_string(&marker) {
                content = text;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            content.trim().eq_ignore_ascii_case(&dir.display().to_string()),
            "子进程的工作目录不对: {:?}",
            content
        );
    }

    /// 手工冒烟：真的打开一次资源管理器和一个控制台。
    ///
    /// 会弹窗口，所以默认 `#[ignore]`，需要时手动跑：
    /// `cargo test --lib -- --ignored shell_window_smoke`
    #[cfg(target_os = "windows")]
    #[ignore = "会真的打开资源管理器 / 控制台窗口"]
    #[test]
    fn shell_window_smoke() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let dir_str = dir.display().to_string();
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join("shell-smoke.txt");
        std::fs::create_dir_all(out.parent().unwrap()).ok();

        let mut report = String::new();
        report.push_str(&format!("dir = {}\n", dir_str));
        report.push_str(&format!("shell_open(dir) = {:?}\n", shell_open(&dir)));
        report.push_str(&format!(
            "shell_launch(cmd /K) = {:?}\n",
            shell_launch(Path::new("cmd.exe"), &["/K"], Some(&dir))
        ));

        // 对比：直接 CreateProcess 起 explorer.exe（不走 ShellExecute）
        report.push_str(&format!(
            "Command::spawn(explorer.exe, dir) = {:?}\n",
            std::process::Command::new(r"C:\Windows\explorer.exe")
                .arg(&dir)
                .spawn()
                .map(|_| ())
        ));

        std::fs::write(&out, &report).ok();
        std::thread::sleep(Duration::from_secs(2));
    }
}
