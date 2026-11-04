//! VPN 相关的 Tauri 命令与状态轮询

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::platform::{Os, Platform};
use crate::vpn::guide;
use crate::vpn::models::*;
use crate::vpn::session::{LogCursor, VpnSession};
use crate::vpn::{detect, elevate, route};

/// 状态轮询间隔：既能让界面反应够快，也不会把磁盘打满
const POLL_INTERVAL: Duration = Duration::from_millis(600);

/// 前端监听的事件名
pub const EVENT_STATUS: &str = "vpn-status";
pub const EVENT_LOG: &str = "vpn-log";

/// 运行期状态
pub struct VpnRuntime {
    pub session: VpnSession,
    /// 已经推给前端的日志游标
    pub log_cursor: LogCursor,
    /// 上一次推送的状态，避免重复 emit
    pub last_status: Option<VpnStatus>,
    /// 上一次提权方式的描述，用于界面提示
    pub elevation: String,
}

pub struct VpnState(pub Mutex<VpnRuntime>);

impl VpnState {
    pub fn new(session: VpnSession) -> Self {
        VpnState(Mutex::new(VpnRuntime {
            session,
            log_cursor: LogCursor::default(),
            last_status: None,
            elevation: String::new(),
        }))
    }
}

/// 会话目录：主进程与提权 worker 都要能读写。
///
/// 首选应用数据目录（Roaming），但那里并不总是可写：企业策略、杀毒软件的
/// 受控文件夹访问、以及从受限令牌启动的进程都可能被拒绝。这种情况不该让整个
/// VPN 功能直接失败，所以依次退到本地数据目录、临时目录。
///
/// 解析结果会缓存：状态轮询每 600ms 就要用一次，不能每次都去探测磁盘。
static SESSION_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// 用户在界面上看到的会话目录来源，用于解释「为什么日志不在预期位置」
static SESSION_DIR_ORIGIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn session_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(dir) = SESSION_DIR.get() {
        return Ok(dir.clone());
    }
    let (dir, origin) = resolve_session_dir(app)?;
    let _ = SESSION_DIR.set(dir.clone());
    let _ = SESSION_DIR_ORIGIN.set(origin);
    Ok(dir)
}

/// 会话目录来自哪个位置（应用数据目录 / 本地数据目录 / 临时目录）
pub fn session_dir_origin() -> String {
    SESSION_DIR_ORIGIN
        .get()
        .cloned()
        .unwrap_or_else(|| "应用数据目录".to_string())
}

fn resolve_session_dir(app: &AppHandle) -> Result<(PathBuf, String), String> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(base) = app.path().app_data_dir() {
        candidates.push(("应用数据目录".to_string(), base));
    }
    if let Ok(base) = app.path().app_local_data_dir() {
        candidates.push(("本地数据目录".to_string(), base));
    }
    // dev 构建在受限环境（受控文件夹访问、沙箱）下可能连本地数据目录都建不出来。
    // 这时退到可执行文件旁：target/debug 对开发进程一定可写，提权 worker 也用同一路径。
    #[cfg(debug_assertions)]
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push((
                "开发目录（可执行文件旁）".to_string(),
                parent.join("ws-workbench-dev"),
            ));
        }
    }
    // 兜底：临时目录在当前用户 profile 下，权限最宽松
    candidates.push((
        "临时目录".to_string(),
        std::env::temp_dir().join(crate::vpn::APP_DIR_NAME),
    ));

    let mut tried: Vec<String> = Vec::new();
    for (label, base) in candidates {
        if let Some(dir) = probe_writable(&base) {
            return Ok((dir, label));
        }
        tried.push(base.display().to_string());
    }

    Err(format!(
        "找不到可写的会话目录，已依次尝试：{}。\
         请检查杀毒软件的受控文件夹访问或企业策略是否禁止本程序写入用户目录。",
        tried.join("、")
    ))
}

