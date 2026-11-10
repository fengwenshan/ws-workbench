//! 系统通知：连接生效、规则热更新生效、代理模式切换生效时提醒一下。
//!
//! 不引第三方通知库，直接用系统自带能力：
//!
//! - Windows：PowerShell 调 WinRT 的 `ToastNotificationManager` 发 toast；
//! - macOS：`osascript -e 'display notification ...'`。
//!
//! Windows 上这条路**不是百分百能用**：Win11 24H2/25H2 起，便携 exe（没有 MSIX
//! 包身份、也没有开始菜单里的同 AUMID 快捷方式）调 `CreateToastNotifier` 会直接
//! 报「该进程没有程序包标识符」(0x80073D54)——第三方库 notify-rust 报的是同一个
//! 错，亲测。托盘气泡（`NotifyIcon`）这条路在同一台机器上不弹也不报错（通知历史
//! 里查不到记录），所以指望不上。
//!
//! 于是这里只负责「尽力发系统通知」，发不出去必须**如实返回错误**：调用方据此
//! 退回应用内提示，用户总能看到连接结果。

use std::path::Path;
use std::time::Duration;

/// 通知内容上限：提示语都是短句，万一 profile 名很长，截断比让脚本处理大字符串稳
const MAX_TEXT_LEN: usize = 120;

/// AUMID（应用用户模型 ID）：Windows 通知里显示的应用标识
const AUMID: &str = "WsWorkbench";

/// 触发通知的几个时机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// 连接生效
    Connected,
    /// 断开生效
    Disconnected,
    /// 已连接状态下运行期切换了代理模式
    ModeSwitched,
}

/// 判定只需要这两个字段；单独抽出来是为了让判定成为纯函数、可测。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub phase: String,
    pub applied_mode: String,
}

impl Snapshot {
    pub fn new(phase: &str, applied_mode: &str) -> Snapshot {
        Snapshot {
            phase: phase.to_string(),
            applied_mode: applied_mode.to_string(),
        }
    }
}

/// 对比前后两次状态，返回这次该发哪些通知。
///
/// - 「已连接」只在**阶段发生跃迁**时报一次：状态文件里 phase 会在每次写盘时重复
///   出现，按内容比较才不会连环弹；
/// - 「已断开」只认「本来连着」这个前提，`disconnecting` 与随后的 `disconnected` 算
///   同一次断开，不会连弹两条；
/// - 模式切换只在「本来就连着」的时候报：连上那一拍 worker 会把 `applied_mode`
///   一起写进去，那不是用户切模式，不该再补一条通知。
pub fn triggers(prev: &Snapshot, next: &Snapshot) -> Vec<Trigger> {
    let mut out = Vec::new();
    let was_connected = prev.phase == "connected";
    let is_connected = next.phase == "connected";
    if is_connected && !was_connected {
        out.push(Trigger::Connected);
    }
    if was_connected && (next.phase == "disconnected" || next.phase == "disconnecting") {
        out.push(Trigger::Disconnected);
    }
    if was_connected
        && is_connected
        && !prev.applied_mode.is_empty()
        && prev.applied_mode != next.applied_mode
    {
        out.push(Trigger::ModeSwitched);
    }
    out
}

/// 发一条系统通知。
///
/// 失败只返回错误文本，由调用方决定要不要记录：通知是锦上添花，
/// 任何情况下都不能影响连接、分流这些主流程。
pub fn notify(title: &str, body: &str) -> Result<(), String> {
    let title = clip(title);
    let body = clip(body);
    if title.is_empty() && body.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe().ok();
        let script = toast_script(&title, &body, exe.as_deref().and_then(|p| p.to_str()));
        let (text, ok) = crate::process::run_with_timeout(
            Path::new(&powershell_path()),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encoded_command(&script),
            ],
            Duration::from_secs(15),
        );
        if ok {
            Ok(())
        } else {
            Err(text.trim().to_string())
        }
    }

    #[cfg(target_os = "macos")]
    {
        let script = applescript(&title, &body);
        let (text, ok) = crate::process::run_with_timeout(
            Path::new("/usr/bin/osascript"),
            &["-e", &script],
            Duration::from_secs(10),
        );
        if ok {
            Ok(())
        } else {
            Err(text.trim().to_string())
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (title, body);
        Ok(())
    }
}

/// 系统自带的 Windows PowerShell 路径（固定 5.1，不依赖 PATH）
#[cfg(target_os = "windows")]
fn powershell_path() -> String {
    std::env::var("SystemRoot")
        .map(|root| format!(r"{}\System32\WindowsPowerShell\v1.0\powershell.exe", root))
        .unwrap_or_else(|_| "powershell.exe".to_string())
}

