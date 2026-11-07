//! 「弹窗拦截」页签（仅 Windows）：检测 + 可选拦截两种模式。
//!
//! 设计见 docs/superpowers/specs/2026-11-07-system-tools-design.md 4.3。
//!
//! 实现要点：
//! - **检测模式（默认）**：只记录 + 推事件，关闭/隐藏必须由用户在列表里逐条点。
//! - **拦截模式（可选，默认关）**：命中打分闸门后自动关闭/隐藏。闸门是纯函数
//!   `should_intercept`：分数阈值、至少两条命中原因、进程/类名白名单（系统进程与
//!   任务栏等硬保护）、忽略名单、同进程 5 秒冷却、每分钟自动处理上限（熔断）。
//!   设置存 `<data_dir>/popup-settings.json`，界面上随时可关。
//! - 检测用 `EnumWindows` 轮询（约 800ms 一轮，和火绒的定时器 + 枚举思路一致），
//!   比 WinEvent hook 的 API 面小、不会被 UIPI 挡掉事件；代价是窗口会先闪一下。
//! - 识别启发式：`WS_EX_TOOLWINDOW` / `WS_EX_NOACTIVATE` / `WS_EX_TOPMOST`、
//!   无标题、尺寸异常、无 owner、非前台进程、常见广告类名线索等，只用来打分排序。
//! - 忽略名单按进程名存 `<data_dir>/popup-ignore.json`，不动 AppConfig。
//!
//! 能力边界（UI 里也写了）：管理员权限窗口关不掉（UIPI）、UAC/锁屏/内嵌广告看不到、
//! 不注入、不驱动、不杀进程。

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Default)]
pub struct PopupStatus {
    pub supported: bool,
    pub running: bool,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopupHit {
    pub id: u64,
    pub at_ms: u64,
    pub process: String,
    pub title: String,
    pub class_name: String,
    pub width: i32,
    pub height: i32,
    pub score: u32,
    pub reasons: Vec<String>,
    /// 拦截模式自动处理动作：None 表示只记录；Some("close") / Some("hide") 表示已自动处理
    pub auto_action: Option<String>,
}

/// 判定窗口「可疑」用的特征。抽成纯结构，方便单测打分逻辑。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub struct WindowFeatures {
    pub has_title: bool,
    pub is_tool_window: bool,
    pub is_no_activate: bool,
    pub is_topmost: bool,
    /// 是否有关联 owner 窗口（暂不参与打分，保留给后续规则用）
    pub is_foreground_process: bool,
    pub width: i32,
    pub height: i32,
    pub class_name: String,
    pub title: String,
    pub process: String,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl WindowFeatures {
    pub fn display_title(&self) -> String {
        const MAX: usize = 120;
        let mut title = self.title.clone();
        if title.chars().count() > MAX {
            title = title.chars().take(MAX).collect();
            title.push('…');
        }
        title
    }
}

/// 拦截模式设置（持久化到 `<data_dir>/popup-settings.json`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PopupSettings {
    /// 是否开启拦截模式（默认关：只检测不动手）
    pub intercept: bool,
    /// 自动处理动作：`close`（发 WM_CLOSE）或 `hide`（SW_HIDE）
    pub action: String,
    /// 自动处理的最低可疑分
    pub min_score: u32,
    /// 每分钟最多自动处理几个窗口（熔断，防止误杀连环触发）
    pub max_per_minute: u32,
}

impl Default for PopupSettings {
    fn default() -> Self {
        Self {
            intercept: false,
            action: "close".into(),
            min_score: 8,
            max_per_minute: 6,
        }
    }
}

impl PopupSettings {
    /// 把外部传进来的值收进安全范围（前端/手工改 JSON 都可能越界）
    pub fn normalized(mut self) -> Self {
        self.action = match self.action.as_str() {
            "hide" => "hide".into(),
            _ => "close".into(),
        };
        self.min_score = self.min_score.clamp(5, 30);
        self.max_per_minute = self.max_per_minute.clamp(1, 30);
        self
    }
}