/// 目录能建出来不代表能写进去。
///
/// `create_dir_all` 在目录已存在时会直接返回成功，把一个只读目录也伪装成可用，
/// 所以必须真写一个探针文件才算数 —— 否则问题会推迟到连接时才爆发。
fn probe_writable(base: &std::path::Path) -> Option<PathBuf> {
    let dir = base.join("vpn");
    std::fs::create_dir_all(&dir).ok()?;
    let probe = dir.join(".write-probe");
    std::fs::write(&probe, b"ok").ok()?;
    let _ = std::fs::remove_file(&probe);
    Some(dir)
}

fn session_of(app: &AppHandle) -> Result<VpnSession, String> {
    Ok(VpnSession::new(session_dir(app)?))
}

/// 上次注册免 UAC 计划任务时用的会话目录（记在配置里）。
///
/// 计划任务的命令行把会话目录写死了，所以「任务存在」不等于「任务可用」。
/// 记一份到 store 里，连接时比一下字符串就够了，不必每次去查任务定义。
fn recorded_task_dir(app: &AppHandle) -> Option<String> {
    let dir = crate::config::load_config(app).vpn_worker_session_dir;
    if dir.trim().is_empty() {
        None
    } else {
        Some(dir)
    }
}

/// 把「注册计划任务时用的会话目录」记进配置，供下次连接比对。
fn remember_task_dir(app: &AppHandle, dir: &std::path::Path) {
    let mut config = crate::config::load_config(app);
    let value = dir.to_string_lossy().to_string();
    if config.vpn_worker_session_dir == value {
        return;
    }
    config.vpn_worker_session_dir = value;
    if let Err(err) = crate::config::save_config(app, &config) {
        // 记不上不影响本次连接，只是下次可能多弹一次 UAC，不要因此中断流程
        eprintln!("记录计划任务会话目录失败: {}", err);
    }
}

/// 清掉记录（移除计划任务时一并调用）
fn forget_task_dir(app: &AppHandle) {
    let mut config = crate::config::load_config(app);
    if config.vpn_worker_session_dir.is_empty() {
        return;
    }
    config.vpn_worker_session_dir = String::new();
    let _ = crate::config::save_config(app, &config);
}

// ============================== 连接器 ==============================

/// 组装探测上下文：资源目录 / 可执行文件目录 / 应用数据目录。
/// 与项目打包页共用同一份上下文，保证「设置页里指定的路径」两边都生效。
fn detect_context(app: &AppHandle) -> crate::apps::AppContext {
    crate::commands::app_context(app)
}

#[tauri::command]
pub fn vpn_detect_connectors(
    app: AppHandle,
    custom_openconnect: Option<String>,
) -> Vec<ConnectorInfo> {
    detect::detect_all(
        &detect_context(&app),
        custom_openconnect.as_deref().unwrap_or(""),
    )
}

/// 从用户选定的目录里导入一份 openconnect（含同目录的 DLL），放进程序自管目录。
///
/// 这样换机器时只要指一下已有的 openconnect 目录即可，不需要管理员权限，
/// 也不用改动安装目录。
#[tauri::command]
pub fn vpn_import_openconnect(app: AppHandle, source_dir: String) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {}", e))?;
    let version = detect::import_openconnect(&data_dir, std::path::Path::new(&source_dir))?;
    Ok(format!("已导入 OpenConnect {}", version))
}

#[tauri::command]
pub fn vpn_remove_managed_openconnect(app: AppHandle) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {}", e))?;
    detect::remove_managed_openconnect(&data_dir)?;
    Ok("已移除导入的 OpenConnect".to_string())
}