/// 把脚本编码成 PowerShell 的 `-EncodedCommand` 参数（UTF-16LE + base64）。
///
/// 不用 `-Command` 传脚本：toast 的 XML 属性必须带双引号，而这个脚本要穿过
/// 「Rust 拼命令行 → Win32 解析 → PowerShell 再解析」三层，转义稍不留神就变形
/// （实测 `-Command` 下 `template="ToastGeneric"` 的引号会被吃掉，LoadXml 直接报
/// `0xC00CE502`）。编码方式零转义，顺带也不受控制台代码页影响，中文不会变问号。
pub fn encoded_command(script: &str) -> String {
    use base64::Engine as _;
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 压平成一行并截断：通知里换行会被系统折行，控制字符还可能弄坏脚本。
fn clip(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= MAX_TEXT_LEN {
        return flat.to_string();
    }
    let mut out: String = flat.chars().take(MAX_TEXT_LEN).collect();
    out.push('…');
    out
}

/// XML 文本转义。`'` 也转：整段 XML 是塞在 PowerShell 单引号字符串里的。
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// AppleScript 双引号字符串转义
fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Windows 通知脚本：先按当前用户注册 AUMID，再调 WinRT 发一条 toast。
///
/// 整段脚本在**非交互** PowerShell 里跑（`-NonInteractive -EncodedCommand`），
/// 所以 `$ErrorActionPreference = 'Stop'` 必须写在最前面：默认的 `Continue` 会把
/// `Show` 的失败只打进 stderr 然后照样以 0 退出，调用方就会误判「已经弹出去了」。
///
/// 注册 AUMID（`HKCU\Software\Classes\AppUserModelId\...`）是免管理员、可反复写的：
/// 不注册的话，系统设置里根本看不到这个应用，通知也容易被静默丢掉（现场就踩到
/// 「退出码 0 但什么都没弹」）。注册失败不算致命 —— 用 `try` 包住，别拖累发通知。
pub fn toast_script(title: &str, body: &str, exe: Option<&str>) -> String {
    let icon = exe
        .map(|path| path.replace('\'', "''"))
        .unwrap_or_default();
    format!(
        "$ErrorActionPreference = 'Stop'; \
         try {{ \
             $key = 'HKCU:\\Software\\Classes\\AppUserModelId\\{aumid}'; \
             New-Item -Path $key -Force | Out-Null; \
             New-ItemProperty -Path $key -Name DisplayName -Value 'ws-workbench' -PropertyType String -Force | Out-Null; \
             New-ItemProperty -Path $key -Name IconUri -Value '{icon}' -PropertyType String -Force | Out-Null; \
             New-ItemProperty -Path $key -Name ShowInSettings -Value 1 -PropertyType DWord -Force | Out-Null \
         }} catch {{ }}; \
         [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
         [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom, ContentType = WindowsRuntime] | Out-Null; \
         $xml = New-Object Windows.Data.Xml.Dom.XmlDocument; \
         $xml.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>'); \
         $toast = New-Object Windows.UI.Notifications.ToastNotification $xml; \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{aumid}').Show($toast)",
        xml_escape(title),
        xml_escape(body),
        aumid = AUMID,
    )
}

