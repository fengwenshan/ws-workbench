//! macOS 启动项：用户 / 系统 `LaunchAgents` + 登录项（`System Events`）。
//!
//! 设计 4.2：
//! - 用户 LaunchAgents：`~/Library/LaunchAgents/*.plist`，可启停；
//! - 系统 LaunchAgents：`/Library/LaunchAgents/*.plist`，需要 root → 只读；
//! - 启停用 `launchctl disable/enable gui/$UID/<label>` +
//!   `launchctl bootstrap/bootout gui/$UID <plist>`，只动用户域；
//! - 登录项用 `osascript` 走 System Events：读 `name/path`，关闭 = 删除登录项，
//!   重新打开 = 用 id 里带的路径重新添加（被移除后列表里就没有这一行了，
//!   正常操作路径只需要删除；重新添加保留给「id 还能对上」的调用）；
//! - 首次访问 System Events 会触发「自动化」授权，被拒绝时**不阻塞页面**：
//!   列表照常返回 LaunchAgents，另加一条只读提示项说明授权缺失。
//!
//! 纯解析逻辑（plist / launchctl 输出 / osascript 输出）放在文件上半部分，
//! 单测覆盖；真正跑 `launchctl` / `osascript` 的调用集中在下半部分。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{StartupItem, APP_AUTOSTART_NAME, LOGIN_ITEMS_UNAVAILABLE_ID};

const LAUNCH_AGENT_ID_PREFIX: &str = "mac:launch_agent:";
const LOGIN_ITEM_ID_PREFIX: &str = "mac:login_item:";
/// 登录项 id 里 name 与 path 的分隔符（不可见字符，文件名/路径不会出现）
const LOGIN_ITEM_PATH_SEP: char = '\u{1f}';
/// System Events 读取登录项的脚本（`name<TAB>path<LF>` 每行一项）
const LOGIN_ITEMS_SCRIPT: &str = r#"tell application "System Events"
	set output to ""
	repeat with loginItem in login items
		set output to output & (name of loginItem) & tab & (path of loginItem as text) & linefeed
	end repeat
	return output
end tell"#;

// ============================== 纯逻辑（可跨平台单测） ==============================

/// 从 plist 读出来的 LaunchAgent 信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAgentInfo {
    pub label: String,
    pub command: String,
    pub run_at_load: bool,
}

/// 一条登录项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginItemInfo {
    pub name: String,
    pub path: String,
}

/// 解析一个 LaunchAgent plist。
///
/// - `Label` 缺失时用文件名词干兜底（有些第三方 plist 写得不规范）；
/// - 命令优先用 `Program` + `ProgramArguments`；只有 `ProgramArguments` 时把
///   第一个参数当可执行文件。
pub fn parse_launch_agent(value: &plist::Value, fallback_label: &str) -> Option<LaunchAgentInfo> {
    let dict = value.as_dictionary()?;
    let label = dict
        .get("Label")
        .and_then(|value| value.as_string())
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_label.trim().to_string());
    if label.is_empty() {
        return None;
    }

    let program = dict
        .get("Program")
        .and_then(|value| value.as_string())
        .map(str::trim)
        .filter(|program| !program.is_empty())
        .map(str::to_string);
    let program_args: Vec<String> = dict
        .get("ProgramArguments")
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_string())
                .map(str::trim)
                .filter(|arg| !arg.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let (program, args) = match program {
        Some(program) => (program, program_args),
        None => {
            let mut iter = program_args.into_iter();
            let program = iter.next()?;
            (program, iter.collect::<Vec<_>>())
        }
    };

    Some(LaunchAgentInfo {
        label,
        command: join_command(&program, &args),
        run_at_load: dict
            .get("RunAtLoad")
            .and_then(|value| value.as_boolean())
            .unwrap_or(false),
    })
}

/// 拼一条给人看的命令行（含空格的参数补引号）
pub fn join_command(program: &str, args: &[String]) -> String {
    let mut parts = vec![quote_part(program)];
    parts.extend(args.iter().map(|arg| quote_part(arg)));
    parts.join(" ")
}

fn quote_part(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.contains(' ') || trimmed.contains('\t') {
        format!("\"{}\"", trimmed)
    } else {
        trimmed.to_string()
    }
}