/// 程序托管的 openconnect 是否已存在
#[tauri::command]
pub fn vpn_managed_openconnect_path(app: AppHandle) -> Option<String> {
    let data_dir = app.path().app_data_dir().ok()?;
    let dir = crate::apps::managed_dir(crate::apps::AppId::Openconnect, &data_dir);
    for name in crate::apps::AppId::Openconnect.executable_names() {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

/// 查一个自定义路径上的 openconnect 版本号
#[tauri::command]
pub fn vpn_connector_version(path: String) -> String {
    let info = ConnectorInfo {
        kind: "openconnect".to_string(),
        name: String::new(),
        path,
        version: String::new(),
        available: true,
        source: "custom".to_string(),
        note: String::new(),
    };
    detect::fetch_version(&info)
}

#[tauri::command]
pub fn vpn_install_guide(candidates: Vec<ConnectorInfo>) -> InstallGuide {
    let openconnect_available = candidates
        .iter()
        .any(|c| c.kind == "openconnect" && c.available);
    guide::build(Os::current(), openconnect_available)
}

// ============================== 规则校验 ==============================

#[tauri::command]
pub fn vpn_validate_cidr(cidr: String) -> Result<String, String> {
    route::normalize_cidr(&cidr)
}

/// 检查规则表里的问题：
/// 1. 某条规则被另一条更宽泛的规则完全覆盖，永远不会生效
/// 2. 同一条规则重复出现
#[tauri::command]
pub fn vpn_check_rule_overlaps(cidrs: Vec<String>) -> Vec<String> {
    let mut parsed: Vec<(String, u32, u8)> = Vec::new();
    for cidr in cidrs {
        if let Ok((network, prefix)) = route::parse_cidr(&cidr) {
            let text = route::normalize_cidr(&cidr).unwrap_or(cidr);
            parsed.push((text, network, prefix));
        }
    }

    let mut warnings = Vec::new();

    // 重复项
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (text, _, _) in &parsed {
        *counts.entry(text.as_str()).or_insert(0) += 1;
    }
    for (text, count) in counts {
        if count > 1 {
            warnings.push(format!("{} 重复出现 {} 次，可以删掉多余的", text, count));
        }
    }

    // 被更宽泛的规则覆盖
    for (i, (text_a, network_a, prefix_a)) in parsed.iter().enumerate() {
        for (j, (text_b, network_b, prefix_b)) in parsed.iter().enumerate() {
            if i == j || prefix_b >= prefix_a {
                continue;
            }
            let mask_b = route::prefix_to_mask(*prefix_b);
            if network_a & mask_b == *network_b {
                warnings.push(format!("{} 已被 {} 覆盖，这条规则不会生效", text_a, text_b));
            }
        }
    }

    warnings.sort();
    warnings.dedup();
    warnings
}

/// 单条规则的入参校验，供 UI 即时反馈。
pub fn validate_rule_input(kind: &str, value: &str) -> Result<(), String> {
    use crate::vpn::rule_engine::{parse_port_range, RuleKind};
    let kind = RuleKind::parse(kind)?;
    if value.trim().is_empty() {
        return Err("匹配值不能为空".to_string());
    }
    match kind {
        RuleKind::DstPort => {
            parse_port_range(value)?;
        }
        RuleKind::IpCidr => {
            crate::vpn::route::normalize_cidr(value)?;
        }
        _ => {}
    }
    Ok(())
}

/// 校验单条规则并回填规范化后的匹配值（`IP-CIDR` 会规范化网络地址）。
#[tauri::command]
pub fn vpn_validate_rule(kind: String, value: String) -> Result<String, String> {
    validate_rule_input(&kind, &value)?;
    Ok(match crate::vpn::rule_engine::RuleKind::parse(&kind)? {
        crate::vpn::rule_engine::RuleKind::IpCidr => crate::vpn::route::normalize_cidr(&value)?,
        _ => value,
    })
}

/// 用给定规则集与一条输入预览命中结果，供 UI 展示「这条会走哪」。
#[tauri::command]
pub fn vpn_preview_rules(
    rules: Vec<VpnRule>,
    default_policy: String,
    domain: Option<String>,
    ip: Option<String>,
    port: Option<u16>,
    process: Option<String>,
) -> Result<String, String> {
    use crate::vpn::rule_engine::{compile, MatchInput, Policy, Rule, RuleKind};
    let mut compiled = Vec::new();
    for r in rules.iter().filter(|r| r.enabled) {
        let kind = RuleKind::parse(&r.kind)?;
        let policy = Policy::parse(&r.policy)?;
        compiled.push(Rule::new(kind, &r.value, policy).with_no_resolve(r.no_resolve));
    }
    let default = Policy::parse(&default_policy)?;
    let set = compile(&compiled, &std::collections::HashMap::new(), default)?;
    let verdict = set.evaluate(&MatchInput {
        domain: domain.as_deref(),
        ip: ip.as_deref(),
        port,
        process: process.as_deref(),
    });
    Ok(verdict.as_str().to_string())
}

// ============================== 提权 ==============================

#[derive(serde::Serialize)]
pub struct ElevationInfo {
    /// 当前进程是否已是管理员
    pub elevated: bool,
    /// Windows 计划任务是否已注册（注册后连接不再弹 UAC）
    pub task_ready: bool,
    /// 平台
    pub platform: String,
    /// 给界面的一句话说明
    pub hint: String,
}

#[tauri::command]
pub fn vpn_elevation_info(app: AppHandle) -> ElevationInfo {
    // 平台判断统一走 platform 模块。
    // 之前这里是「不是 windows 就当 macos」，Linux 上会错误地提示系统授权框。
    let os = Os::current();
    let elevated = elevate::is_elevated();
    // 任务存在还不够：里面写死了会话目录，回退过就作废了，
    // 否则界面会显示「已注册免 UAC」，结果连接时又弹一次 UAC。
    let task_ready = match session_of(&app) {
        Ok(session) => {
            let recorded = recorded_task_dir(&app);
            elevate::worker_task_exists()
                && elevate::worker_task_matches(&session.dir, recorded.as_deref())
        }
        Err(_) => elevate::worker_task_exists(),
    };

    let hint = if elevated {
        "当前已是管理员权限，无需额外提权".to_string()
    } else if Platform::is_windows(&Platform) {
        if task_ready {
            "已注册免 UAC 计划任务，之后连接不再弹管理员确认".to_string()
        } else {
            "尚未注册免 UAC 计划任务，首次连接会弹一次管理员确认".to_string()
        }
    } else if Platform::is_macos(&Platform) {
        "首次连接会弹出系统授权框（等价于 Windows 的 UAC）".to_string()
    } else {
        "Linux 下需要 root 权限或 CAP_NET_ADMIN 才能创建隧道并修改路由".to_string()
    };

    ElevationInfo {
        elevated,
        task_ready,
        platform: os.as_str().to_string(),
        hint,
    }
}

/// 注册免 UAC 的计划任务（Windows 会弹一次 UAC 用于注册）
#[tauri::command]
pub fn vpn_setup_elevation(app: AppHandle) -> Result<String, String> {
    let session = session_of(&app)?;
    let exe = std::env::current_exe().map_err(|e| format!("取当前程序路径失败: {}", e))?;
    session.ensure()?;
    elevate::ensure_worker_task(&exe, &session)?;
    if elevate::worker_task_exists() {
        // 任务命令行里带着会话目录，记下来，下次连接就知道它还有没有效
        remember_task_dir(&app, &session.dir);
        Ok("计划任务已注册，之后连接不再弹 UAC".to_string())
    } else {
        Err("计划任务注册后未查询到，可能被系统策略拦截".to_string())
    }
}

#[tauri::command]
pub fn vpn_remove_elevation(app: AppHandle) -> Result<String, String> {
    elevate::delete_worker_task()?;
    forget_task_dir(&app);
    Ok("已移除免 UAC 计划任务".to_string())
}

// ============================== 连接 / 断开 ==============================

#[derive(serde::Serialize)]
pub struct ConnectResult {
    pub connector_name: String,
    pub connector_path: String,
    pub elevation: String,
}

/// 必须是 async：非 async 的 Tauri 命令跑在主线程上，而下面的提权步骤要等用户
/// 在系统授权框上点确认（最长 120s + 10s 轮询），同步执行会把整个界面卡死。
#[tauri::command]
pub async fn vpn_connect(
    app: AppHandle,
    state: State<'_, VpnState>,
    profile: VpnProfile,
    rules: Vec<VpnRule>,
    default_policy: Option<String>,
    custom_openconnect: Option<String>,
) -> Result<ConnectResult, String> {
    // 兜底策略由前端从所选规则集透传，缺省或空白时退回直连
    let default_policy = default_policy
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "DIRECT".to_string());
    // 1. 选连接器
    let candidates = vpn_detect_connectors(app.clone(), custom_openconnect.clone());
    let picked = detect::pick_connector(&candidates, profile.connector).ok_or_else(|| {
        "没有可用的连接器：没找到 openconnect。请到 VPN 页查看安装引导。".to_string()
    })?;
    let connector = picked.clone();

    // 2. 取密码（password 已是明文，落盘保护由整块配置的 SM4 信封承担）
    // 旧字段级密文若迁移失败会保留 password_encrypted=true，此时绝不能把密文当密码发出去
    if profile.password_encrypted {
        return Err("该配置的旧密码尚未完成迁移，请在 VPN 配置页重新填写密码".to_string());
    }
    let password = profile.password.clone();
    if password.is_empty() {
        return Err("这个配置还没有保存密码，请先在配置里填写密码".to_string());
    }

    // 3. 组装连接请求
    let rules: Vec<VpnRule> = rules.into_iter().filter(|rule| rule.enabled).collect();

    // 规则先编译一次：任何一条规则不合法都在这里拦下，避免带着坏规则去连。
    // 这是 fail-closed —— 宁可连接失败，也不放行一份语义错误的规则集。
    {
        use crate::vpn::rule_engine::{compile, Policy, Rule, RuleKind};
        let mut compiled = Vec::new();
        for r in rules.iter().filter(|r| r.enabled) {
            let kind = RuleKind::parse(&r.kind)?;
            let policy = Policy::parse(&r.policy)?;
            compiled.push(Rule::new(kind, &r.value, policy).with_no_resolve(r.no_resolve));
        }
        let default = Policy::parse(&default_policy)?;
        compile(&compiled, &std::collections::HashMap::new(), default)
            .map_err(|e| format!("规则编译失败：{}", e))?;
    }

    // 先落盘连接请求。持锁的部分只做文件操作，绝不跨过提权那一步。
    let session = {
        let mut runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
        let session = runtime.session.clone();
        session.prepare()?;
        runtime.log_cursor = LogCursor::default();
        runtime.last_status = None;
        session
    };

    let exe = std::env::current_exe().map_err(|e| format!("取当前程序路径失败: {}", e))?;
    let request = VpnRequest {
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        server: profile.server.clone(),
        username: profile.username.clone(),
        password,
        protocol: profile.protocol.clone(),
        connector: profile.connector,
        connector_path: connector.path.clone(),
        rules: rules.clone(),
        default_policy: default_policy.clone(),
        fake_ip_range: profile.fake_ip_range.clone(),
        fake_ip_filter: profile.fake_ip_filter.clone(),
        use_vpn_dns: profile.use_vpn_dns,
        learn: profile.learn,
        extra_args: profile.extra_args.clone(),
        script_path: String::new(), // worker 生成脚本后回填
        hook_exe: exe.to_string_lossy().to_string(),
        created_at: crate::util::time::now_local_rfc3339(),
    };
    session.write_request(&request)?;

    // 接下来可能弹出 UAC 并等用户点确认，这一步必须放开锁：
    // 否则状态轮询线程会被一起卡住，界面看上去就像死了。
    let elevation = if elevate::is_elevated() {
        spawn_worker_directly(&exe, &session)?;
        "直接启动（当前已是管理员）".to_string()
    } else {
        // 计划任务的命令行里写死了会话目录，所以「任务存在」不等于「任务可用」。
        // 配置里记着上次注册时用的目录（见 remember_task_dir），比一下就知道。
        let mut recorded = recorded_task_dir(&app);
        let task_usable = elevate::worker_task_exists()
            && elevate::worker_task_matches(&session.dir, recorded.as_deref());

        if Platform::is_windows(&Platform) && !task_usable {
            // 注册计划任务：首次会弹一次 UAC，之后免 UAC。
            // 会话目录回退过的话，旧任务里写死的是旧路径，必须重新注册，
            // 否则 worker 会往旧目录写、主进程在新目录等，连接必然失败。
            match elevate::ensure_worker_task(&exe, &session) {
                Ok(_) => {
                    remember_task_dir(&app, &session.dir);
                    // 任务已经指向当前目录了，别再拿旧记录去判断，
                    // 否则刚注册完又会被当成「过期任务」而白白弹一次 UAC。
                    recorded = Some(session.dir.to_string_lossy().to_string());
                }
                Err(e) => {
                    session.append_log(&format!(
                        "[elevate] 计划任务注册失败，将回退到 UAC 提权: {}",
                        e
                    ));
                }
            }
        }
        elevate::start_worker(&exe, &session, recorded.as_deref())?
    };

    if let Ok(mut runtime) = state.0.lock() {
        runtime.elevation = elevation.clone();
    }

    Ok(ConnectResult {
        connector_name: connector.name,
        connector_path: connector.path,
        elevation,
    })
}

/// 当前进程已是管理员时，直接起 worker，省掉一次多余的系统授权
fn spawn_worker_directly(exe: &std::path::Path, session: &VpnSession) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let mut command = Command::new(exe);
    command
        .arg("--vpn-worker")
        .arg(&session.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::vpn::connector::CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 worker 失败: {}", e))
}

/// 同样必须是 async：这里要等 worker 收尾（最长 20s），同步执行会卡住界面。
#[tauri::command]
pub async fn vpn_disconnect(app: AppHandle, state: State<'_, VpnState>) -> Result<String, String> {
    let runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    let session = runtime.session.clone();
    drop(runtime);

    let status = session.read_status().unwrap_or_default();
    if status.phase == VpnPhase::Disconnected {
        return Ok("当前未连接".to_string());
    }

    elevate::stop_worker(&session);
    app.emit(
        EVENT_STATUS,
        VpnStatus {
            phase: VpnPhase::Disconnecting,
            message: "正在断开并还原路由".to_string(),
            ..status
        },
    )
    .ok();

    // 等 worker 自己收尾，超时就标记为已断开，避免界面卡在「断开中」
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        if let Some(latest) = session.read_status() {
            if latest.phase == VpnPhase::Disconnected || latest.phase == VpnPhase::Failed {
                return Ok(latest.message);
            }
            // worker 进程已经没了却还没写终态，说明是被强杀
            if latest.pid != 0 && !is_process_alive(latest.pid) {
                let mut final_status = latest;
                final_status.phase = VpnPhase::Disconnected;
                final_status.message = "worker 已退出，已断开".to_string();
                final_status.routes.clear();
                let _ = session.write_status(&final_status);
                return Ok(final_status.message);
            }
        }
    }

    Err("断开超时，请检查日志；若网络异常可尝试重新连接一次".to_string())
}

