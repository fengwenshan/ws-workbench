//! 提权启动 worker
//!
//! Windows：注册一个 `RL HIGHEST` 的计划任务，之后每次连接只用 `schtasks /Run`
//! 触发，**不再弹 UAC**。只有首次注册那一下需要管理员确认。
//! 如果注册失败（比如用户没批准），自动退回「每次连接弹一次 UAC」的直接提权方式。
//!
//! macOS：brew 装的 openconnect 需要 root 才能建 utun，用 `osascript` 的
//! `with administrator privileges` 拉起 worker（系统授权框，与 Windows 的 UAC 对应）。
//!
//! 关键约束：提权进程**无法继承父进程的标准句柄**，所以 worker 的一切结果都通过
//! 会话目录里的文件回传（见 `session.rs`）。

use std::fs;
use std::path::Path;

use crate::vpn::session::VpnSession;

/// 计划任务名（Windows）
pub const TASK_NAME: &str = "WsToolsVpnWorker";

/// 提权辅助脚本的退出码文件
const ELEVATE_EXIT_FILE: &str = "elevate.exit";

/// macOS 提权启动的诊断文件：记录提权 shell 的启动标记与 worker 的 stdout/stderr。
/// 提权进程拿不到父进程的管道，只能靠它把失败现场落盘。
#[cfg(target_os = "macos")]
const ELEVATE_OUT_FILE: &str = "elevate.out";

