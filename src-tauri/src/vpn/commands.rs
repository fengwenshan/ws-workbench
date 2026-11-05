//! VPN 相关的 Tauri 命令与状态轮询

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::platform::{Os, Platform};
use crate::vpn::guide;
use crate::vpn::models::*;
use crate::vpn::session::{LogCursor, VpnSession};
use crate::vpn::models::ProxyMode;
use crate::vpn::{detect, elevate};

/// 状态轮询间隔：既能让界面反应够快，也不会把磁盘打满
const POLL_INTERVAL: Duration = Duration::from_millis(600);

/// 前端监听的事件名
pub const EVENT_STATUS: &str = "vpn-status";
pub const EVENT_LOG: &str = "vpn-log";

/// 运行期状态
pub struct VpnRuntime {
    pub session: VpnSession,
    /// 运行期「当前生效配置」：同通道共享隧道时，它是切过去的那份配置的 id
    pub policy_profile_id: Option<String>,
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
            policy_profile_id: None,
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

/// 启动时没能探测到可写会话目录的原因（成功时为空）。
///
/// 这种情况会退到临时目录，然后**一直用到重启**。以前失败原因只打在 stderr 上，
/// GUI 用户看不到，最后只在点「连接」时看到一句
/// 「创建会话目录失败 C:\...\Temp\ws-workbench-vpn: 拒绝访问」——
/// 指向的临时目录只是受害者，真正的原因（用户目录当时不可写）被藏起来了。
static SESSION_DIR_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

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

/// 把「启动时探测会话目录失败」的上下文拼到错误信息后面（没失败过就返回空串）。
fn session_dir_diagnosis() -> String {
    let reason = SESSION_DIR_ERROR
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_default();
    if reason.trim().is_empty() {
        return String::new();
    }
    format!(
        "\n应用启动时没能找到可写的会话目录，只能退到临时目录：{}。\
         重启应用会重新探测；如果重启后仍然失败，\
         请检查杀毒软件的「受控文件夹访问」或企业策略是否禁止本程序写入用户目录。",
        reason
    )
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
    // 便携版、以及用户目录整个不可写的机器：可执行文件旁边一定是我们自己的目录，
    // 不需要任何额外的用户目录权限。放在临时目录**之前**——临时目录会随时间被清理，
    // 而且它自己也可能不可写（本机就遇到过 TEMP 的权限被改过、整个目录写不了的情况）。
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(("程序目录（可执行文件旁）".to_string(), parent.to_path_buf()));
        }
    }
    // 兜底：临时目录在当前用户 profile 下，权限最宽松
    candidates.push((
        "临时目录".to_string(),
        std::env::temp_dir().join(crate::vpn::APP_DIR_NAME),
    ));

    let mut tried: Vec<String> = Vec::new();
    for (label, base) in candidates {
        match probe_writable(&base) {
            Some(dir) => {
                if let Ok(mut slot) = SESSION_DIR_ERROR.lock() {
                    *slot = None;
                }
                return Ok((dir, label));
            }
            None => tried.push(base.display().to_string()),
        }
    }

    let reason = format!(
        "找不到可写的会话目录，已依次尝试：{}。\
         请检查杀毒软件的受控文件夹访问或企业策略是否禁止本程序写入用户目录。",
        tried.join("、")
    );
    if let Ok(mut slot) = SESSION_DIR_ERROR.lock() {
        *slot = Some(format!("已依次尝试：{}", tried.join("、")));
    }
    Err(reason)
}

/// 目录能建出来不代表能写进去。
///
/// `create_dir_all` 在目录已存在时会直接返回成功，把一个只读目录也伪装成可用，
/// 所以必须真写一个探针文件才算数 —— 否则问题会推迟到连接时才爆发。
///
/// 探测失败会重试几次：杀毒软件/索引器短暂持有刚建的目录、或沙箱在切换策略的
/// 一瞬间收紧权限，都会让一次探测假失败，进而把整个会话退到临时目录直到重启。
fn probe_writable(base: &std::path::Path) -> Option<PathBuf> {
    let dir = base.join("vpn");
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let probe = dir.join(".write-probe");
        if std::fs::write(&probe, b"ok").is_err() {
            continue;
        }
        let _ = std::fs::remove_file(&probe);
        return Some(dir);
    }
    None
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