/// 进程是否还活着（用于判断 worker 是否被强杀）
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        let (text, _) = crate::process::run_with_timeout(
            std::path::Path::new("tasklist.exe"),
            &["/FI", &format!("PID eq {}", pid), "/NH"],
            Duration::from_secs(15),
        );
        text.contains(&pid.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        // macOS 没有 /proc，必须真的去问一下进程表。
        // `ps -p <pid> -o pid=` 命中时退出码为 0 并打印 pid，找不到时非 0 且无输出。
        let (text, ok) = crate::process::run_with_timeout(
            std::path::Path::new("/bin/ps"),
            &["-p", &pid.to_string(), "-o", "pid="],
            Duration::from_secs(5),
        );
        ok && text.contains(&pid.to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::path::Path::new(&format!("/proc/{}", pid)).exists()
    }
}

// ============================== 状态与日志 ==============================

#[derive(serde::Serialize, Clone)]
pub struct VpnSnapshotPayload {
    pub status: VpnStatus,
    pub elevation: String,
}

#[tauri::command]
pub fn vpn_get_snapshot(state: State<'_, VpnState>) -> VpnSnapshotPayload {
    let runtime = match state.0.lock() {
        Ok(runtime) => runtime,
        Err(_) => {
            return VpnSnapshotPayload {
                status: VpnStatus::default(),
                elevation: String::new(),
            }
        }
    };
    VpnSnapshotPayload {
        status: runtime.session.read_status().unwrap_or_default(),
        elevation: runtime.elevation.clone(),
    }
}