/// 当前进程是否已经具备管理员权限
pub fn is_elevated() -> bool {
    #[cfg(target_os = "windows")]
    {
        let script = "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)";
        let (text, ok) = crate::process::run_with_timeout(
            Path::new("powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ],
            std::time::Duration::from_secs(20),
        );
        ok && text.trim().eq_ignore_ascii_case("true")
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Unix 下用 euid 判断
        unsafe { libc_geteuid() == 0 }
    }
}

#[cfg(not(target_os = "windows"))]
extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

/// 计划任务是否已经注册过
pub fn worker_task_exists() -> bool {
    #[cfg(target_os = "windows")]
    {
        let (_, ok) = crate::process::run_with_timeout(
            Path::new("schtasks.exe"),
            &["/Query", "/TN", TASK_NAME],
            std::time::Duration::from_secs(20),
        );
        ok
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

/// 读回已注册计划任务的完整定义（XML 文本）。
///
/// 任务里 `/TR` 把会话目录路径写死了，所以任务一旦注册就和那个目录绑死。
/// 会话目录如果发生回退（用户目录被策略/受控文件夹访问挡住），
/// 旧任务仍然指向旧目录，直接 /Run 会让 worker 和主进程读写两个地方。
fn worker_task_definition() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let (text, ok) = crate::process::run_with_timeout(
            Path::new("schtasks.exe"),
            &["/Query", "/TN", TASK_NAME, "/XML"],
            std::time::Duration::from_secs(20),
        );
        if ok {
            Some(text)
        } else {
            None
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// 已注册的计划任务是否就是指向当前会话目录。
///
/// `recorded` 是上次注册成功后记进 store 的目录，命中就能直接比字符串，
/// 不必真的去问 schtasks。为 None 说明没有记录（旧版本注册的任务，
/// 或者 store 写失败），这时才退回去读任务定义。
///
/// 读不到任务定义时保守地返回 true：宁可复用（可能失败并回退到 UAC 提权），
/// 也不要在每次连接时都重新注册、重新弹一次 UAC。
pub fn worker_task_matches(session_dir: &Path, recorded: Option<&str>) -> bool {
    if let Some(dir) = recorded {
        return paths_equal(dir, session_dir);
    }
    match worker_task_definition() {
        Some(definition) => definition_mentions_dir(&definition, session_dir),
        None => true,
    }
}

/// 两个目录路径是否指向同一个位置。
///
/// Windows 盘符和路径大小写不敏感，所以统一按小写比较；
/// 顺带把反斜杠归一化，兼容手写配置或从别处拷来的路径。
fn paths_equal(a: &str, b: &Path) -> bool {
    let normalize = |text: &str| {
        text.trim()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    let left = normalize(a);
    let right = normalize(&b.to_string_lossy());
    !left.is_empty() && left == right
}

/// 任务定义 XML 里是否出现了这个会话目录。
///
/// XML 里的路径不会被转义（引号会被转成 `&quot;`，但路径本身是原样的），
/// 所以直接做子串匹配即可。
fn definition_mentions_dir(definition: &str, session_dir: &Path) -> bool {
    let needle = session_dir.to_string_lossy();
    if needle.is_empty() {
        return false;
    }
    definition.contains(needle.as_ref())
}


/// 通过一个临时批处理文件提权执行命令，并把退出码写回文件。
///
/// 之所以绕这一圈：`Start-Process -Verb RunAs` 启动的进程既不继承句柄也不回传退出码，
/// 所以让批处理自己把结果写进文件，父进程轮询读取。
#[cfg(target_os = "windows")]
fn run_elevated_batch(session: &VpnSession, name: &str, body: &str) -> Result<(), String> {
    session.ensure()?;
    let exit_path = session.dir.join(ELEVATE_EXIT_FILE);
    let _ = fs::remove_file(&exit_path);

    let batch_path = session.dir.join(format!("{}.cmd", name));
    let batch = format!(
        "@echo off\r\n{}\r\necho %ERRORLEVEL% > \"{}\"\r\n",
        body,
        exit_path.display()
    );
    fs::write(&batch_path, batch)
        .map_err(|e| format!("写入提权脚本失败 {}: {}", batch_path.display(), e))?;

    // 用 PowerShell 的 RunAs 触发 UAC。-- 参数用单引号包住，避免路径里的空格被拆开
    let ps = format!(
        "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','\"{}\"' -Verb RunAs -Wait -WindowStyle Hidden",
        batch_path.display()
    );
    let (text, ok) = crate::process::run_with_timeout(
        Path::new("powershell.exe"),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &ps,
        ],
        std::time::Duration::from_secs(120),
    );

    // PowerShell 本身返回失败通常意味着用户拒绝了 UAC
    if !ok && !exit_path.exists() {
        return Err(format!(
            "提权被拒绝或失败（可能需要管理员批准）: {}",
            text.trim()
        ));
    }

    // 给批处理一点时间把退出码落盘
    for _ in 0..40 {
        if exit_path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    let code = fs::read_to_string(&exit_path)
        .map_err(|_| "提权脚本没有返回结果（可能被 UAC 拦截）".to_string())?;
    let code = code.trim().to_string();
    let _ = fs::remove_file(&exit_path);
    let _ = fs::remove_file(&batch_path);

    if code == "0" {
        Ok(())
    } else {
        Err(format!("提权脚本返回错误码 {}", code))
    }
}

/// 生成「注册计划任务」的 schtasks 命令。
///
/// `/TR` 里的路径必须用 `\"` 转义，schtasks 才认得嵌入的引号；
/// `/RL HIGHEST` 让任务以最高权限运行，之后 `/Run` 触发就不会再弹 UAC。
pub fn task_create_command(exe: &Path, session_dir: &Path) -> String {
    format!(
        "schtasks /Create /TN \"{}\" /TR \"\\\"{}\\\" --vpn-worker \\\"{}\\\"\" /SC ONCE /ST 00:00 /RL HIGHEST /F",
        TASK_NAME,
        exe.display(),
        session_dir.display()
    )
}

/// 生成「提权直接拉起 worker」的命令。
///
/// 必须用 `start` 让 worker 脱离批处理：worker 的生命周期是整个会话，
/// 直接执行的话批处理和外层的 `Start-Process -Wait` 会一直卡到断开为止。
pub fn worker_launch_command(exe: &Path, session_dir: &Path) -> String {
    format!(
        "start \"\" \"{}\" --vpn-worker \"{}\"",
        exe.display(),
        session_dir.display()
    )
}

/// 注册（或刷新）Windows 计划任务。返回 Ok(true) 表示任务可用。
pub fn ensure_worker_task(exe: &Path, session: &VpnSession) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let body = task_create_command(exe, &session.dir);
        match run_elevated_batch(session, "create-task", &body) {
            Ok(()) => Ok(worker_task_exists()),
            Err(e) => Err(e),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (exe, session);
        // macOS 走 osascript 提权，不需要预注册
        Ok(true)
    }
}

/// 删除计划任务
pub fn delete_worker_task() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let (text, ok) = crate::process::run_with_timeout(
            Path::new("schtasks.exe"),
            &["/Delete", "/TN", TASK_NAME, "/F"],
            std::time::Duration::from_secs(30),
        );
        if ok {
            Ok(())
        } else {
            Err(format!("删除计划任务失败: {}", text.trim()))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(())
    }
}

/// 启动 worker。
///
/// `recorded_task_dir` 是上次注册计划任务时用的会话目录（来自 store），
/// 用来判断现有任务是否还指向当前目录。为空表示没有记录，会去查任务定义。
pub fn start_worker(
    exe: &Path,
    session: &VpnSession,
    recorded_task_dir: Option<&str>,
) -> Result<String, String> {
    // 停止标记的生命周期只由 prepare()（新会话）与 stop_worker()（请求停止）掌控。
    // 这里再清一次会在「断开还没收尾就重连」时抹掉刚写下的停止请求，
    // 导致旧 worker 一直不被回收。
    session.ensure()?;

    #[cfg(target_os = "windows")]
    {
        // 除了「任务存在」，还得确认任务指向的就是当前会话目录：
        // 否则 /Run 起来的是往旧目录写的 worker，主进程等不到任何状态。
        if worker_task_exists() && worker_task_matches(&session.dir, recorded_task_dir) {
            let (text, ok) = crate::process::run_with_timeout(
                Path::new("schtasks.exe"),
                &["/Run", "/TN", TASK_NAME],
                std::time::Duration::from_secs(30),
            );
            if ok {
                return Ok("计划任务".to_string());
            }
            session.append_log(&format!(
                "[elevate] 计划任务启动失败，改用 UAC 提权: {}",
                text.trim()
            ));
        } else if worker_task_exists() {
            session.append_log(
                "[elevate] 计划任务指向的会话目录已过期，改用 UAC 提权拉起 worker",
            );
        }

        // 退回：直接提权拉起 worker
        let body = worker_launch_command(exe, &session.dir);
        run_elevated_batch(session, "run-worker", &body)?;
        Ok("UAC 提权".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let out_path = session.dir.join(ELEVATE_OUT_FILE);
        // 起始标记 + worker 的 stdout/stderr 都追加到同一个文件，方便区分
        // 「提权 shell 没跑起来」和「shell 跑了但 worker 立刻失败」。
        //
        // 刻意不用 nohup：`do shell script` 下没有控制终端，nohup 会因
        // TIOCNOTTY 失败而报 "can't detach from console: Inappropriate ioctl
        // for device" 后直接退出，worker 根本没机会启动。
        // 改用子 shell 后台化 `( ... & )`：子 shell 立刻退出，worker 被 launchd
        // 收养，三个标准流全部重定向（不依赖终端）即可长期存活。
        let command = format!(
            "echo \"[elevate] uid=$(id -u) at $(date +%H:%M:%S)\" >> {out}; \
             ( {exe} --vpn-worker {dir} >> {out} 2>&1 < /dev/null & )",
            out = shell_quote(&out_path.to_string_lossy()),
            exe = shell_quote(&exe.to_string_lossy()),
            dir = shell_quote(&session.dir.to_string_lossy()),
        );
        let script = format!(
            "do shell script \"{}\" with administrator privileges",
            command.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let (text, ok) = crate::process::run_with_timeout(
            Path::new("/usr/bin/osascript"),
            &["-e", &script],
            std::time::Duration::from_secs(120),
        );
        if !ok {
            return Err(format!("提权启动失败: {}", text.trim()));
        }

        // 命令末尾的 `&` 让 root shell 立刻退出 0，所以 osascript 的退出码
        // 永远为真，不能拿来判定 worker 是否真的起来了。
        // 改为主动等待 worker 写下首行日志（会话目录已被 prepare 清空过）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if session.log_path().exists() {
                return Ok("系统授权".to_string());
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err(format!(
            "提权授权已通过，但 worker 未启动，诊断信息见 {}",
            out_path.display()
        ))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (exe, session);
        Err("当前平台暂不支持提权启动".to_string())
    }
}

/// 请求 worker 停止（写 stop.flag），worker 自行清理路由与隧道后退出
pub fn stop_worker(session: &VpnSession) {
    session.request_stop();
}

#[cfg(target_os = "macos")]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_name_is_stable() {
        // 任务名会写进系统计划任务列表，改动需要同步清理旧任务
        assert_eq!(TASK_NAME, "WsToolsVpnWorker");
    }

    #[test]
    fn task_create_command_escapes_embedded_quotes() {
        let command = task_create_command(
            Path::new(r"C:\Program Files\ws-tools\ws-tools.exe"),
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn"),
        );
        // /TR 的值必须用 \" 包住带空格的路径，schtasks 才认得
        assert!(command.contains(r#"/TR "\"C:\Program Files\ws-tools\ws-tools.exe\" --vpn-worker \"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn\"""#));
        // 免 UAC 的关键参数
        assert!(command.contains("/RL HIGHEST"));
        assert!(command.contains("/F"));
    }

    #[test]
    fn worker_launch_command_detaches_from_batch() {
        let command = worker_launch_command(
            Path::new(r"C:\app\ws-tools.exe"),
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn"),
        );
        // 必须以 start 开头：否则批处理会一直等 worker 退出（整整一个会话）
        assert!(command.starts_with(r#"start "" "#));
        assert!(command.contains(r#"--vpn-worker "C:\Users\demo"#));
    }

    #[test]
    fn task_definition_matches_only_the_baked_in_directory() {
        // 模拟 schtasks /Query /XML 的真实输出：路径原样出现，引号被转成 &quot;
        let definition = r#"<Command>"C:\app\ws-tools.exe" --vpn-worker "C:\Users\demo\AppData\Roaming\com.ws.tools\vpn"</Command>"#;

        // 路径一致 → 可以复用免 UAC 任务
        assert!(definition_mentions_dir(
            definition,
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn")
        ));

        // 会话目录回退到别的盘/别的根 → 必须重新注册，不能复用旧任务
        assert!(!definition_mentions_dir(
            definition,
            Path::new(r"C:\Users\demo\AppData\Local\com.ws.tools\vpn")
        ));
        assert!(!definition_mentions_dir(
            definition,
            Path::new(r"C:\Users\demo\AppData\Local\Temp\com.ws.tools\vpn")
        ));
    }

    #[test]
    fn empty_directory_never_matches() {
        // 空路径是子串匹配的万能匹配，必须显式挡掉
        assert!(!definition_mentions_dir(
            "<Command>anything</Command>",
            Path::new("")
        ));
    }

    #[test]
    fn recorded_directory_is_compared_case_insensitively() {
        // Windows 上盘符和大小写都不敏感，用户手改过配置也要能对上
        assert!(paths_equal(
            r"c:\users\demo\appdata\roaming\com.ws.tools\vpn",
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn")
        ));
        // 尾部斜杠、正反斜杠混用也应视为同一目录
        assert!(paths_equal(
            "C:/Users/demo/AppData/Roaming/com.ws.tools/vpn/",
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn")
        ));
        // 空记录不能当成匹配，否则会误判成「任务可用」而不去重注册
        assert!(!paths_equal(
            "",
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn")
        ));
    }

    #[test]
    fn recorded_directory_wins_without_querying_schtasks() {
        let current = Path::new(r"C:\Users\demo\AppData\Local\com.ws.tools\vpn");
        // 记录一致 → 复用（此分支不会去读任务定义）
        assert!(worker_task_matches(
            current,
            Some(r"C:\Users\demo\AppData\Local\com.ws.tools\vpn")
        ));
        // 记录指向旧目录 → 必须重新注册，不能复用旧任务
        assert!(!worker_task_matches(
            current,
            Some(r"C:\Users\demo\AppData\Roaming\com.ws.tools\vpn")
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