#[tauri::command]
pub fn vpn_install_guide(candidates: Vec<ConnectorInfo>) -> InstallGuide {
    let openconnect_available = candidates
        .iter()
        .any(|c| c.kind == "openconnect" && c.available);
    guide::build(Os::current(), openconnect_available)
}

// ============================== 规则校验 ==============================

/// 单条规则的入参校验，供 UI 即时反馈。
pub fn validate_rule_input(kind: &str, value: &str) -> Result<(), String> {
    use crate::vpn::rule_engine::RuleKind;
    let kind = RuleKind::parse(kind)?;
    if value.trim().is_empty() {
        return Err("匹配值不能为空".to_string());
    }
    match kind {
        RuleKind::IpCidr => {
            crate::vpn::route::normalize_cidr(value)?;
        }
        RuleKind::Domain | RuleKind::DomainSuffix => {
            crate::vpn::split::normalize_domain(value)
                .ok_or_else(|| format!("不是合法域名：{}", value))?;
        }
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

/// 预览一条输入会走哪：只看「域名/网段是否命中走 VPN 的规则」。
///
/// 早期这里按连接匹配（端口、进程都能参与），那是捕获引擎时代的语义；
/// 现在 macOS 走路由注入，能不能进隧道只取决于域名和网段，所以参数也只剩这两个。
#[tauri::command]
pub fn vpn_preview_rules(
    rules: Vec<VpnRule>,
    default_policy: String,
    domain: Option<String>,
    ip: Option<String>,
) -> Result<String, String> {
    let plan = crate::vpn::split::plan(&rules, &default_policy);
    if let Some(domain) = domain.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        let name = domain.trim_end_matches('.').to_ascii_lowercase();
        let hit = plan.domains.iter().any(|suffix| {
            name == *suffix || name.ends_with(&format!(".{}", suffix))
        });
        return Ok(if hit { "VPN" } else { "DIRECT" }.to_string());
    }
    if let Some(ip) = ip.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        let addr: std::net::Ipv4Addr = ip
            .parse()
            .map_err(|_| format!("IP 不合法：{}", ip))?;
        let hit = plan.cidrs.iter().any(|cidr| {
            crate::vpn::route::parse_cidr(cidr)
                .map(|(net, prefix)| {
                    let mask = if prefix == 0 {
                        0
                    } else {
                        u32::MAX << (32 - prefix)
                    };
                    (u32::from(addr) & mask) == net
                })
                .unwrap_or(false)
        });
        return Ok(if hit { "VPN" } else { "DIRECT" }.to_string());
    }
    Err("请提供要预览的域名或 IP".to_string())
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
    session
        .ensure()
        .map_err(|err| format!("{}{}", err, session_dir_diagnosis()))?;
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

/// 一次连接运行期要用的全部策略。
struct ProxyPlan {
    mode: ProxyMode,
    /// 启用中的规则；全局/直连模式下只是随请求带着，不编译也不参与判定
    rules: Vec<VpnRule>,
    default_policy: String,
}

/// 解析代理模式并整理规则。
///
/// 规则始终全量下发：worker 在全局/直连模式下会忽略它们，但用户切回规则模式时
/// 需要立刻能用，不必重连。是否编译校验由调用方按模式决定。
fn resolve_proxy_plan(
    mode: &str,
    rules: Vec<VpnRule>,
    default_policy: String,
) -> Result<ProxyPlan, String> {
    Ok(ProxyPlan {
        mode: ProxyMode::parse(mode)?,
        rules: rules.into_iter().filter(|rule| rule.enabled).collect(),
        default_policy,
    })
}

/// 编译校验一份规则集；任何一条不合法都返回错误。
/// 连接与运行期热更新共用，保证「能存进配置的规则」和「能连上的规则」一致。
fn validate_rule_set(rules: &[VpnRule], default_policy: &str) -> Result<(), String> {
    use crate::vpn::rule_engine::{Policy, RuleKind};
    for r in rules.iter().filter(|rule| rule.enabled) {
        RuleKind::parse(&r.kind)?;
        Policy::parse(&r.policy)?;
        validate_rule_input(&r.kind, &r.value)?;
    }
    Policy::parse(default_policy)?;
    Ok(())
}

/// 会话是否处于「有活动连接」状态
fn session_is_active(session: &VpnSession) -> bool {
    let status = session.read_status().unwrap_or_default();
    !matches!(status.phase, VpnPhase::Disconnected | VpnPhase::Failed)
}

/// 运行期切换代理模式。
///
/// 连接中写 `mode` 文件，worker 200ms 内读到并只改判定开关；
/// 未连接时直接返回 Ok，模式本身由前端持久化在配置里。
#[tauri::command]
pub fn vpn_set_proxy_mode(state: State<'_, VpnState>, mode: String) -> Result<(), String> {
    let mode = ProxyMode::parse(&mode)?;
    let runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    if !session_is_active(&runtime.session) {
        return Ok(());
    }
    runtime.session.write_mode(mode.as_str())
}

/// 运行期推送规则。
///
/// 先编译校验再落盘：校验失败时返回错误，正在运行的旧规则继续生效。
#[tauri::command]
pub fn vpn_push_rules(
    state: State<'_, VpnState>,
    rules: Vec<VpnRule>,
    default_policy: Option<String>,
) -> Result<(), String> {
    let default_policy = default_policy
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "DIRECT".to_string());
    let enabled: Vec<VpnRule> = rules.into_iter().filter(|r| r.enabled).collect();
    validate_rule_set(&enabled, &default_policy)?;
    let runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    if !session_is_active(&runtime.session) {
        return Ok(());
    }
    runtime.session.write_rules(&enabled, &default_policy)
}