/// 解析 `launchctl print-disabled gui/$UID` 的输出。
///
/// 输出形如：
/// ```text
/// disabled services = {
///     "com.foo.bar" => disabled
///     "com.foo.baz" => enabled
/// }
/// ```
/// 只把 `=> disabled` 的 label 收进集合；读不到/格式变了都退化成「没有禁用项」。
pub fn parse_disabled_services(output: &str) -> HashSet<String> {
    let mut disabled = HashSet::new();
    for line in output.lines() {
        let line = line.trim();
        let Some((left, right)) = line.rsplit_once("=>") else {
            continue;
        };
        if right.trim() != "disabled" {
            continue;
        }
        let name = left.trim().trim_matches('"').trim();
        if !name.is_empty() {
            disabled.insert(name.to_string());
        }
    }
    disabled
}

/// 解析 System Events 的输出（`name<TAB>path` 每行一项）。
pub fn parse_login_items(output: &str) -> Vec<LoginItemInfo> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                return None;
            }
            let (name, path) = match line.split_once('\t') {
                Some((name, path)) => (name.trim(), path.trim()),
                None => (line.trim(), ""),
            };
            if name.is_empty() {
                return None;
            }
            Some(LoginItemInfo {
                name: name.to_string(),
                path: path.to_string(),
            })
        })
        .collect()
}

/// 登录项 id：`mac:login_item:<name><US><path>`
///
/// 带上 path 是为了让「关闭后再打开」能重新 `make login item`；正常操作里
/// 关闭后该行会从列表消失，所以这主要是防御性设计。
pub fn login_item_id(name: &str, path: &str) -> String {
    format!(
        "{}{}{}{}",
        LOGIN_ITEM_ID_PREFIX, name, LOGIN_ITEM_PATH_SEP, path
    )
}

pub fn parse_login_item_id(id: &str) -> Option<LoginItemInfo> {
    let rest = id.strip_prefix(LOGIN_ITEM_ID_PREFIX)?;
    let (name, path) = match rest.split_once(LOGIN_ITEM_PATH_SEP) {
        Some((name, path)) => (name, path),
        None => (rest, ""),
    };
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(LoginItemInfo {
        name: name.to_string(),
        path: path.trim().to_string(),
    })
}

/// LaunchAgent 的 id：`mac:launch_agent:<scope>:<label>`
pub fn launch_agent_id(scope: &str, label: &str) -> String {
    format!("{}{}:{}", LAUNCH_AGENT_ID_PREFIX, scope, label)
}

fn parse_launch_agent_id(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix(LAUNCH_AGENT_ID_PREFIX)?;
    let (scope, label) = rest.split_once(':')?;
    let label = label.trim();
    if scope.is_empty() || label.is_empty() {
        return None;
    }
    Some((scope.to_string(), label.to_string()))
}

// ============================== 平台实现（launchctl / osascript） ==============================

/// 列出全部启动项：用户 LaunchAgents → 系统 LaunchAgents → 登录项。
///
/// 登录项读取失败只降级成一条提示项，不影响前面的 LaunchAgents。
pub fn list() -> Vec<StartupItem> {
    let disabled = read_disabled_services();
    let mut items = Vec::new();
    if let Some(home) = crate::cleaner::home_dir() {
        items.extend(read_launch_agents(
            &home.join("Library/LaunchAgents"),
            "user",
            &disabled,
        ));
    }
    items.extend(read_launch_agents(
        Path::new("/Library/LaunchAgents"),
        "machine",
        &disabled,
    ));

    match read_login_items() {
        Ok(login_items) => {
            for login in login_items {
                items.push(login_item_startup(&login));
            }
        }
        Err(error) => items.push(login_items_unavailable(&error)),
    }
    items
}

/// 用户级 / 系统级 LaunchAgents（系统级只读）
fn read_launch_agents(
    dir: &Path,
    scope: &str,
    disabled: &HashSet<String>,
) -> Vec<StartupItem> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let can_toggle = scope == "user";
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') || !file_name.to_ascii_lowercase().ends_with(".plist") {
            continue;
        }
        let fallback_label = Path::new(&file_name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| file_name.clone());
        // 单个 plist 解析失败就跳过，不拖垮整页
        let Ok(value) = plist::Value::from_file(&path) else {
            continue;
        };
        let Some(info) = parse_launch_agent(&value, &fallback_label) else {
            continue;
        };

        let mut detail = format!(
            "LaunchAgent：{}{}",
            path.display(),
            if info.run_at_load {
                "（RunAtLoad：是）"
            } else {
                "（RunAtLoad 未开启，登录时不会自动运行）"
            }
        );
        if !can_toggle {
            detail.push_str("；系统级 LaunchAgent 需要 root 权限，本版本只读展示");
        }
        items.push(StartupItem {
            id: launch_agent_id(scope, &info.label),
            name: info.label.clone(),
            command: info.command.clone(),
            source: "launch_agent".to_string(),
            scope: scope.to_string(),
            enabled: !disabled.contains(&info.label),
            can_toggle,
            detail,
        });
    }
    items.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    items
}

