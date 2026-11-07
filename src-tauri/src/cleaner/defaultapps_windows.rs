//! Windows 侧默认应用：只读展示 + 「打开方式」/「系统设置」引导（设计 4.4）。
//!
//! 为什么只读：Win8+ 起 `HKCU\...\Explorer\FileExts\<ext>\UserChoice` 的 ProgId 带
//! 系统哈希签名，第三方写入会被系统校验后重置（设计「非目标」明确不做），所以
//! 这里只读注册表；修改交给系统的「打开方式」对话框（勾选「始终使用」）或
//! `ms-settings:defaultapps`。
//!
//! 本文件分两层（与 `installer_windows.rs` 一致）：
//! - **与 winreg 无关的纯逻辑**：命令行 → exe、名称优先级、详情文案，macOS 上可单测；
//! - **读注册表 / 起系统对话框**：`#[cfg(target_os = "windows")]` 隔离。

use super::{display_extension, normalize_extension, DefaultAppEntry, SUPPORTED_EXTENSIONS};

/// 一个文件类型的当前关联（从 `UserChoice` 与 HKCR 解析出来的原始数据）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Association {
    /// `UserChoice\ProgId`；没有记录时空串
    pub prog_id: String,
    /// `HKCR\<ProgId>\shell\open\command` 默认值
    pub open_command: String,
    /// 从打开命令里解析出来的程序路径
    pub command_app: Option<String>,
    /// `HKCR\Applications\<exe>` 默认值：注册应用时写的友好名（如 "Visual Studio Code"）
    pub application_name: String,
    /// `HKCR\<ProgId>` 默认值：文件类型描述（如 "VLC media file"）
    pub file_type_name: String,
}

impl Association {
    /// 展示用的应用名：优先注册应用名 → exe 文件名 → ProgId
    pub fn app_name(&self) -> String {
        pick_app_name(
            &self.application_name,
            self.command_app.as_deref(),
            &self.prog_id,
        )
    }

    /// 列表里的备注文案
    pub fn detail(&self) -> String {
        windows_detail(&self.prog_id, &self.open_command)
    }
}

// ============================== 纯逻辑（可跨平台单测） ==============================

/// 从 `shell\open\command` 里取出程序路径（只取第一个 token，不启动）。
///
/// Windows 生成关联命令时程序路径总是带引号（`"C:\Program Files\Foo\foo.exe" "%1"`），
/// 所以引号优先；不带引号时按首个空白切分（路径含空格的裸写法本身有歧义，
/// 系统也不会生成这种值）。这里只做「取 exe」，不需要完整还原
/// `CommandLineToArgvW` 的参数语义。
pub fn exe_from_open_command(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        let program = rest[..end].trim();
        return (!program.is_empty()).then(|| program.to_string());
    }
    let program = trimmed.split_whitespace().next()?.trim();
    (!program.is_empty()).then(|| program.to_string())
}

/// 程序路径 → 文件名（`C:\Program Files\Foo\Code.exe` → `Code.exe`）
pub fn exe_file_name(program: &str) -> Option<String> {
    let trimmed = program.trim().trim_matches('"');
    let file_name = trimmed.rsplit(['\\', '/']).next()?.trim();
    (!file_name.is_empty()).then(|| file_name.to_string())
}

/// 程序路径或文件名 → 去掉 `.exe` 后的展示名（`C:\...\Code.exe` → `Code`）
fn exe_stem(program: &str) -> String {
    let Some(file_name) = exe_file_name(program) else {
        return String::new();
    };
    let lowered = file_name.to_ascii_lowercase();
    let stem = match lowered.strip_suffix(".exe") {
        Some(_) => &file_name[..file_name.len() - 4],
        None => file_name.as_str(),
    };
    stem.trim().to_string()
}