/// 同通道切换的三种结论
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwitchDecision {
    /// 已连接且通道身份相同：只换运行期策略，隧道不动
    Shared,
    /// 已连接但通道身份不同：必须先断开重连
    NeedsReconnect,
    /// 当前没有活动连接
    NoSession,
}

fn decide_switch(active: Option<&str>, target: Option<&str>) -> SwitchDecision {
    match (active, target) {
        (None, _) => SwitchDecision::NoSession,
        // 空身份说明状态文件来自旧版本，不能当成同通道
        (Some(current), Some(next)) if !current.is_empty() && current == next => {
            SwitchDecision::Shared
        }
        _ => SwitchDecision::NeedsReconnect,
    }
}

/// 批量计算配置的通道身份，前端据此渲染「共用通道」状态
#[tauri::command]
pub fn vpn_conn_identities(profiles: Vec<VpnProfile>) -> Vec<String> {
    profiles.iter().map(conn_identity).collect()
}

/// 同通道切换：把运行期策略换成这份配置的规则与模式，隧道、网卡、路由一律不动。
///
/// 返回 true 表示已切换；false 表示通道参数不同，需要前端确认后断开重连。
#[tauri::command]
pub fn vpn_switch_profile(
    state: State<'_, VpnState>,
    profile: VpnProfile,
    rules: Vec<VpnRule>,
    default_policy: Option<String>,
) -> Result<bool, String> {
    let mut runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    let session = runtime.session.clone();
    if !session_is_active(&session) {
        return Err("当前未连接".to_string());
    }
    let status = session.read_status().unwrap_or_default();
    let target = conn_identity(&profile);
    match decide_switch(Some(status.conn_identity.as_str()), Some(target.as_str())) {
        SwitchDecision::NoSession => Err("当前未连接".to_string()),
        SwitchDecision::NeedsReconnect => Ok(false),
        SwitchDecision::Shared => {
            let default_policy = default_policy
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "DIRECT".to_string());
            let enabled: Vec<VpnRule> =
                rules.into_iter().filter(|rule| rule.enabled).collect();
            // 校验失败就直接返回错误，正在跑的隧道与旧规则都不受影响
            validate_rule_set(&enabled, &default_policy)?;
            session.write_rules(&enabled, &default_policy)?;
            session.write_mode(&profile.proxy_mode)?;
            runtime.policy_profile_id = Some(profile.id.clone());
            Ok(true)
        }
    }
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
    // 已有活动连接时拒绝再次连接：多个 worker 会抢同一套路由。
    // 同通道切换走 vpn_switch_profile，不需要重建隧道。
    if let Ok(runtime) = state.0.lock() {
        if session_is_active(&runtime.session) {
            return Err("已有活动连接，请先断开当前连接".to_string());
        }
    }
    // 兜底策略由前端从所选规则集透传，缺省或空白时退回直连
    let default_policy = default_policy
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "DIRECT".to_string());
    // 代理模式是连接行为的唯一来源：全局/直连模式会忽略前端传来的规则，
    // 避免界面已经切模式、实际仍按旧规则分流。
    let plan = resolve_proxy_plan(&profile.proxy_mode, rules, default_policy)?;
    let (rules, default_policy) = (plan.rules.clone(), plan.default_policy.clone());
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

    // 3. 规则已经在 resolve_proxy_plan 里按模式处理：规则模式过滤停用项，
    // 全局/直连模式清空规则并覆盖兜底走向。

    // 规则只在规则模式下编译校验：全局/直连用不到规则，不该因为它们不合法连不上。
    // 规则模式下这是 fail-closed —— 宁可连接失败，也不放行一份语义错误的规则集。
    if plan.mode == ProxyMode::Rule {
        validate_rule_set(&rules, &default_policy)?;
    }

    // 先落盘连接请求。持锁的部分只做文件操作，绝不跨过提权那一步。
    let session = {
        let mut runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
        // 启动时如果退到了临时目录，这里再探一次：用户目录当时可能只是被
        // 杀毒/沙箱临时锁住，现在又好了。能探到就切回去，不必让用户重启。
        if let Ok(dir) = session_dir(&app) {
            if dir != runtime.session.dir {
                runtime.session = VpnSession::new(dir);
            }
        }
        let session = runtime.session.clone();
        session
            .prepare()
            .map_err(|err| format!("{}{}", err, session_dir_diagnosis()))?;
        runtime.log_cursor = LogCursor::default();
        runtime.last_status = None;
        runtime.policy_profile_id = Some(profile.id.clone());
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
        proxy_mode: plan.mode.as_str().to_string(),
        conn_identity: conn_identity(&profile),
        rules: rules.clone(),
        default_policy: default_policy.clone(),
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

    // 心跳线程：worker 靠这个文件判断主进程是否还活着。
    // 主进程被强杀时没人来请求停止，worker 会在心跳过期后自己走正常收尾流程。
    {
        let heartbeat = session.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            loop {
                if heartbeat.touch_alive().is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(2));
                match heartbeat.read_status() {
                    Some(status)
                        if matches!(
                            status.phase,
                            VpnPhase::Disconnected | VpnPhase::Failed
                        ) =>
                    {
                        break;
                    }
                    // 启动阶段可能还读不到状态；超过两分钟仍读不到就放弃
                    None if started.elapsed() > Duration::from_secs(120) => break,
                    _ => {}
                }
            }
        });
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
    let (session, _) = {
        let mut runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
        runtime.policy_profile_id = None;
        (runtime.session.clone(), ())
    };

    let status = session.read_status().unwrap_or_default();
    if status.phase == VpnPhase::Disconnected {
        return Ok("当前未连接".to_string());
    }

    elevate::stop_worker(&session);
    // 事件统一发快照形状：前端监听器按 VpnSnapshotPayload 解析
    app.emit(
        EVENT_STATUS,
        VpnSnapshotPayload {
            status: VpnStatus {
                phase: VpnPhase::Disconnecting,
                message: "正在断开并还原路由".to_string(),
                ..status
            },
            elevation: String::new(),
            policy_profile_id: String::new(),
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
    /// 运行期「当前生效配置」；为空时以 status.profile_id 为准
    pub policy_profile_id: String,
}

#[tauri::command]
pub fn vpn_get_snapshot(state: State<'_, VpnState>) -> VpnSnapshotPayload {
    let runtime = match state.0.lock() {
        Ok(runtime) => runtime,
        Err(_) => {
            return VpnSnapshotPayload {
                status: VpnStatus::default(),
                elevation: String::new(),
                policy_profile_id: String::new(),
            }
        }
    };
    let status = runtime.session.read_status().unwrap_or_default();
    let policy_profile_id = runtime
        .policy_profile_id
        .clone()
        .unwrap_or_else(|| status.profile_id.clone());
    VpnSnapshotPayload {
        status,
        elevation: runtime.elevation.clone(),
        policy_profile_id,
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

            let (status, elevation, lines, policy_owner) = {
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
                    runtime.policy_profile_id.clone(),
                )
            };

            if let Some(status) = status {
                let policy_profile_id =
                    policy_owner.unwrap_or_else(|| status.profile_id.clone());
                let json = serde_json::to_string(&status).unwrap_or_default();
                if json != last_status_json {
                    last_status_json = json;
                    let _ = app.emit(
                        EVENT_STATUS,
                        VpnSnapshotPayload {
                            status: status.clone(),
                            elevation: elevation.clone(),
                            policy_profile_id: policy_profile_id.clone(),
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
        // 托盘退出、Cmd+Q、菜单退出都走这里：别把控制文件留给下次
        session.remove_control_files();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_the_three_live_kinds() {
        assert!(validate_rule_input("DOMAIN", "oa.example.com").is_ok());
        assert!(validate_rule_input("DOMAIN-SUFFIX", "example.com").is_ok());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/8").is_ok());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/33").is_err());
        assert!(validate_rule_input("DOMAIN", "").is_err());
        assert!(validate_rule_input("bogus", "x").is_err());
        // 退役的三种类型必须明确报错，而不是静默通过
        for kind in ["DOMAIN-KEYWORD", "PROCESS-NAME", "DST-PORT", "RULE-SET"] {
            assert!(
                validate_rule_input(kind, "443").is_err(),
                "{} 应该被拒绝",
                kind
            );
        }
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

    fn sample_rule(enabled: bool) -> VpnRule {
        VpnRule {
            id: "r1".to_string(),
            kind: "DOMAIN".to_string(),
            value: "example.com".to_string(),
            policy: "VPN".to_string(),
            remark: String::new(),
            enabled,
        }
    }

    #[test]
    fn rule_mode_keeps_enabled_rules_and_default() {
        let plan = resolve_proxy_plan(
            "rule",
            vec![sample_rule(true), sample_rule(false)],
            "DIRECT".to_string(),
        )
        .unwrap();
        assert_eq!(plan.mode, ProxyMode::Rule);
        assert_eq!(plan.rules.len(), 1);
        assert_eq!(plan.default_policy, "DIRECT");
    }

    #[test]
    fn global_mode_keeps_rules_but_switches_mode() {
        let plan = resolve_proxy_plan(
            "global",
            vec![sample_rule(true)],
            "DIRECT".to_string(),
        )
        .unwrap();
        assert_eq!(plan.mode, ProxyMode::Global);
        assert_eq!(plan.rules.len(), 1);
    }

    #[test]
    fn direct_mode_keeps_rules_but_switches_mode() {
        let plan = resolve_proxy_plan(
            "direct",
            vec![sample_rule(true)],
            "DIRECT".to_string(),
        )
        .unwrap();
        assert_eq!(plan.mode, ProxyMode::Direct);
        assert_eq!(plan.rules.len(), 1);
    }

    #[test]
    fn unknown_proxy_mode_is_rejected() {
        assert!(resolve_proxy_plan("bogus", Vec::new(), "DIRECT".to_string()).is_err());
    }

    #[test]
    fn switch_decision_matches_channel_identity() {
        assert_eq!(decide_switch(Some("abc"), Some("abc")), SwitchDecision::Shared);
        assert_eq!(
            decide_switch(Some("abc"), Some("def")),
            SwitchDecision::NeedsReconnect
        );
        assert_eq!(decide_switch(None, Some("abc")), SwitchDecision::NoSession);
        // 旧状态文件里没有身份时按「需要重连」处理，不能当成同通道
        assert_eq!(
            decide_switch(Some(""), Some("abc")),
            SwitchDecision::NeedsReconnect
        );
    }
}