/// 永远不自动处理的进程：系统外壳 / 安全 / 输入法 / 本应用自己。
/// 命中这些进程只会记录，绝不自动关闭。
const PROTECTED_PROCESSES: [&str; 22] = [
    "explorer.exe",
    "dwm.exe",
    "winlogon.exe",
    "csrss.exe",
    "smss.exe",
    "lsass.exe",
    "svchost.exe",
    "consent.exe",
    "taskmgr.exe",
    "lockapp.exe",
    "logonui.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "textinputhost.exe",
    "applicationframehost.exe",
    "ctfmon.exe",
    "sihost.exe",
    "runtimebroker.exe",
    "securityhealthsystray.exe",
    "systemsettings.exe",
    "ws-workbench.exe",
];

/// 永远不自动处理的窗口类：桌面 / 任务栏 / 托盘 / 开始菜单等外壳窗口。
const PROTECTED_CLASSES: [&str; 8] = [
    "progman",
    "workerw",
    "shell_traywnd",
    "shell_secondarytraywnd",
    "notifyiconoverflowwindow",
    "trayclockwclass",
    "tasklistthumbbar",
    "windows.ui.core.corewindow",
];

fn is_protected_process(process: &str) -> bool {
    let lower = process.to_ascii_lowercase();
    if PROTECTED_PROCESSES
        .iter()
        .any(|protected| protected.eq_ignore_ascii_case(&lower))
    {
        return true;
    }
    // 本应用自己的进程名可能带平台后缀，按当前 exe 名兜底比较
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().to_ascii_lowercase()))
        .is_some_and(|own| own == lower)
}

fn is_protected_class(class_name: &str) -> bool {
    let lower = class_name.to_ascii_lowercase();
    PROTECTED_CLASSES
        .iter()
        .any(|protected| lower == *protected || lower.starts_with(protected))
}