/// `Applications\code.exe` 这类 ProgId 直接就是可执行文件名
pub fn exe_from_applications_prog_id(prog_id: &str) -> Option<String> {
    const PREFIX: &str = r"Applications\";
    let trimmed = prog_id.trim();
    let prefix = trimmed.get(..PREFIX.len())?;
    if !prefix.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let rest = trimmed[PREFIX.len()..].trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// 应用名的优先级：注册应用名 → exe 文件名（去 `.exe`）→ ProgId → 「未设置」。
pub fn pick_app_name(application_name: &str, program: Option<&str>, prog_id: &str) -> String {
    let application_name = application_name.trim();
    if !application_name.is_empty() {
        return application_name.to_string();
    }
    if let Some(program) = program {
        let stem = exe_stem(program);
        if !stem.is_empty() {
            return stem;
        }
    }
    let prog_id = prog_id.trim();
    if prog_id.is_empty() {
        "未设置".to_string()
    } else {
        prog_id.to_string()
    }
}

/// 列表备注：Windows 为什么只能只读、该去哪里改
pub fn windows_detail(prog_id: &str, open_command: &str) -> String {
    let prog_id = prog_id.trim();
    if prog_id.is_empty() {
        return "Windows 没有该类型的默认应用记录；点「选择默认应用」在系统对话框里勾选 \
                「始终使用」即可完成设置。"
            .to_string();
    }
    let mut detail = format!("Windows 只读展示（ProgId: {}）", prog_id);
    let open_command = open_command.trim();
    if !open_command.is_empty() {
        detail.push_str(&format!("；打开命令: {}", open_command));
    }
    detail.push_str(
        "。Windows 不允许第三方静默修改默认应用，请在「打开方式」对话框或系统设置里确认。",
    );
    detail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_quoted_exe_from_open_command() {
        assert_eq!(
            exe_from_open_command(r#""C:\Program Files\Foo\foo.exe" "%1""#).as_deref(),
            Some(r"C:\Program Files\Foo\foo.exe")
        );
        // 没有参数
        assert_eq!(
            exe_from_open_command(r#""C:\Program Files\Foo\bar.exe""#).as_deref(),
            Some(r"C:\Program Files\Foo\bar.exe")
        );
        // 不带引号：按首个空白切
        assert_eq!(
            exe_from_open_command(r"C:\Windows\notepad.exe %1").as_deref(),
            Some(r"C:\Windows\notepad.exe")
        );
        // 只取第一个 token，后面的参数不参与
        assert_eq!(
            exe_from_open_command(r#"rundll32.exe shell32.dll,OpenAs_RunDLL x.pdf"#).as_deref(),
            Some("rundll32.exe")
        );
        assert_eq!(exe_from_open_command(""), None);
        assert_eq!(exe_from_open_command("   "), None);
        // 引号不闭合视为无效
        assert_eq!(exe_from_open_command(r#""C:\Foo\foo.exe"#), None);
    }

    #[test]
    fn exe_file_name_takes_last_component() {
        assert_eq!(
            exe_file_name(r"C:\Program Files\Foo\Code.exe").as_deref(),
            Some("Code.exe")
        );
        assert_eq!(exe_file_name("/usr/bin/foo").as_deref(), Some("foo"));
        assert_eq!(exe_file_name("   "), None);
        assert_eq!(exe_file_name(r#""C:\Foo\bar.exe""#).as_deref(), Some("bar.exe"));
    }

    #[test]
    fn applications_prog_id_yields_exe() {
        assert_eq!(
            exe_from_applications_prog_id(r"Applications\code.exe").as_deref(),
            Some("code.exe")
        );
        assert_eq!(
            exe_from_applications_prog_id(r"applications\code.exe").as_deref(),
            Some("code.exe")
        );
        assert_eq!(exe_from_applications_prog_id("txtfile"), None);
        assert_eq!(exe_from_applications_prog_id(r"Applications\"), None);
    }

    #[test]
    fn app_name_prefers_registered_application_name() {
        assert_eq!(
            pick_app_name(
                "Visual Studio Code",
                Some(r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe"),
                "Applications\\code.exe"
            ),
            "Visual Studio Code"
        );
        // 没有注册应用名：退到 exe 文件名（去 .exe）
        assert_eq!(
            pick_app_name("", Some(r"C:\Program Files\Google\Chrome\chrome.exe"), "ChromeHTML"),
            "chrome"
        );
        // 只有 ProgId
        assert_eq!(pick_app_name("", None, "MSEdgePDF"), "MSEdgePDF");
        // 什么都没有
        assert_eq!(pick_app_name("", None, ""), "未设置");
    }

    #[test]
    fn detail_explains_windows_readonly_and_guides() {
        let detail = windows_detail("MSEdgePDF", r#""C:\Program Files (x86)\Microsoft\Edge\msedge.exe" "%1""#);
        assert!(detail.contains("ProgId: MSEdgePDF"));
        assert!(detail.contains("只读"));
        assert!(detail.contains("打开方式"));
        // 没有记录时给引导，而不是空文案
        let empty = windows_detail("", "");
        assert!(empty.contains("选择默认应用"));
        assert!(empty.contains("始终使用"));
    }

    #[test]
    fn association_name_and_detail_are_composed() {
        let association = Association {
            prog_id: "txtfile".to_string(),
            open_command: r#""C:\Windows\notepad.exe" "%1""#.to_string(),
            command_app: Some(r"C:\Windows\notepad.exe".to_string()),
            application_name: String::new(),
            file_type_name: "文本文档".to_string(),
        };
        assert_eq!(association.app_name(), "notepad");
        assert!(association.detail().contains("txtfile"));

        // 空关联：名字是「未设置」，备注是引导文案
        assert_eq!(Association::default().app_name(), "未设置");
        assert!(Association::default().detail().contains("选择默认应用"));
    }
}

// ============================== Windows 实现（注册表 / 系统对话框） ==============================

#[cfg(target_os = "windows")]
mod win_impl {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    use winreg::RegKey;

    use super::*;

    /// `UserChoice` 所在根
    const FILE_EXTS_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts";
    /// 临时探测文件目录（打开方式对话框按扩展名给候选应用，需要真实文件）
    const PROBE_DIR: &str = "ws-workbench-defaultapps";
    /// 探测文件保留时长：对话框可能开着很久，只在明显陈旧时清理
    const PROBE_STALE_AFTER: Duration = Duration::from_secs(60 * 60);

    fn read_default_value(key: &RegKey) -> String {
        key.get_value::<String, _>("").unwrap_or_default()
    }

    /// 读一个扩展名的当前关联；没有 `UserChoice` 记录时返回默认值
    fn read_association(extension: &str) -> Association {
        let prog_id = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(format!(
                r"{}\{}\UserChoice",
                FILE_EXTS_PATH,
                display_extension(extension)
            ))
            .ok()
            .and_then(|key| key.get_value::<String, _>("ProgId").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        if prog_id.is_empty() {
            return Association::default();
        }

        let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
        let file_type_name = hkcr
            .open_subkey(&prog_id)
            .ok()
            .map(|key| read_default_value(&key))
            .unwrap_or_default();

        let mut open_command = hkcr
            .open_subkey(format!(r"{}\shell\open\command", prog_id))
            .ok()
            .map(|key| read_default_value(&key))
            .unwrap_or_default();
        // `Applications\foo.exe` 这类 ProgId 的打开命令有时挂在 Applications 下
        if open_command.trim().is_empty() {
            if let Some(exe) = exe_from_applications_prog_id(&prog_id) {
                open_command = hkcr
                    .open_subkey(format!(r"Applications\{}\shell\open\command", exe))
                    .ok()
                    .map(|key| read_default_value(&key))
                    .unwrap_or_default();
            }
        }

        let command_app = exe_from_open_command(&open_command);
        let application_name = command_app
            .as_deref()
            .and_then(exe_file_name)
            .and_then(|file_name| {
                hkcr.open_subkey(format!(r"Applications\{}", file_name))
                    .ok()
                    .map(|key| read_default_value(&key))
            })
            .map(|value| value.trim().to_string())
            .unwrap_or_default();

        Association {
            prog_id,
            open_command,
            command_app,
            application_name,
            file_type_name,
        }
    }

    /// 列表主流程：读 14 个扩展名的 `UserChoice`
    pub fn list() -> Vec<DefaultAppEntry> {
        SUPPORTED_EXTENSIONS
            .iter()
            .map(|extension| {
                let association = read_association(extension);
                DefaultAppEntry {
                    extension: display_extension(extension),
                    current_app_name: association.app_name(),
                    current_app_id: association.prog_id.clone(),
                    // Windows 不能在本应用内改（哈希保护），前端只给引导按钮
                    can_change: false,
                    detail: association.detail(),
                }
            })
            .collect()
    }

    /// 打开系统「默认应用」设置（ms-settings 深链，Win10/11 可用）
    pub fn reveal_settings() -> Result<(), String> {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:defaultapps"])
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开系统默认应用设置失败: {}", error))
    }

    /// 用临时空文件调起系统「打开方式」对话框；用户勾选「始终使用」即完成设置
    pub fn open_with(extension: &str) -> Result<(), String> {
        let file = create_probe_file(extension)?;
        std::process::Command::new("rundll32.exe")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(&file)
            .spawn()
            .map_err(|error| {
                format!(
                    "打开「打开方式」对话框失败: {}（临时文件: {}）",
                    error,
                    file.display()
                )
            })?;
        Ok(())
    }

    /// 建一个带扩展名的临时空文件，并顺手清理一小时前的旧探测文件
    fn create_probe_file(extension: &str) -> Result<PathBuf, String> {
        let normalized = normalize_extension(extension);
        if normalized.is_empty() {
            return Err("扩展名不能为空".to_string());
        }
        let dir = std::env::temp_dir().join(PROBE_DIR);
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("创建临时目录失败: {}", error))?;
        cleanup_stale_probes(&dir);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let path = dir.join(format!(
            "probe-{}-{}.{}",
            std::process::id(),
            stamp,
            normalized
        ));
        std::fs::write(&path, b"").map_err(|error| format!("创建临时文件失败: {}", error))?;
        Ok(path)
    }

    fn cleanup_stale_probes(dir: &PathBuf) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let stale = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > PROBE_STALE_AFTER);
            if stale {
                // 对话框早已关闭才会过期；删不掉（被占用）就留着，下次再试
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_impl::{list, open_with, reveal_settings};