/// 前端刚打开 VPN 页时，把已有的日志尾巴一次性拉回来
#[tauri::command]
pub fn vpn_read_logs(state: State<'_, VpnState>, max_lines: Option<usize>) -> Vec<String> {
    match state.0.lock() {
        Ok(runtime) => runtime.session.read_log_tail(max_lines.unwrap_or(500)),
        Err(_) => Vec::new(),
    }
}

/// 拉取学习模式的完整明细。
///
/// 明细不跟着 `status.json` 走（那边 600ms 一次 diff，塞不动几十 KB），
/// 只有用户主动展开面板时才读一次 `learn.json`。
#[tauri::command]
pub fn vpn_read_learn_report(state: State<'_, VpnState>) -> Option<LearnReport> {
    match state.0.lock() {
        Ok(runtime) => runtime.session.read_learn(),
        Err(_) => None,
    }
}

/// 清空学习结果。
///
/// 只放一个标志文件，由 worker 自己把内存里的报告重置掉：报告本体在 worker 手上，
/// 主进程直接删 `learn.json` 的话，下一轮采样会立刻把它写回来。
#[tauri::command]
pub fn vpn_clear_learn_report(state: State<'_, VpnState>) -> Result<(), String> {
    let session = {
        let runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
        runtime.session.clone()
    };
    session.request_learn_clear();
    Ok(())
}