fn login_item_startup(login: &LoginItemInfo) -> StartupItem {
    let path_label = if login.path.is_empty() {
        "（路径未知）".to_string()
    } else {
        login.path.clone()
    };
    StartupItem {
        id: login_item_id(&login.name, &login.path),
        name: login.name.clone(),
        command: login.path.clone(),
        source: "login_item".to_string(),
        scope: "user".to_string(),
        enabled: true,
        can_toggle: true,
        detail: format!("登录项：{}；关闭会从「登录项」中移除", path_label),
    }
}

fn login_items_unavailable(error: &str) -> StartupItem {
    StartupItem {
        id: LOGIN_ITEMS_UNAVAILABLE_ID.to_string(),
        name: "登录项（需要自动化授权）".to_string(),
        command: String::new(),
        source: "login_item".to_string(),
        scope: "user".to_string(),
        enabled: false,
        can_toggle: false,
        detail: format!(
            "无法读取登录项：{}。请到「系统设置 › 隐私与安全性 › 自动化」允许本应用控制 System Events 后重试（不影响上面的 LaunchAgents 列表）。",
            error.trim()
        ),
    }
}

/// 启用 / 禁用一条启动项
pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    if let Some((scope, label)) = parse_launch_agent_id(id) {
        if scope != "user" {
            return Err("系统级 LaunchAgent 需要 root 权限，本版本不支持启停".to_string());
        }
        let Some(home) = crate::cleaner::home_dir() else {
            return Err("取不到用户主目录".to_string());
        };
        let dir = home.join("Library/LaunchAgents");
        let path = find_launch_agent(&dir, &label)
            .ok_or_else(|| format!("找不到 LaunchAgent 文件：{}", label))?;
        return toggle_launch_agent(&path, &label, enabled);
    }

    if let Some(login) = parse_login_item_id(id) {
        if enabled {
            return add_login_item(&login.name, &login.path);
        }
        return delete_login_item(&login.name);
    }

    Err(format!("无效的启动项标识: {}", id))
}

/// 在目录里按 Label 找 plist（Label 与文件名不一定一致）
fn find_launch_agent(dir: &Path, label: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_plist = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("plist"))
            .unwrap_or(false);
        if !path.is_file() || !is_plist {
            continue;
        }
        let fallback = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_default();
        let Ok(value) = plist::Value::from_file(&path) else {
            continue;
        };
        if let Some(info) = parse_launch_agent(&value, &fallback) {
            if info.label == label {
                return Some(path);
            }
        }
    }
    None
}

/// 切换 LaunchAgent：先写 disabled override，再按需 bootstrap / bootout
fn toggle_launch_agent(path: &Path, label: &str, enabled: bool) -> Result<(), String> {
    let domain = gui_domain();
    let service = format!("{}/{}", domain, label);
    let plist = path.to_string_lossy().to_string();

    if enabled {
        launchctl(&["enable", &service])
            .map_err(|error| format!("启用 LaunchAgent 失败: {}", error))?;
        if !is_service_loaded(&service) {
            launchctl(&["bootstrap", &domain, &plist])
                .map_err(|error| format!("加载 LaunchAgent 失败: {}", error))?;
        }
        Ok(())
    } else {
        // 先 disable 再 bootout：bootout 之后 launchd 不会因为 RunAtLoad 再拉起来
        launchctl(&["disable", &service])
            .map_err(|error| format!("禁用 LaunchAgent 失败: {}", error))?;
        if is_service_loaded(&service) {
            launchctl(&["bootout", &domain, &plist])
                .map_err(|error| format!("停止 LaunchAgent 失败: {}", error))?;
        }
        Ok(())
    }
}

fn is_service_loaded(service: &str) -> bool {
    std::process::Command::new("launchctl")
        .args(["print", service])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn read_disabled_services() -> HashSet<String> {
    match launchctl(&["print-disabled", &gui_domain()]) {
        // 读不到就当没有禁用项：状态显示不准好过整页报错
        Ok(output) => parse_disabled_services(&output),
        Err(_) => HashSet::new(),
    }
}

fn gui_domain() -> String {
    // 用 libc::getuid 而不是 `id -u` 起子进程：启动项列表每次刷新都要读状态
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchctl(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|error| format!("启动 launchctl 失败: {}", error))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!("launchctl {} 失败", args.join(" "))
    } else {
        stderr
    })
}