/// 拦截模式的判定结果：要动窗口时带上动作，否则给不处理的原因（只用于调试/测试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterceptPlan {
    Intercept(&'static str),
    Skip(&'static str),
}

/// 拦截模式的纯逻辑闸门：设置开着、分数够、命中原因够多、没被保护、没被忽略、
/// 没超过每分钟预算时，才允许自动处理。
pub fn should_intercept(
    features: &WindowFeatures,
    score: u32,
    reasons: &[String],
    settings: &PopupSettings,
    ignored_processes: &[String],
    actions_this_minute: u32,
) -> InterceptPlan {
    if !settings.intercept {
        return InterceptPlan::Skip("拦截模式未开启");
    }
    if score < settings.min_score {
        return InterceptPlan::Skip("可疑分未达阈值");
    }
    if reasons.len() < 2 {
        return InterceptPlan::Skip("命中原因不足两条");
    }
    if is_protected_process(&features.process) || is_protected_class(&features.class_name) {
        return InterceptPlan::Skip("系统窗口或受保护进程");
    }
    if ignored_processes
        .iter()
        .any(|process| process.eq_ignore_ascii_case(&features.process))
    {
        return InterceptPlan::Skip("进程在忽略名单里");
    }
    if actions_this_minute >= settings.max_per_minute {
        return InterceptPlan::Skip("已达每分钟自动处理上限");
    }
    InterceptPlan::Intercept(if settings.action == "hide" {
        "hide"
    } else {
        "close"
    })
}

/// 广告弹窗的类名/标题线索（命中加分，不是硬规则）
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const AD_CLASS_HINTS: [&str; 10] = [
    "popup",
    "advert",
    "ad_",
    "ads_",
    "float",
    "news",
    "activity",
    "promo",
    "notice",
    "recommend",
];

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const AD_TITLE_HINTS: [&str; 8] = [
    "广告",
    "活动",
    "推荐",
    "福利",
    "红包",
    "优惠",
    "抽奖",
    "弹窗",
];

/// 给窗口打分：分数只用于排序与「可疑」标记，不触发任何自动动作。
///
/// 返回 (score, reasons)，score 越大越可疑；0 分表示没有命中任何可疑特征。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn score_window(features: &WindowFeatures) -> (u32, Vec<String>) {
    let mut score: u32 = 0;
    let mut reasons: Vec<String> = Vec::new();

    if !features.has_title {
        score += 2;
        reasons.push("没有窗口标题".into());
    }
    if features.is_tool_window {
        score += 3;
        reasons.push("工具窗口样式（WS_EX_TOOLWINDOW）".into());
    }
    if features.is_no_activate {
        score += 2;
        reasons.push("不抢焦点（WS_EX_NOACTIVATE）".into());
    }
    if features.is_topmost {
        score += 1;
        reasons.push("始终置顶".into());
    }
    if !features.is_foreground_process {
        // 非前台进程自己弹出来的窗口更可疑；前台进程的正常窗口不加分
        score += 1;
        reasons.push("弹窗进程不在前台".into());
    }
    if features.width <= 0 || features.height <= 0 {
        score += 2;
        reasons.push("窗口尺寸无效".into());
    } else if features.width < 260 || features.height < 140 {
        score += 2;
        reasons.push(format!("尺寸偏小（{}×{}）", features.width, features.height));
    }
    let class_lower = features.class_name.to_ascii_lowercase();
    if let Some(hint) = AD_CLASS_HINTS
        .iter()
        .find(|hint| class_lower.contains(*hint))
    {
        score += 3;
        reasons.push(format!("类名含「{hint}」"));
    }
    let title_lower = features.title.to_lowercase();
    if let Some(hint) = AD_TITLE_HINTS
        .iter()
        .find(|hint| title_lower.contains(*hint))
    {
        score += 2;
        reasons.push(format!("标题含「{hint}」"));
    }
    (score, reasons)
}

/// 检测线程的公共句柄，Windows 实现填具体字段，其它平台保持空。
#[derive(Default)]
struct Manager {
    #[cfg(target_os = "windows")]
    inner: Option<imp::Inner>,
    #[cfg(not(target_os = "windows"))]
    supported: bool,
}

static MANAGER: Mutex<Manager> = Mutex::new(Manager {
    #[cfg(target_os = "windows")]
    inner: None,
    #[cfg(not(target_os = "windows"))]
    supported: false,
});

pub fn status() -> PopupStatus {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    {
        let running = manager.inner.is_some();
        let count = manager
            .inner
            .as_ref()
            .map(|inner| inner.buffer.lock().map(|buffer| buffer.len()).unwrap_or(0))
            .unwrap_or(0);
        PopupStatus {
            supported: true,
            running,
            count,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &manager.supported;
        PopupStatus {
            supported: false,
            running: false,
            count: 0,
        }
    }
}

pub fn recent() -> Vec<PopupHit> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    {
        manager
            .inner
            .as_ref()
            .and_then(|inner| inner.buffer.lock().ok().map(|buffer| buffer.iter().cloned().collect()))
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &manager;
        Vec::new()
    }
}