/// macOS 通知脚本
///
/// 走 `osascript` 而不是 `UNUserNotificationCenter`：后者要求 app bundle 里带签名和
/// 使用授权（`requestAuthorization`），还要处理回调，为一个「连上了」提示不值当。
/// 加 `sound name "default"`：macOS 的通知中心默认只闪一下，有声音才不容易漏。
pub fn applescript(title: &str, body: &str) -> String {
    format!(
        "display notification \"{}\" with title \"{}\" sound name \"default\"",
        applescript_escape(body),
        applescript_escape(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_flattens_newlines_and_truncates() {
        assert_eq!(clip("已连接\nVPN\t"), "已连接 VPN");
        let long = "x".repeat(MAX_TEXT_LEN + 10);
        let clipped = clip(&long);
        assert_eq!(clipped.chars().count(), MAX_TEXT_LEN + 1, "超长要截断并加省略号");
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn toast_script_escapes_xml_and_registers_aumid() {
        let script = toast_script("标题 & <测试>", "it's \"quoted\"", Some(r"C:\app\ws.exe"));
        assert!(script.contains("标题 &amp; &lt;测试&gt;"));
        assert!(script.contains("it&#39;s &quot;quoted&quot;"));
        assert!(script.contains("CreateToastNotifier('WsWorkbench')"));
        assert!(script.contains("ToastGeneric"));
        // 注册 AUMID 是免管理员、可反复写的；不注册系统设置里看不到这个应用
        assert!(script.contains("AppUserModelId\\WsWorkbench"));
        assert!(script.contains(r"IconUri -Value 'C:\app\ws.exe'"));
        // 单引号字符串里不能再出现裸单引号，否则 PowerShell 会把脚本截断
        let xml_part = script.split("LoadXml('").nth(1).unwrap();
        let value = xml_part.split("')").next().unwrap();
        assert!(!value.contains('\''), "XML 文本里的单引号必须转义: {value}");
    }

    #[test]
    fn toast_script_reports_failure_instead_of_exiting_zero() {
        let script = toast_script("标题", "正文", None);
        // 默认的 Continue 会把 Show 的失败吞掉、照样退出 0，调用方就以为弹出来了
        assert!(script.starts_with("$ErrorActionPreference = 'Stop'"));
        // 只有注册 AUMID 那段可以 catch（失败了也要照常发通知），
        // 发 toast 之后绝不能再 catch 掉错误
        let last_catch = script.rfind("catch").expect("注册段要 catch");
        let show_at = script.find("CreateToastNotifier").expect("要发 toast");
        assert!(
            last_catch < show_at,
            "CreateToastNotifier 之后不能再吞错误，否则退出码 0 会被当成成功"
        );
    }

    #[test]
    fn applescript_escapes_quotes_and_backslashes() {
        let script = applescript("标题", "C:\\path \"x\"");
        assert!(script.starts_with("display notification"));
        assert!(script.contains("C:\\\\path \\\"x\\\""));
        assert!(script.contains("with title \"标题\""));
    }

    #[test]
    fn encoded_command_round_trips_to_utf16() {
        use base64::Engine as _;
        let script = toast_script("标题", "正文", None);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded_command(&script))
            .expect("必须是合法 base64");
        let (pairs, rest) = decoded.as_chunks::<2>();
        assert!(rest.is_empty(), "UTF-16 字节数必须是偶数");
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), script);
    }

    #[test]
    fn only_the_transition_into_connected_notifies() {
        let idle = Snapshot::new("disconnected", "");
        let connecting = Snapshot::new("connecting", "");
        let connected = Snapshot::new("connected", "rule");
        assert_eq!(triggers(&idle, &connecting), vec![]);
        assert_eq!(triggers(&connecting, &connected), vec![Trigger::Connected]);
        // 状态文件重复写同样的内容：不再弹
        assert_eq!(triggers(&connected, &connected), vec![]);
        // 断开重连能再弹一次
        assert_eq!(
            triggers(&Snapshot::new("failed", "rule"), &connected),
            vec![Trigger::Connected]
        );
    }

    #[test]
    fn mode_switch_only_counts_while_already_connected() {
        let connected = Snapshot::new("connected", "rule");
        let global = Snapshot::new("connected", "global");
        assert_eq!(triggers(&connected, &global), vec![Trigger::ModeSwitched]);
        // 连上那一拍 applied_mode 一起写入，不能当成「用户切了模式」
        assert_eq!(
            triggers(
                &Snapshot::new("connecting", "rule"),
                &Snapshot::new("connected", "global")
            ),
            vec![Trigger::Connected]
        );
        // 还不知道旧模式（空串）时不报，避免启动时凭空弹一条
        assert_eq!(
            triggers(
                &Snapshot::new("connected", ""),
                &Snapshot::new("connected", "direct")
            ),
            vec![]
        );
    }

    #[test]
    fn disconnect_notifies_once_and_only_after_being_connected() {
        let connected = Snapshot::new("connected", "rule");
        // disconnecting → disconnected 是同一次断开，只报一次
        assert_eq!(
            triggers(&connected, &Snapshot::new("disconnecting", "rule")),
            vec![Trigger::Disconnected]
        );
        assert_eq!(
            triggers(&Snapshot::new("disconnecting", "rule"), &Snapshot::new("disconnected", "")),
            vec![]
        );
        // 轮询恰好跳过 disconnecting：直接从 connected 落到 disconnected 也要报
        assert_eq!(
            triggers(&connected, &Snapshot::new("disconnected", "")),
            vec![Trigger::Disconnected]
        );
        // 本来就没连上（启动、失败、连接没成）不该报「已断开」
        assert_eq!(
            triggers(&Snapshot::new("failed", "rule"), &Snapshot::new("disconnected", "")),
            vec![]
        );
        assert_eq!(
            triggers(&Snapshot::new("", ""), &Snapshot::new("disconnected", "")),
            vec![]
        );
    }
}