fn osascript(script: &str) -> Result<String, String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|error| format!("启动 osascript 失败: {}", error))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        "osascript 执行失败".to_string()
    } else {
        stderr
    })
}

fn read_login_items() -> Result<Vec<LoginItemInfo>, String> {
    let output = osascript(LOGIN_ITEMS_SCRIPT)?;
    Ok(parse_login_items(&output))
}

fn delete_login_item(name: &str) -> Result<(), String> {
    let script = format!(
        "tell application \"System Events\"\n\tdelete login item \"{}\"\nend tell",
        escape_applescript(name)
    );
    osascript(&script).map(|_| ())
}

fn add_login_item(name: &str, path: &str) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Err(format!(
            "「{}」缺少可执行文件路径，无法自动恢复为登录项；请到「系统设置 › 通用 › 登录项」手动添加",
            name
        ));
    }
    let script = format!(
        "tell application \"System Events\"\n\tmake login item at end with properties {{path:\"{}\", hidden:false}}\nend tell",
        escape_applescript(path)
    );
    osascript(&script).map(|_| ())
}

/// AppleScript 字符串字面量转义（反斜杠与双引号）
fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

// ============================== 本应用登录启动 ==============================

fn app_agent_path() -> Option<PathBuf> {
    crate::cleaner::home_dir().map(|home| {
        home.join("Library/LaunchAgents")
            .join(format!("{}.plist", APP_AUTOSTART_NAME))
    })
}

/// 只认「配置存在」：plist 在 = 用户开了登录启动（不额外探测 launchctl，
/// 避免一次开关要等系统服务状态）
pub fn app_autostart_get() -> bool {
    app_agent_path().map(|path| path.is_file()).unwrap_or(false)
}

pub fn app_autostart_set(enabled: bool) -> Result<(), String> {
    let path = app_agent_path().ok_or_else(|| "取不到用户主目录".to_string())?;
    let domain = gui_domain();
    let service = format!("{}/{}", domain, APP_AUTOSTART_NAME);

    if !enabled {
        if is_service_loaded(&service) {
            // bootout 失败不阻断删除 plist：没有 plist 就不会再登录启动
            let _ = launchctl(&["bootout", &domain, &path.to_string_lossy()]);
        }
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "删除登录启动配置失败（{}）: {}",
                path.display(),
                error
            )),
        };
    }

    write_app_agent_plist(&path)?;
    // 之前可能被 launchctl disable 过：先 enable，再按需 bootstrap
    let _ = launchctl(&["enable", &service]);
    if !is_service_loaded(&service) {
        launchctl(&["bootstrap", &domain, &path.to_string_lossy()])
            .map_err(|error| format!("加载登录启动配置失败: {}", error))?;
    }
    Ok(())
}

/// 写 `~/Library/LaunchAgents/ws-workbench.plist`：RunAtLoad + 当前可执行文件路径
fn write_app_agent_plist(path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("获取当前程序路径失败: {}", error))?;
    if let Some(parent) = path.parent() {
        crate::util::fs::create_dir_all(parent)?;
    }

    let mut dict = plist::Dictionary::new();
    dict.insert(
        "Label".to_string(),
        plist::Value::String(APP_AUTOSTART_NAME.to_string()),
    );
    dict.insert(
        "ProgramArguments".to_string(),
        plist::Value::Array(vec![plist::Value::String(
            exe.to_string_lossy().to_string(),
        )]),
    );
    dict.insert("RunAtLoad".to_string(), plist::Value::Boolean(true));
    plist::Value::Dictionary(dict)
        .to_file_xml(path)
        .map_err(|error| format!("写入登录启动配置失败（{}）: {}", path.display(), error))
}

// ============================== 单测（纯解析逻辑） ==============================

#[cfg(test)]
mod tests {
    use super::*;

    fn value_from_pairs(pairs: Vec<(&str, plist::Value)>) -> plist::Value {
        let mut dict = plist::Dictionary::new();
        for (key, value) in pairs {
            dict.insert(key.to_string(), value);
        }
        plist::Value::Dictionary(dict)
    }

    fn strings(items: &[&str]) -> plist::Value {
        plist::Value::Array(items.iter().map(|item| plist::Value::String(item.to_string())).collect())
    }

