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

#[cfg(target_os = "windows")]
use std::fs;
use std::path::Path;

use crate::vpn::session::VpnSession;

/// 计划任务名（Windows）
pub const TASK_NAME: &str = "WsWorkbenchVpnWorker";

/// 提权辅助脚本的退出码文件（仅 Windows）
#[cfg(target_os = "windows")]
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
        // 不要用 `cmd /C ... > file` 再读：现场 11:54 的任务 XML 明明存在，
        // 这里却返回 None，worker_task_matches 退回 recorded=true，拉起了
        // 另一个目录里的旧 worker。直接拿 schtasks 的 stdout 原始字节，
        // 自己按 BOM 解 UTF-16，路径比较才不会被编码和 cmd 退出码干扰。
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};

        let output = Command::new("schtasks.exe")
            .args(["/Query", "/TN", TASK_NAME, "/XML"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(crate::process::CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = decode_task_xml(&output.stdout);
        // 解出替换字符说明编码判断错了；宁可当读不到（走重新注册/UAC），
        // 也不要拿一段乱码去和当前路径比较。
        (!text.trim().is_empty() && !text.contains('\u{FFFD}')).then_some(text)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// `schtasks /Query /XML` 的原始输出可能是 UTF-16LE/BE（带 BOM），也可能是
/// UTF-8（取决于 Windows 版本和重定向方式）。这里按 BOM 解码，避免中文路径
/// 变成 `D:\�����ļ�\...` 后匹配永远失败。
#[cfg(target_os = "windows")]
fn decode_task_xml(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    // 有些 Windows 的 schtasks 重定向输出不带 BOM。按 NUL 字节的位置判断
    // UTF-16 字节序：ASCII XML 的 UTF-16LE 奇数位是 0，UTF-16BE 偶数位是 0。
    let odd_nuls = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    let even_nuls = bytes.iter().step_by(2).filter(|b| **b == 0).count();
    let half = bytes.len() / 2;
    if half > 4 && odd_nuls > half / 2 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if half > 4 && even_nuls > half / 2 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// 已注册的计划任务是否指向**当前程序**和**当前会话目录**。
///
/// 只比对会话目录是不够的：便携版、安装版、不同版本的绿色包经常共用同一份
/// AppData 会话目录，只要目录对上就会一直复用由另一个 exe 注册的旧任务。
/// 现场表现就是新版主进程通过计划任务拉起了旧版 worker（worker 日志里的
/// 「程序」与主程序路径不同），旧 worker 仍按旧的心跳阈值判断，隧道刚建好
/// 就被它收掉——而新版的修复代码根本没被执行到。
///
/// `recorded` 是上次注册成功后记进 store 的目录，仅用于日志/兼容旧调用；
/// **不能**在读不到任务定义时单独拿它当“任务可用”的证据。
///
/// 现场 11:54 的教训：任务定义读取失败后按 recorded 目录返回 true，结果
/// `/Run` 拉起的是另一个目录里的旧 worker（主程序 1026，worker 1106），
/// 旧 worker 用旧心跳逻辑把隧道收掉。读不到定义时宁可返回 false，走一次
/// 重新注册/UAC，也不能拉起一个无法确认版本的 worker。
pub fn worker_task_matches(exe: &Path, session_dir: &Path, _recorded: Option<&str>) -> bool {
    match worker_task_definition() {
        Some(definition) => task_definition_matches(&definition, exe, session_dir),
        None => false,
    }
}

/// 任务定义里的 exe 与会话目录是否都与当前进程一致。
///
/// 单独拆出来是为了能直接喂一段 XML 做单测，不必真的去查系统任务。
fn task_definition_matches(definition: &str, exe: &Path, session_dir: &Path) -> bool {
    definition_mentions_exe(definition, exe) && definition_mentions_dir(definition, session_dir)
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
/// 所以直接做子串匹配即可。大小写和斜杠方向按 Windows 习惯归一化后再比：
/// 用户可能从资源管理器复制过带正斜杠的路径，schtasks 也会原样回读。
fn definition_mentions_dir(definition: &str, session_dir: &Path) -> bool {
    let needle = session_dir.to_string_lossy();
    if needle.trim().is_empty() {
        return false;
    }
    normalized(definition).contains(&normalized(&needle))
}

/// 任务定义 XML 里是否出现了当前可执行文件。
///
/// `<Command>` 里可能是裸路径，也可能是整条带引号的命令行（`/TR` 的值），
/// 两种都只是「路径出现在文本里」，做归一化子串匹配即可。
fn definition_mentions_exe(definition: &str, exe: &Path) -> bool {
    let needle = exe.to_string_lossy();
    if needle.trim().is_empty() {
        return false;
    }
    normalized(definition).contains(&normalized(&needle))
}

/// 路径比较前的归一化：Windows 盘符/路径大小写不敏感，反斜杠与正斜杠等价。
fn normalized(text: &str) -> String {
    text.replace('\\', "/").to_lowercase()
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
    // cmd.exe 默认用系统 OEM 代码页读 .cmd；现场路径里有中文（D:\工作文件\...）
    // 时会被读成乱码，导致 start/schtasks 找不到 exe，而批处理仍可能返回 0。
    // 先切到 UTF-8 再执行后面的命令（Rust 写出的批处理正文也是 UTF-8）。
    let batch = format!(
        "@echo off\r\nchcp 65001 >nul\r\n{}\r\necho %ERRORLEVEL% > \"{}\"\r\n",
        body,
        exit_path.display()
    );
    fs::write(&batch_path, batch)
        .map_err(|e| format!("写入提权脚本失败 {}: {}", batch_path.display(), e))?;

    // 用 PowerShell 的 RunAs 触发 UAC。-- 参数用单引号包住，避免路径里的空格被拆开。
    //
    // 这里**不能加 -Wait**：PowerShell 5.1 的 Start-Process -Wait 会等整个进程树，
    // 而批处理里的 start 拉起的 worker 正是 cmd.exe 的子进程。现场 11:28 的日志里
    // worker 11:28:02 就起来了，主进程却到 11:28:14（worker 退出后）才执行到
    // “worker 启动请求已返回”，随后才启动心跳；worker 在 11s 时判定主进程已死，
    // 把刚连上的隧道收掉。去掉 -Wait，改为轮询 elevate.exit。
    let ps = format!(
        "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','\"{}\"' -Verb RunAs -WindowStyle Hidden",
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

    // Start-Process 不再 -Wait，所以这里要自己等批处理把退出码写下来。
    // 注意：只等 elevate.exit，不等 worker。批处理执行完 start 就会写这个文件。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !exit_path.exists() {
        if std::time::Instant::now() >= deadline {
            return Err(
                "提权脚本 120s 内没有返回结果（可能被 UAC 拦截，或批处理没有执行）".to_string(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let code = fs::read_to_string(&exit_path)
        .map_err(|_| "提权脚本没有返回结果（可能被 UAC 拦截）".to_string())?;
    let code = code.trim().to_string();
    // 故意保留 create-task.cmd / run-worker.cmd / elevate.exit：
    // 现场只发回会话目录时，这几份文件能证明提权脚本到底执行了什么、
    // 返回码是多少。它们不含密码，收集脚本会一并打包。

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
/// 直接执行的话批处理会一直卡到 worker 退出（整个会话）为止。
pub fn worker_launch_command(exe: &Path, session_dir: &Path) -> String {
    format!(
        "start \"\" \"{}\" --vpn-worker \"{}\"",
        exe.display(),
        session_dir.display()
    )
}

/// 生成「注册任务 + 立刻拉起 worker」的命令：一次 UAC 把两件事都办完。
///
/// 用在「任务不存在」或「任务指向旧程序/旧目录」的那次连接上：既然这次必然
/// 要过一次 UAC（换新包、换了目录、或上次注册没做成），就把免 UAC 任务顺手
/// 注册好，**下一次连接开始就不用再点任何东西**。
///
/// 注册失败（组策略禁止建任务等）不能拖累本次连接：两条命令用 `&` 串，不是
/// `&&`；注册脚本的完整输出写进会话目录的 `create-task.log`，现场能直接看。
pub fn task_create_and_launch_command(exe: &Path, session_dir: &Path) -> String {
    format!(
        "{} > \"{}\" 2>&1 & {}",
        task_create_command(exe, session_dir),
        session_dir.join("create-task.log").display(),
        worker_launch_command(exe, session_dir)
    )
}

/// 注册（或刷新）Windows 计划任务。返回 Ok(true) 表示任务可用。
pub fn ensure_worker_task(exe: &Path, session: &VpnSession) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let body = task_create_command(exe, &session.dir);
        run_elevated_batch(session, "create-task", &body)?;
        if !worker_task_exists() {
            return Ok(false);
        }
        // schtasks 返回成功后系统任务不一定已经可读；给一点重试时间。
        // 只检查「任务存在」会让注册按钮报成功、状态却一直是未注册。
        for _ in 0..5 {
            if worker_task_matches(exe, &session.dir, None) {
                return Ok(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Ok(false)
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

/// 删除计划任务的提权版本。
///
/// 任务是用 `/RL HIGHEST` 注册的，普通权限下 `schtasks /Delete` 可能返回
/// “拒绝访问”，而 UI 之前没有捕获异常，表现就是点“移除任务”毫无反应。
/// 直接删除失败时再走一次 UAC，把删除动作放进提权批处理里。
#[cfg(target_os = "windows")]
pub fn delete_worker_task_elevated(session: &VpnSession) -> Result<(), String> {
    let body = format!("schtasks /Delete /TN \"{}\" /F", TASK_NAME);
    run_elevated_batch(session, "delete-task", &body)
}

#[cfg(not(target_os = "windows"))]
pub fn delete_worker_task_elevated(_session: &VpnSession) -> Result<(), String> {
    Ok(())
}

/// 等待 worker 在会话目录里写下启动日志。
///
/// 只看「schtasks /Run 返回 0」不够：任务动作可能因为路径/权限/杀软直接失败，
/// 而 schtasks 仍然返回成功。现场 11:02 的日志就是只有主进程心跳、没有 worker
/// 启动行；必须等到日志里真的出现 worker 启动行，才能把「提权成功」当成成功。
///
/// 不能只看文件变长：本函数前后的 `[elevate]` 日志也会让文件变长。只认
/// `[worker] 启动` 这个 worker 入口的第一行。
#[cfg(target_os = "windows")]
fn wait_for_worker_log(session: &VpnSession, before: u64, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        // 主进程的心跳线程要等 start_worker() 返回后才启动；这里等待 worker
        // 启动时不能让 worker 误以为主进程已经死了。等待期间替心跳续命。
        let _ = session.touch_alive();
        if let Ok(bytes) = fs::read(session.log_path()) {
            if bytes.len() as u64 > before {
                let start = (before as usize).min(bytes.len());
                let tail = &bytes[start..];
                if String::from_utf8_lossy(tail).contains("[worker] 启动") {
                    return true;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// 启动 worker。
///
/// `_recorded_task_dir` 是上次注册计划任务时用的会话目录（来自 store），
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
    if !exe.is_file() {
        session.append_log(&format!("[elevate] worker 程序不存在: {}", exe.display()));
        return Err(format!("worker 程序不存在: {}", exe.display()));
    }
    // start_worker() 返回前主进程的心跳线程还没启动；先写一次心跳，
    // 避免 worker 刚启动就看到「10s 没有心跳」而自我收尾。
    let _ = session.touch_alive();

    #[cfg(target_os = "windows")]
    {
        // 除了「任务存在」，还得确认任务指向的就是当前程序和当前会话目录：
        // 否则 /Run 起来的是旧版本 exe（或往旧目录写的 worker），主进程等不到
        // 任何状态，甚至会被旧 worker 用旧逻辑把刚建好的隧道收掉。
        let task_exists = worker_task_exists();
        if !task_exists {
            session.append_log(
                "[elevate] 还没有免 UAC 计划任务：首次连接会弹一次 UAC，\
                 并在同一次里自动注册好任务，之后连接不再弹框",
            );
        }
        if task_exists {
            let matches = worker_task_matches(exe, &session.dir, recorded_task_dir);
            session.append_log(&format!(
                "[elevate] 计划任务存在，指向当前程序/目录: {}",
                matches
            ));
            if matches {
                session.append_log("[elevate] 执行 schtasks /Run 触发计划任务");
                let before = fs::metadata(session.log_path())
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                let (text, ok) = crate::process::run_with_timeout(
                    Path::new("schtasks.exe"),
                    &["/Run", "/TN", TASK_NAME],
                    std::time::Duration::from_secs(30),
                );
                session.append_log(&format!(
                    "[elevate] schtasks /Run 返回：ok={}，{}",
                    ok,
                    text.trim()
                ));
                if ok {
                    if wait_for_worker_log(session, before, std::time::Duration::from_secs(15)) {
                        return Ok("计划任务".to_string());
                    }
                    session.append_log(
                        "[elevate] 计划任务已触发，但 15s 内没有 worker 启动日志；改用 UAC 提权",
                    );
                } else {
                    session.append_log(&format!(
                        "[elevate] 计划任务启动失败，改用 UAC 提权: {}",
                        text.trim()
                    ));
                }
            } else {
                session.append_log(
                    "[elevate] 计划任务指向的程序或会话目录已过期：本次用 UAC 提权启动，\
                     并在同一次里把任务刷新到当前版本（新包只需要这一次 UAC）",
                );
                if let Some(definition) = worker_task_definition() {
                    let preview: String = definition.chars().take(1200).collect();
                    session.append_log(&format!(
                        "[elevate] 当前任务定义（前 1200 字）: {}",
                        preview
                    ));
                }
            }
        }

        // 退回：提权拉起 worker，并在同一次提权里把免 UAC 任务注册好。
        // 「只在第一次连接点一次 UAC」就是靠这里：换新包、换了目录、上次没注册
        // 成功，都只会在这一次提权里顺带修好，下一次连接直接走 schtasks /Run。
        let body = task_create_and_launch_command(exe, &session.dir);
        session.append_log(
            "[elevate] 改用 UAC 提权启动 worker，并在同一次提权里自动注册免 UAC 任务\
             （注册失败不影响本次连接，详见会话目录里的 create-task.log）",
        );
        let before = fs::metadata(session.log_path())
            .map(|meta| meta.len())
            .unwrap_or(0);
        run_elevated_batch(session, "create-task-and-run", &body)?;
        if wait_for_worker_log(session, before, std::time::Duration::from_secs(15)) {
            // 注册是否成功不影响本次连接的结果，只是决定下次还弹不弹
            if worker_task_matches(exe, &session.dir, recorded_task_dir) {
                Ok("UAC 提权（已自动注册免 UAC 任务）".to_string())
            } else {
                session.append_log(
                    "[elevate] 本次连接已启动，但免 UAC 任务没有注册成功（可能被组策略拦截）；\
                     下次连接仍会弹一次 UAC",
                );
                Ok("UAC 提权".to_string())
            }
        } else {
            session.append_log("[elevate] UAC 提权命令已执行，但 15s 内没有 worker 启动日志");
            Err("提权命令已执行，但 worker 未启动（15s 内无日志）；请查看 worker.log，并尝试以管理员身份运行程序".to_string())
        }
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
        assert_eq!(TASK_NAME, "WsWorkbenchVpnWorker");
    }

    #[test]
    fn task_create_command_escapes_embedded_quotes() {
        let command = task_create_command(
            Path::new(r"C:\Program Files\ws-workbench\ws-workbench.exe"),
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn"),
        );
        // /TR 的值必须用 \" 包住带空格的路径，schtasks 才认得
        assert!(command.contains(r#"/TR "\"C:\Program Files\ws-workbench\ws-workbench.exe\" --vpn-worker \"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn\"""#));
        // 免 UAC 的关键参数
        assert!(command.contains("/RL HIGHEST"));
        assert!(command.contains("/F"));
    }

    #[test]
    fn worker_launch_command_detaches_from_batch() {
        let command = worker_launch_command(
            Path::new(r"C:\app\ws-workbench.exe"),
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn"),
        );
        // 必须以 start 开头：否则批处理会一直等 worker 退出（整整一个会话）
        assert!(command.starts_with(r#"start "" "#));
        assert!(command.contains(r#"--vpn-worker "C:\Users\demo"#));
    }

    #[test]
    fn create_and_launch_registers_first_then_starts_unconditionally() {
        let command = task_create_and_launch_command(
            Path::new(r"C:\Users\demo\Desktop\ws-workbench\ws-workbench.exe"),
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn"),
        );
        // 先建任务（/RL HIGHEST + /F），再无条件 start：
        // 用 `&` 而不是 `&&`，注册被组策略拦了也要把本次 worker 拉起来
        let register_at = command.find("schtasks /Create").expect("要先注册任务");
        let start_at = command.find(r#"start "" "#).expect("要拉起 worker");
        assert!(register_at < start_at, "注册必须在 start 之前: {command}");
        assert!(command.contains("/RL HIGHEST"));
        assert!(
            !command.contains("&&"),
            "注册失败不能连累本次连接: {command}"
        );
        // 注册日志落在会话目录，现场能直接看到 schtasks 的原始报错
        assert!(command.contains("create-task.log"));
        assert!(command.contains(r#"--vpn-worker "C:\Users\demo"#));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn waiting_for_worker_requires_the_worker_start_line() {
        let dir = std::env::temp_dir().join("ws-elevate-wait-test");
        let _ = std::fs::remove_dir_all(&dir);
        let session = VpnSession::new(dir.clone());
        session.ensure().unwrap();
        let before = std::fs::metadata(session.log_path())
            .map(|meta| meta.len())
            .unwrap_or(0);
        // 主进程自己的日志也会让文件变长，但这不是 worker 起来了。
        session.append_log("[elevate] 计划任务已触发");
        assert!(!wait_for_worker_log(
            &session,
            before,
            std::time::Duration::from_millis(100)
        ));
        session.append_log("[worker] 启动，pid 1234");
        assert!(wait_for_worker_log(
            &session,
            before,
            std::time::Duration::from_millis(100)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_definition_matches_only_the_baked_in_directory() {
        // 模拟 schtasks /Query /XML 的真实输出：路径原样出现，引号被转成 &quot;
        let definition = r#"<Command>"C:\app\ws-workbench.exe" --vpn-worker "C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn"</Command>"#;

        // 路径一致 → 可以复用免 UAC 任务
        assert!(definition_mentions_dir(
            definition,
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn")
        ));

        // 会话目录回退到别的盘/别的根 → 必须重新注册，不能复用旧任务
        assert!(!definition_mentions_dir(
            definition,
            Path::new(r"C:\Users\demo\AppData\Local\com.ws.workbench\vpn")
        ));
        assert!(!definition_mentions_dir(
            definition,
            Path::new(r"C:\Users\demo\AppData\Local\Temp\com.ws.workbench\vpn")
        ));
    }

    #[test]
    fn stale_task_pointing_at_another_exe_is_rejected() {
        // 现场：便携版与安装版共用同一个 AppData 会话目录，旧任务里写死的是
        // D:\ws-workbench\ws-workbench.exe，而主程序在 D:\软件\VPN\ws-workbench。
        // 只比对目录会把这种任务当成可用，拉起来的却是旧 worker。
        let old = Path::new(r"D:\ws-workbench\ws-workbench.exe");
        let current = Path::new(r"D:\软件\VPN\ws-workbench\ws-workbench.exe");
        let session = Path::new(r"C:\Users\user\AppData\Roaming\com.ws.workbench\vpn");
        let definition = r#"<Command>"D:\ws-workbench\ws-workbench.exe" --vpn-worker "C:\Users\user\AppData\Roaming\com.ws.workbench\vpn"</Command>"#;

        assert!(definition_mentions_exe(definition, old));
        assert!(!definition_mentions_exe(definition, current));
        // 目录一样、程序不一样 —— 这一条以前被误判成「任务可用」
        assert!(definition_mentions_dir(definition, session));
        assert!(!task_definition_matches(definition, current, session));
        // 程序、目录都对才能复用
        assert!(task_definition_matches(definition, old, session));
    }

    #[test]
    fn task_definition_path_comparison_ignores_case_and_slashes() {
        let definition = r#"<Command>"C:\APP\ws-workbench.exe" --vpn-worker "C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn"</Command>"#;
        assert!(task_definition_matches(
            definition,
            Path::new("c:/app/ws-workbench.exe"),
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn")
        ));
    }

    #[test]
    fn empty_directory_never_matches() {
        // 空路径是子串匹配的万能匹配，必须显式挡掉
        assert!(!definition_mentions_dir(
            "<Command>anything</Command>",
            Path::new("")
        ));
        assert!(!definition_mentions_exe(
            "<Command>anything</Command>",
            Path::new("")
        ));
    }

    #[test]
    fn recorded_directory_is_compared_case_insensitively() {
        // Windows 上盘符和大小写都不敏感，用户手改过配置也要能对上
        assert!(paths_equal(
            r"c:\users\demo\appdata\roaming\com.ws.workbench\vpn",
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn")
        ));
        // 尾部斜杠、正反斜杠混用也应视为同一目录
        assert!(paths_equal(
            "C:/Users/demo/AppData/Roaming/com.ws.workbench/vpn/",
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn")
        ));
        // 空记录不能当成匹配，否则会误判成「任务可用」而不去重注册
        assert!(!paths_equal(
            "",
            Path::new(r"C:\Users\demo\AppData\Roaming\com.ws.workbench\vpn")
        ));
    }

    #[test]
    fn unreadable_task_definition_is_not_trusted() {
        // 本机没有注册任务时 schtasks 查询会失败。即使 recorded 目录一致，
        // 也不能据此复用：否则会拉起一个无法确认版本的 worker。
        // 现场 11:54 的 1026 主程序 + 1106 worker 就是这样来的。
        let exe = Path::new(r"C:\app\ws-workbench.exe");
        let current = Path::new(r"C:\Users\demo\AppData\Local\com.ws.workbench\vpn");
        assert!(!worker_task_matches(
            exe,
            current,
            Some(r"C:\Users\demo\AppData\Local\com.ws.workbench\vpn")
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn task_xml_decoder_handles_utf16le_with_bom() {
        let text = r#"<Command>"D:\工作文件\1106\ws-workbench.exe"</Command>"#;
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_task_xml(&bytes), text);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn task_xml_decoder_handles_utf16le_without_bom() {
        let text = r#"<Command>"D:\工作文件\1106\ws-workbench.exe"</Command>"#;
        let mut bytes = Vec::new();
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_task_xml(&bytes), text);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