#[tauri::command]
pub fn vpn_open_session_dir(app: AppHandle) -> Result<(), String> {
    let dir = session_dir(&app)?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    crate::commands::open_dir(dir.to_string_lossy().to_string()).map(|_| ())
}

/// 会话目录的实际位置与来源。
///
/// 界面用来说明日志到底写在哪儿 —— 一旦发生回退（Roaming 不可写时退到本地数据
/// 目录或临时目录），用户按默认路径是找不到日志的，必须明确告知。
#[tauri::command]
pub fn vpn_session_dir_info(app: AppHandle) -> Result<SessionDirInfo, String> {
    let dir = session_dir(&app)?;
    let origin = session_dir_origin();
    Ok(SessionDirInfo {
        path: dir.to_string_lossy().to_string(),
        fallback: origin != "应用数据目录",
        origin,
    })
}

#[tauri::command]
pub fn vpn_clear_logs(state: State<'_, VpnState>) -> Result<(), String> {
    let runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    let path = runtime.session.log_path();
    let _ = std::fs::remove_file(&path);
    Ok(())
}

// ============================== 状态轮询 ==============================

/// 后台轮询会话目录，把状态与新增日志推给前端。
///
/// 之所以用轮询而不是管道：提权 worker 无法把输出回传给父进程，
/// 只能靠共用的状态文件。
pub fn start_status_poller(app: AppHandle) {
    std::thread::spawn(move || {
        let mut cursor = LogCursor::default();
        let mut last_status_json = String::new();

        loop {
            std::thread::sleep(POLL_INTERVAL);

            let state = match app.try_state::<VpnState>() {
                Some(state) => state,
                // 应用正在退出
                None => return,
            };

            let (status, elevation, lines) = {
                let runtime = match state.0.lock() {
                    Ok(runtime) => runtime,
                    Err(_) => continue,
                };
                let (lines, next_cursor) = runtime.session.read_log(&cursor);
                cursor = next_cursor;
                (
                    runtime.session.read_status(),
                    runtime.elevation.clone(),
                    lines,
                )
            };

            if let Some(status) = status {
                let json = serde_json::to_string(&status).unwrap_or_default();
                if json != last_status_json {
                    last_status_json = json;
                    let _ = app.emit(
                        EVENT_STATUS,
                        VpnSnapshotPayload {
                            status: status.clone(),
                            elevation: elevation.clone(),
                        },
                    );
                }
            }

            if !lines.is_empty() {
                let _ = app.emit(EVENT_LOG, lines);
            }
        }
    });
}