    #[test]
    fn parses_launch_agent_with_program_arguments() {
        let value = value_from_pairs(vec![
            ("Label", plist::Value::String("com.example.agent".to_string())),
            ("RunAtLoad", plist::Value::Boolean(true)),
            (
                "ProgramArguments",
                strings(&["/Applications/My App/helper", "--daemon", "--name=a b"]),
            ),
        ]);
        let info = parse_launch_agent(&value, "fallback").unwrap();
        assert_eq!(info.label, "com.example.agent");
        assert!(info.run_at_load);
        assert_eq!(
            info.command,
            r#""/Applications/My App/helper" --daemon "--name=a b""#
        );
    }

    #[test]
    fn parses_launch_agent_with_program_field() {
        let value = value_from_pairs(vec![
            ("Label", plist::Value::String("com.example.agent".to_string())),
            ("Program", plist::Value::String("/usr/local/bin/agent".to_string())),
            ("ProgramArguments", strings(&["--flag", "value"])),
        ]);
        let info = parse_launch_agent(&value, "fallback").unwrap();
        assert_eq!(info.label, "com.example.agent");
        assert!(!info.run_at_load, "缺省 RunAtLoad 视为 false");
        assert_eq!(info.command, "/usr/local/bin/agent --flag value");
    }

    #[test]
    fn parses_launch_agent_falls_back_to_file_stem() {
        let value = value_from_pairs(vec![("ProgramArguments", strings(&["/bin/true"]))]);
        let info = parse_launch_agent(&value, "com.example.from-file").unwrap();
        assert_eq!(info.label, "com.example.from-file");
        assert_eq!(info.command, "/bin/true");
    }

    #[test]
    fn rejects_plist_without_command() {
        let value = value_from_pairs(vec![("Label", plist::Value::String("com.example".into()))]);
        assert!(parse_launch_agent(&value, "com.example").is_none());
        // 不是 dict 也返回 None
        assert!(parse_launch_agent(&plist::Value::String("x".into()), "x").is_none());
    }

    #[test]
    fn parses_disabled_services_output() {
        let output = r#"
disabled services = {
	"com.foo.bar" => disabled
	"com.foo.baz" => enabled
	"com.example.launch" => disabled
}
"#;
        let disabled = parse_disabled_services(output);
        assert!(disabled.contains("com.foo.bar"));
        assert!(disabled.contains("com.example.launch"));
        assert!(!disabled.contains("com.foo.baz"));
        assert_eq!(disabled.len(), 2);
        // 空输出 / 垃圾输出不 panic
        assert!(parse_disabled_services("").is_empty());
        assert!(parse_disabled_services("nothing here").is_empty());
    }

    #[test]
    fn parses_login_items_output() {
        let output = "Foo\t/Applications/Foo.app\nBar\t/Applications/Bar Baz.app\n\n";
        let items = parse_login_items(output);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Foo");
        assert_eq!(items[0].path, "/Applications/Foo.app");
        assert_eq!(items[1].path, "/Applications/Bar Baz.app");
        // 没有 path 的行也保留
        let items = parse_login_items("OnlyName");
        assert_eq!(items.len(), 1);
        assert!(items[0].path.is_empty());
    }

    #[test]
    fn login_item_id_roundtrip() {
        let id = login_item_id("WeChat", "/Applications/WeChat.app");
        let parsed = parse_login_item_id(&id).unwrap();
        assert_eq!(parsed.name, "WeChat");
        assert_eq!(parsed.path, "/Applications/WeChat.app");
        // 名称为空 / 前缀不对都返回 None
        assert!(parse_login_item_id("mac:login_item:").is_none());
        assert!(parse_login_item_id("mac:launch_agent:user:x").is_none());
    }

    #[test]
    fn launch_agent_id_roundtrip() {
        let id = launch_agent_id("user", "com.example.agent");
        assert_eq!(id, "mac:launch_agent:user:com.example.agent");
        let (scope, label) = parse_launch_agent_id(&id).unwrap();
        assert_eq!(scope, "user");
        assert_eq!(label, "com.example.agent");
        assert!(parse_launch_agent_id("mac:launch_agent:user").is_none());
    }

    #[test]
    fn escapes_applescript_literals() {
        assert_eq!(escape_applescript("/Applications/Foo.app"), "/Applications/Foo.app");
        assert_eq!(
            escape_applescript(r#"/tmp/a "quoted" \ name"#),
            r#"/tmp/a \"quoted\" \\ name"#
        );
    }
}