pub fn clear() {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    if let Some(inner) = manager.inner.as_ref() {
        if let Ok(mut buffer) = inner.buffer.lock() {
            buffer.clear();
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &manager;
}

#[cfg(target_os = "windows")]
pub fn start(app: tauri::AppHandle) -> Result<PopupStatus, String> {
    let mut manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    if manager.inner.is_some() {
        return Ok(status());
    }
    manager.inner = Some(imp::spawn(app)?);
    Ok(status())
}

#[cfg(not(target_os = "windows"))]
pub fn start(_app: tauri::AppHandle) -> Result<PopupStatus, String> {
    Err("仅 Windows 支持弹窗检测".into())
}

pub fn stop() -> Result<PopupStatus, String> {
    let mut manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    if let Some(inner) = manager.inner.take() {
        inner.stop();
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &mut manager;
    Ok(status())
}

#[cfg(target_os = "windows")]
pub fn dismiss(id: u64, action: &str) -> Result<(), String> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let inner = manager
        .inner
        .as_ref()
        .ok_or_else(|| "弹窗检测未开启".to_string())?;
    inner.dismiss(id, action)
}

#[cfg(not(target_os = "windows"))]
pub fn dismiss(_id: u64, _action: &str) -> Result<(), String> {
    Err("仅 Windows 支持弹窗检测".into())
}

/// 读取/更新忽略名单（按进程名），并持久化到应用数据目录。
pub fn ignore(process: &str, ignored: bool) -> Result<Vec<String>, String> {
    let data_dir = data_dir()?;
    let mut list = imp::load_ignore(&data_dir);
    let needle = process.to_ascii_lowercase();
    list.retain(|item| item.to_ascii_lowercase() != needle);
    if ignored && !process.trim().is_empty() {
        list.push(process.to_string());
        list.sort();
        list.dedup();
    }
    imp::save_ignore(&data_dir, &list)?;
    Ok(list)
}

/// 读拦截模式设置：检测线程在跑就用线程里的那份，否则读磁盘（失败退回默认值）。
pub fn settings_get() -> PopupSettings {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    if let Some(inner) = manager.inner.as_ref() {
        return inner
            .settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default();
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &manager;
    drop(manager);
    load_settings().unwrap_or_default()
}

/// 写拦截模式设置：先落盘，再同步给正在跑的检测线程。
pub fn settings_set(settings: PopupSettings) -> Result<PopupSettings, String> {
    let normalized = settings.normalized();
    save_settings(&normalized)?;
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    if let Some(inner) = manager.inner.as_ref() {
        if let Ok(mut current) = inner.settings.lock() {
            *current = normalized.clone();
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &manager;
    Ok(normalized)
}

fn settings_path() -> Result<std::path::PathBuf, String> {
    Ok(data_dir()?.join("popup-settings.json"))
}

fn load_settings() -> Result<PopupSettings, String> {
    let path = settings_path()?;
    let raw = std::fs::read_to_string(&path).map_err(|err| format!("读取弹窗设置失败：{err}"))?;
    serde_json::from_str::<PopupSettings>(&raw)
        .map(PopupSettings::normalized)
        .map_err(|err| format!("解析弹窗设置失败：{err}"))
}

fn save_settings(settings: &PopupSettings) -> Result<(), String> {
    let path = settings_path()?;
    let raw = serde_json::to_string_pretty(settings).map_err(|err| format!("序列化弹窗设置失败：{err}"))?;
    std::fs::write(&path, raw).map_err(|err| format!("写入弹窗设置失败：{err}"))
}

fn data_dir() -> Result<std::path::PathBuf, String> {
    let base = std::env::var_os("WS_WORKBENCH_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            let path = std::path::PathBuf::from(home);
            if cfg!(target_os = "windows") {
                Some(path.join("AppData").join("Roaming").join("ws-workbench"))
            } else {
                Some(
                    path.join("Library")
                        .join("Application Support")
                        .join("ws-workbench"),
                )
            }
        })
        .ok_or_else(|| "取不到应用数据目录".to_string())?;
    std::fs::create_dir_all(&base).map_err(|err| format!("创建数据目录失败：{err}"))?;
    Ok(base)
}

// ============================== 平台实现 ==============================

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use windows::core::BOOL;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect,
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, ShowWindow,
        GWL_EXSTYLE, SW_HIDE, WM_CLOSE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST,
    };

    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(800);
    const BUFFER_LIMIT: usize = 200;
    const DISMISS_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);

    pub struct Inner {
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
        pub buffer: Arc<Mutex<VecDeque<PopupHit>>>,
        handles: Arc<Mutex<HashMap<u64, isize>>>,
        last_dismiss: Arc<Mutex<HashMap<String, std::time::Instant>>>,
        pub settings: Arc<Mutex<PopupSettings>>,
    }

    impl Inner {
        pub fn stop(mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }

        pub fn dismiss(&self, id: u64, action: &str) -> Result<(), String> {
            let handle = self
                .handles
                .lock()
                .map_err(|_| "窗口句柄表损坏".to_string())?
                .get(&id)
                .copied()
                .ok_or_else(|| "窗口已关闭".to_string())?;
            let process = self
                .buffer
                .lock()
                .ok()
                .and_then(|buffer| buffer.iter().find(|hit| hit.id == id).map(|hit| hit.process.clone()))
                .unwrap_or_default();
            {
                let mut last = self.last_dismiss.lock().map_err(|_| "限流表损坏".to_string())?;
                if let Some(at) = last.get(&process) {
                    if at.elapsed() < DISMISS_COOLDOWN {
                        return Err("同一进程 5 秒内只处理一次".into());
                    }
                }
                last.insert(process, std::time::Instant::now());
            }
            apply_window_action(handle, action);
            Ok(())
        }
    }

    /// 对窗口执行动作：close 走 WM_CLOSE（正常关闭流程），hide 只是 SW_HIDE
    fn apply_window_action(handle: isize, action: &str) {
        let hwnd = HWND(handle as *mut _);
        unsafe {
            if action == "hide" {
                let _ = ShowWindow(hwnd, SW_HIDE);
            } else {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn spawn(app: tauri::AppHandle) -> Result<Inner, String> {
        use tauri::Emitter;

        let buffer = Arc::new(Mutex::new(VecDeque::<PopupHit>::new()));
        let handles = Arc::new(Mutex::new(HashMap::<u64, isize>::new()));
        let last_dismiss = Arc::new(Mutex::new(HashMap::<String, std::time::Instant>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let data_dir = super::data_dir()?;
        let ignore = Arc::new(Mutex::new(load_ignore(&data_dir)));
        let settings = Arc::new(Mutex::new(super::load_settings().unwrap_or_default()));

        let thread_buffer = Arc::clone(&buffer);
        let thread_handles = Arc::clone(&handles);
        let thread_stop = Arc::clone(&stop);
        let thread_ignore = Arc::clone(&ignore);
        let thread_settings = Arc::clone(&settings);
        let thread_last_dismiss = Arc::clone(&last_dismiss);
        let join = std::thread::Builder::new()
            .name("popup-watch".into())
            .spawn(move || {
                let mut next_id: u64 = 1;
                let mut known: HashMap<isize, String> = HashMap::new();
                let mut minute_start = std::time::Instant::now();
                let mut minute_actions: u32 = 0;
                while !thread_stop.load(Ordering::SeqCst) {
                    let foreground = unsafe { GetForegroundWindow() };
                    let ignored = thread_ignore.lock().map(|list| list.clone()).unwrap_or_default();
                    let settings = thread_settings
                        .lock()
                        .map(|value| value.clone())
                        .unwrap_or_default();
                    if minute_start.elapsed() >= std::time::Duration::from_secs(60) {
                        minute_start = std::time::Instant::now();
                        minute_actions = 0;
                    }
                    for (features, handle) in collect_windows(foreground) {
                        if ignored
                            .iter()
                            .any(|process| process.eq_ignore_ascii_case(&features.process))
                        {
                            continue;
                        }
                        let (score, reasons) = score_window(&features);
                        if score == 0 || reasons.is_empty() {
                            continue;
                        }
                        let signature = format!("{}|{}", features.class_name, features.title);
                        if known.get(&handle).is_some_and(|prev| prev == &signature) {
                            continue;
                        }
                        known.insert(handle, signature);
                        let mut hit = PopupHit {
                            id: next_id,
                            at_ms: now_ms(),
                            process: features.process.clone(),
                            title: features.display_title(),
                            class_name: features.class_name.clone(),
                            width: features.width,
                            height: features.height,
                            score,
                            reasons,
                            auto_action: None,
                        };
                        next_id += 1;

                        // 拦截模式：闸门 + 同进程冷却 + 每分钟熔断（检测模式不会走到这里）
                        let mut cooldown_ok = true;
                        if let Ok(mut last) = thread_last_dismiss.lock() {
                            if let Some(at) = last.get(&features.process) {
                                if at.elapsed() < DISMISS_COOLDOWN {
                                    cooldown_ok = false;
                                }
                            }
                            if cooldown_ok {
                                last.insert(features.process.clone(), std::time::Instant::now());
                            }
                        }
                        if cooldown_ok {
                            if let InterceptPlan::Intercept(action) = should_intercept(
                                &features,
                                hit.score,
                                &hit.reasons,
                                &settings,
                                &ignored,
                                minute_actions,
                            ) {
                                apply_window_action(handle, action);
                                minute_actions += 1;
                                hit.auto_action = Some(action.to_string());
                            }
                        }
                        if let Ok(mut buffer) = thread_buffer.lock() {
                            if buffer.len() >= BUFFER_LIMIT {
                                buffer.pop_front();
                            }
                            buffer.push_back(hit.clone());
                        }
                        if let Ok(mut map) = thread_handles.lock() {
                            map.insert(hit.id, handle);
                        }
                        let _ = app.emit("popup-detected", &hit);
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
            })
            .map_err(|err| format!("启动弹窗检测线程失败：{err}"))?;

        Ok(Inner {
            stop,
            join: Some(join),
            buffer,
            handles,
            last_dismiss,
            settings,
        })
    }

    /// 枚举所有可见顶层窗口，抓取打分需要的特征（带窗口句柄，供用户手动关闭时用）
    fn collect_windows(foreground: HWND) -> Vec<(WindowFeatures, isize)> {
        struct Collect {
            foreground: HWND,
            items: Vec<(WindowFeatures, isize)>,
        }
        unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let collect = &mut *(lparam.0 as *mut Collect);
            if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                return BOOL(1);
            }
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid == 0 {
                return BOOL(1);
            }
            let mut title_buf = [0u16; 512];
            let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) }.max(0) as usize;
            let mut class_buf = [0u16; 256];
            let class_len = unsafe { GetClassNameW(hwnd, &mut class_buf) }.max(0) as usize;
            let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
            let mut rect = RECT::default();
            let has_rect = unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok();
            let foreground_pid = {
                let mut fg_pid = 0u32;
                unsafe { GetWindowThreadProcessId(collect.foreground, Some(&mut fg_pid)) };
                fg_pid
            };
            let features = WindowFeatures {
                has_title: title_len > 0,
                is_tool_window: (ex_style & WS_EX_TOOLWINDOW.0 as i32) != 0,
                is_no_activate: (ex_style & WS_EX_NOACTIVATE.0 as i32) != 0,
                is_topmost: (ex_style & WS_EX_TOPMOST.0 as i32) != 0,
                is_foreground_process: pid == foreground_pid,
                width: if has_rect { rect.right - rect.left } else { 0 },
                height: if has_rect { rect.bottom - rect.top } else { 0 },
                class_name: String::from_utf16_lossy(&class_buf[..class_len.min(class_buf.len())]),
                title: String::from_utf16_lossy(&title_buf[..title_len.min(title_buf.len())]),
                process: process_name(pid),
            };
            collect.items.push((features, hwnd.0 as isize));
            BOOL(1)
        }

        let mut collect = Collect {
            foreground,
            items: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(
                Some(callback),
                LPARAM(&mut collect as *mut Collect as isize),
            );
        }
        collect.items
    }

    fn process_name(pid: u32) -> String {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return format!("pid {pid}");
            };
            let mut buffer = [0u16; 1024];
            let mut size = buffer.len() as u32;
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buffer.as_mut_ptr()),
                &mut size,
            );
            let _ = CloseHandle(process);
            if result.is_err() || size == 0 {
                return format!("pid {pid}");
            }
            let path = String::from_utf16_lossy(&buffer[..size as usize]);
            path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string()
        }
    }

    pub fn load_ignore(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("popup-ignore.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save_ignore(dir: &std::path::Path, list: &[String]) -> Result<(), String> {
        let path = dir.join("popup-ignore.json");
        let raw = serde_json::to_string_pretty(list).map_err(|err| format!("序列化忽略名单失败：{err}"))?;
        std::fs::write(&path, raw).map_err(|err| format!("写入忽略名单失败：{err}"))
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub fn load_ignore(_dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(_dir.join("popup-ignore.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save_ignore(dir: &std::path::Path, list: &[String]) -> Result<(), String> {
        let path = dir.join("popup-ignore.json");
        let raw =
            serde_json::to_string_pretty(list).map_err(|err| format!("序列化忽略名单失败：{err}"))?;
        std::fs::write(&path, raw).map_err(|err| format!("写入忽略名单失败：{err}"))
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> WindowFeatures {
        WindowFeatures {
            has_title: true,
            width: 600,
            height: 400,
            class_name: "Chrome_WidgetWin_1".into(),
            title: "正常窗口".into(),
            process: "chrome.exe".into(),
            is_foreground_process: true,
            ..Default::default()
        }
    }

    #[test]
    fn normal_window_scores_zero() {
        let (score, reasons) = score_window(&base());
        assert_eq!(score, 0);
        assert!(reasons.is_empty());
    }

    #[test]
    fn tool_window_without_title_scores_high() {
        let mut features = base();
        features.has_title = false;
        features.is_tool_window = true;
        features.is_no_activate = true;
        let (score, reasons) = score_window(&features);
        assert!(score >= 7, "score={score}");
        assert!(reasons.iter().any(|reason| reason.contains("标题")));
        assert!(reasons.iter().any(|reason| reason.contains("TOOLWINDOW")));
    }

    #[test]
    fn ad_hints_add_score() {
        let mut features = base();
        features.class_name = "AdvertPopupWnd".into();
        features.title = "限时活动弹窗".into();
        let (score, reasons) = score_window(&features);
        assert!(score >= 5, "score={score}");
        assert!(reasons.iter().any(|reason| reason.contains("类名")));
        assert!(reasons.iter().any(|reason| reason.contains("标题")));
    }

    #[test]
    fn small_window_is_suspicious() {
        let mut features = base();
        features.width = 180;
        features.height = 90;
        let (score, reasons) = score_window(&features);
        assert!(score >= 2);
        assert!(reasons.iter().any(|reason| reason.contains("尺寸")));
    }

    fn features_for_gate() -> WindowFeatures {
        let mut features = base();
        features.has_title = false;
        features.is_tool_window = true;
        features.is_foreground_process = false;
        features.title = "限时活动弹窗".into();
        features.class_name = "AdvertPopupWnd".into();
        features.process = "PotPlayerMini64.exe".into();
        features
    }

    fn intercept_settings() -> PopupSettings {
        PopupSettings {
            intercept: true,
            action: "close".into(),
            min_score: 8,
            max_per_minute: 6,
        }
    }

    #[test]
    fn gate_is_closed_when_intercept_disabled() {
        let features = features_for_gate();
        let (score, reasons) = score_window(&features);
        let mut settings = intercept_settings();
        settings.intercept = false;
        assert_eq!(
            should_intercept(&features, score, &reasons, &settings, &[], 0),
            InterceptPlan::Skip("拦截模式未开启")
        );
    }

    #[test]
    fn gate_opens_for_high_score_window() {
        let features = features_for_gate();
        let (score, reasons) = score_window(&features);
        assert!(score >= 8, "score={score}");
        assert_eq!(
            should_intercept(&features, score, &reasons, &intercept_settings(), &[], 0),
            InterceptPlan::Intercept("close")
        );
    }

    #[test]
    fn hide_action_is_reported() {
        let features = features_for_gate();
        let (score, reasons) = score_window(&features);
        let mut settings = intercept_settings();
        settings.action = "hide".into();
        assert_eq!(
            should_intercept(&features, score, &reasons, &settings, &[], 0),
            InterceptPlan::Intercept("hide")
        );
    }

    #[test]
    fn gate_skips_low_score() {
        let features = base();
        let (score, reasons) = score_window(&features);
        assert_eq!(
            should_intercept(&features, score, &reasons, &intercept_settings(), &[], 0),
            InterceptPlan::Skip("可疑分未达阈值")
        );
    }

    #[test]
    fn gate_skips_when_reasons_too_few() {
        let features = features_for_gate();
        assert_eq!(
            should_intercept(&features, 20, &["只有一个原因".to_string()], &intercept_settings(), &[], 0),
            InterceptPlan::Skip("命中原因不足两条")
        );
    }

    #[test]
    fn gate_protects_system_processes_and_shell_classes() {
        let mut features = features_for_gate();
        let (_, reasons) = score_window(&features);

        features.process = "explorer.exe".into();
        assert_eq!(
            should_intercept(&features, 20, &reasons, &intercept_settings(), &[], 0),
            InterceptPlan::Skip("系统窗口或受保护进程")
        );

        let mut shell = features_for_gate();
        shell.process = "PotPlayerMini64.exe".into();
        shell.class_name = "Shell_TrayWnd".into();
        assert_eq!(
            should_intercept(&shell, 20, &reasons, &intercept_settings(), &[], 0),
            InterceptPlan::Skip("系统窗口或受保护进程")
        );
    }

    #[test]
    fn gate_respects_ignore_list_and_minute_budget() {
        let features = features_for_gate();
        let (score, reasons) = score_window(&features);
        assert_eq!(
            should_intercept(
                &features,
                score,
                &reasons,
                &intercept_settings(),
                &["potplayermini64.exe".to_string()],
                0
            ),
            InterceptPlan::Skip("进程在忽略名单里")
        );
        assert_eq!(
            should_intercept(&features, score, &reasons, &intercept_settings(), &[], 6),
            InterceptPlan::Skip("已达每分钟自动处理上限")
        );
    }

    #[test]
    fn settings_are_clamped_and_action_normalized() {
        let settings = PopupSettings {
            intercept: true,
            action: "whatever".into(),
            min_score: 0,
            max_per_minute: 999,
        }
        .normalized();
        assert_eq!(settings.action, "close");
        assert_eq!(settings.min_score, 5);
        assert_eq!(settings.max_per_minute, 30);

        let settings = PopupSettings {
            intercept: false,
            action: "HIDE".into(),
            min_score: 100,
            max_per_minute: 0,
        }
        .normalized();
        assert_eq!(settings.action, "close", "大小写不匹配时回退到 close");
        assert_eq!(settings.min_score, 30);
        assert_eq!(settings.max_per_minute, 1);
    }

    #[test]
    fn default_settings_are_detection_only() {
        let settings = PopupSettings::default();
        assert!(!settings.intercept, "默认必须是只检测");
        assert_eq!(settings.action, "close");
        assert_eq!(settings.min_score, 8);
        assert_eq!(settings.max_per_minute, 6);
    }

    #[test]
    fn background_process_adds_score() {
        let mut features = base();
        features.is_tool_window = true;
        let (foreground_score, _) = score_window(&features);

        features.is_foreground_process = false;
        let (background_score, reasons) = score_window(&features);
        assert!(background_score > foreground_score);
        assert!(reasons.iter().any(|reason| reason.contains("不在前台")));
    }
}