/// 应用退出前请求 worker 停止，避免留下无人回收的路由
pub fn shutdown(app: &AppHandle) {
    if let Ok(session) = session_of(app) {
        if let Some(status) = session.read_status() {
            if status.phase != VpnPhase::Disconnected && status.phase != VpnPhase::Failed {
                elevate::stop_worker(&session);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_detection_finds_supernet() {
        let warnings = vpn_check_rule_overlaps(vec![
            "192.168.1.0/24".to_string(),
            "192.168.0.0/16".to_string(),
        ]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("192.168.1.0/24"));
        assert!(warnings[0].contains("192.168.0.0/16"));
    }

    #[test]
    fn overlap_detection_ignores_disjoint_rules() {
        let warnings = vpn_check_rule_overlaps(vec![
            "10.0.0.0/8".to_string(),
            "198.51.100.0/24".to_string(),
        ]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn overlap_detection_survives_invalid_input() {
        let warnings = vpn_check_rule_overlaps(vec![
            "not-a-cidr".to_string(),
            "10.0.0.0/8".to_string(),
        ]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn overlap_detection_flags_duplicate_rules() {
        let warnings = vpn_check_rule_overlaps(vec![
            "10.0.0.0/8".to_string(),
            "10.0.0.0/8".to_string(),
        ]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("重复"));
    }

    #[test]
    fn overlap_detection_ignores_equal_prefixes_that_differ() {
        // 同样长度但不同网段，互不覆盖
        let warnings = vpn_check_rule_overlaps(vec![
            "10.0.0.0/8".to_string(),
            "11.0.0.0/8".to_string(),
        ]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn overlap_detection_reports_supernet_only_once() {
        let warnings = vpn_check_rule_overlaps(vec![
            "10.1.0.0/16".to_string(),
            "10.0.0.0/8".to_string(),
        ]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("不会生效"));
    }

    #[test]
    fn validates_known_kinds() {
        assert!(validate_rule_input("DOMAIN", "a.com").is_ok());
        assert!(validate_rule_input("DST-PORT", "443").is_ok());
        assert!(validate_rule_input("DST-PORT", "70000").is_err());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/8").is_ok());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/33").is_err());
        assert!(validate_rule_input("bogus", "x").is_err());
    }

    #[test]
    fn domain_rule_rejects_empty_value() {
        assert!(validate_rule_input("DOMAIN", "").is_err());
    }

    #[test]
    fn probe_accepts_a_writable_directory() {
        let root = std::env::temp_dir().join(format!("ws-probe-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let dir = probe_writable(&root).expect("临时目录应该可写");
        assert!(dir.ends_with("vpn"), "实际: {}", dir.display());
        // 探针文件不能留在用户的会话目录里
        assert!(!dir.join(".write-probe").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn probe_rejects_when_directory_cannot_be_created() {
        // 父路径是个文件，create_dir_all 必然失败
        let root = std::env::temp_dir().join(format!("ws-probe-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::write(&root, b"not a dir").unwrap();

        assert!(probe_writable(&root).is_none());

        let _ = std::fs::remove_file(&root);
    }

    /// 核心性质：目录能建出来 ≠ 能写进去。
    ///
    /// 用「探针路径已被一个同名目录占住」制造「建得成、写不进」的情形。
    /// 如果只看 create_dir_all 的返回值，这种目录会被误判成可用，
    /// 问题就会推迟到用户点「连接」时才以「拒绝访问」爆出来 —— 那正是最初的现场。
    #[test]
    fn probe_rejects_directory_that_exists_but_cannot_be_written() {
        let root = std::env::temp_dir().join(format!("ws-probe-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("vpn").join(".write-probe")).unwrap();

        assert!(
            probe_writable(&root).is_none(),
            "写不进去的目录不能被当成可用"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
