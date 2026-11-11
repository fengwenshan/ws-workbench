//! 「弹窗拦截」页签（仅 Windows）：规则拦截 + 启发式检测。
//!
//! 交互参照火绒的弹窗拦截：用户对某个弹窗点一次「拦截」，就落一条**持久化规则**
//! （进程 / 窗口类 / 标题），以后同类的弹窗一出现就被自动关掉；规则可以随时
//! 停用、删除，也能从「正在运行的窗口」里直接挑一个加规则。没有命中规则的窗口
//! 走启发式打分，只进入「可疑记录」等用户处理（可选开启「智能拦截」再自动动手）。
//!
//! 实现要点：
//! - 检测用 `EnumWindows` 轮询（约 500ms 一轮，和火绒的定时器 + 枚举思路一致），
//!   比 WinEvent hook 的 API 面小、不会被 UIPI 挡掉事件；代价是窗口会先闪一下。
//! - 规则匹配是纯函数 `rule_matches`：进程必须一致；规则里写了窗口类就比较类名，
//!   写了标题就比较标题子串；两者都空表示「该程序的所有弹窗」。
//! - 规则先匹配、后打分：命中规则直接 `WM_CLOSE` / `SW_HIDE` 并记一条拦截记录，
//!   不进可疑列表。同一个窗口句柄最多每 5 秒动一次，避免无视关闭的窗口被反复处理。
//! - 规则存 `<data_dir>/popup-rules.json`，命中次数懒落盘（检测线程最多 10 秒刷一次），
//!   设置存 `popup-settings.json`，忽略名单（白名单）存 `popup-ignore.json`。
//! - **硬保护**：系统外壳进程与窗口类（explorer / 任务栏 / 开始菜单 / 输入法 / 本应用）
//!   既不能被规则命中，也不能被智能拦截；规则新增时也直接拒绝这类进程。
//! - 能力边界（UI 里也写了）：管理员权限窗口关不掉（UIPI）、UAC / 锁屏 / 网页内嵌广告
//!   看不到；不注入进程、不装驱动、不结束进程。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};

/// 记录缓冲上限：拦截记录 + 当前可疑窗口
const HIT_LIMIT: usize = 300;
/// 同一个窗口句柄两次动作之间的最小间隔
const RETRY_AFTER_MS: u64 = 5_000;

#[derive(Debug, Clone, Serialize, Default)]
pub struct PopupStatus {
    pub supported: bool,
    pub running: bool,
    /// 缓冲里的记录总数
    pub count: usize,
    /// 其中已自动拦截（命中规则 / 智能拦截）的条数
    pub blocked: usize,
}

/// 一条弹窗记录：要么是待处理的「可疑」，要么是已经处理掉的「拦截」。
#[derive(Debug, Clone, Serialize, Default)]
pub struct PopupHit {
    pub id: u64,
    pub at_ms: u64,
    /// 进程名，如 `wps.exe`
    pub process: String,
    /// 进程完整路径（取图标 / 加规则用；权限不足时可能为空）
    pub path: String,
    /// 窗口标题（已截断到 120 字符）
    pub title: String,
    /// 窗口类名
    pub class_name: String,
    pub width: i32,
    pub height: i32,
    /// 相对屏幕的位置（右下窗口 / 中间窗口 / 任意位置）
    pub position: String,
    /// 可疑度分值（命中规则时为 0）
    pub score: u32,
    /// 命中原因，直接展示
    pub reasons: Vec<String>,
    /// 已自动处理的动作：`None` 表示还在等用户处理
    pub auto_action: Option<String>,
    /// 命中的规则 id（智能拦截 / 纯检测时为空）
    pub rule_id: Option<String>,
    /// 记录来源：`rule`（规则拦截）/ `smart`（智能拦截）/ `detect`（只记录）
    pub source: String,
}

/// 拦截规则：命中后自动处理窗口。
///
/// 匹配语义（全部满足）：
/// 1. `process` 忽略大小写相等；
/// 2. `class_name` 非空时必须忽略大小写相等；
/// 3. `title` 非空时窗口标题必须包含它（忽略大小写）。
///
/// 所以 `scope=app`（两个匹配字段都空）表示「这个程序的所有弹窗」。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PopupRule {
    pub id: String,
    pub process: String,
    /// 进程完整路径，仅用于展示图标
    pub path: String,
    /// 窗口类名（空 = 不按类名匹配）
    pub class_name: String,
    /// 标题子串（空 = 不按标题匹配）
    pub title: String,
    /// 展示名（火绒那种「广告弹窗 / 红包 / 软件推荐」），不参与匹配
    pub name: String,
    /// 弹窗通常出现的位置（右下窗口 / 中间窗口 / 任意位置），仅展示
    pub position: String,
    /// 展示用范围：`window` / `class` / `app`
    pub scope: String,
    /// 动作：`close`（WM_CLOSE）/ `hide`（SW_HIDE）
    pub action: String,
    pub enabled: bool,
    pub hits: u64,
    pub last_hit_ms: u64,
    pub created_ms: u64,
}

impl Default for PopupRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            process: String::new(),
            path: String::new(),
            class_name: String::new(),
            title: String::new(),
            name: String::new(),
            position: String::new(),
            scope: "class".into(),
            action: "close".into(),
            enabled: true,
            hits: 0,
            last_hit_ms: 0,
            created_ms: 0,
        }
    }
}

/// 前端提交的新规则。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PopupRuleDraft {
    pub process: String,
    pub path: String,
    pub class_name: String,
    pub title: String,
    /// 可选展示名；为空时由标题 / 类名自动生成
    pub name: String,
    /// 可选位置；由「从窗口添加」带上
    pub position: String,
    pub scope: String,
    pub action: String,
}

impl Default for PopupRuleDraft {
    fn default() -> Self {
        Self {
            process: String::new(),
            path: String::new(),
            class_name: String::new(),
            title: String::new(),
            name: String::new(),
            position: String::new(),
            scope: "class".into(),
            action: "close".into(),
        }
    }
}

/// 拦截模式设置（持久化到 `<data_dir>/popup-settings.json`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PopupSettings {
    /// 弹窗拦截总开关：开启后下次启动自动恢复（`popup_start` / `popup_stop` 会同步）
    pub enabled: bool,
    /// 新规则的默认动作，也用于智能拦截
    pub action: String,
    /// 智能拦截：没有命中规则时，自动处理高可疑窗口（默认关）
    pub smart: bool,
    /// 智能拦截的最低可疑分
    pub min_score: u32,
    /// 每分钟最多自动处理几个窗口（熔断，防止误杀连环触发）
    pub max_per_minute: u32,
}

impl Default for PopupSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            action: "close".into(),
            smart: false,
            min_score: 10,
            max_per_minute: 6,
        }
    }
}

impl PopupSettings {
    /// 把外部传进来的值收进安全范围（前端/手工改 JSON 都可能越界）
    pub fn normalized(mut self) -> Self {
        self.action = normalize_action(&self.action).to_string();
        self.min_score = self.min_score.clamp(5, 30);
        self.max_per_minute = self.max_per_minute.clamp(1, 30);
        self
    }
}

/// 判定窗口「可疑」用的特征。抽成纯结构，方便单测打分逻辑。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub struct WindowFeatures {
    pub has_title: bool,
    pub is_tool_window: bool,
    pub is_no_activate: bool,
    pub is_topmost: bool,
    pub is_foreground_process: bool,
    /// 有 owner（`GetWindow(GW_OWNER)` 非空）：通常是应用弹出的附属窗口
    pub is_owned: bool,
    /// 可调整大小（WS_THICKFRAME）：主窗口的典型特征
    pub is_resizable: bool,
    /// 每个进程里挑出来的主窗口，任何规则 / 智能拦截都不能碰
    pub is_main: bool,
    /// 相对屏幕的位置分类（右下窗口 / 中间窗口 / 任意位置）
    pub position: String,
    pub width: i32,
    pub height: i32,
    pub class_name: String,
    pub title: String,
    pub process: String,
    pub path: String,
}

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
impl WindowFeatures {
    pub fn display_title(&self) -> String {
        clean_text(&self.title, 120)
    }

    /// 是否可以当「主窗口」候选：无 owner、非工具窗口、有标题、面积够大。
    ///
    /// 每个进程只保护面积最大的那个，避免把应用自己的主界面给拦掉；托盘类
    /// 小工具没有主窗口时不会误保护它的广告弹窗。
    fn is_main_candidate(&self) -> bool {
        !self.is_owned
            && !self.is_tool_window
            && !self.is_no_activate
            && self.has_title
            && self.width >= 320
            && self.height >= 240
            && (self.is_resizable || self.width >= 480 || self.height >= 400)
    }
}

/// 给每个进程标出主窗口（面积最大的主窗口候选）。
///
/// 这是「只拦应用弹出的广告窗、不拦应用本身」的兜底：即便用户对某进程加了
/// `scope=app` 的规则，主窗口也会被跳过。
pub fn mark_main_windows(items: &mut [(WindowFeatures, isize)]) {
    use std::collections::HashMap;

    let mut best: HashMap<String, (i64, usize)> = HashMap::new();
    for (index, (features, _)) in items.iter().enumerate() {
        if !features.is_main_candidate() {
            continue;
        }
        let area = features.width as i64 * features.height as i64;
        match best.get(&features.process) {
            Some((best_area, _)) if *best_area >= area => {}
            _ => {
                best.insert(features.process.clone(), (area, index));
            }
        }
    }
    for (_, index) in best.values() {
        items[*index].0.is_main = true;
    }
}

/// 窗口相对屏幕的位置。用于规则列表里的「弹窗位置」列，不参与匹配。
pub fn position_of(
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    screen_w: i32,
    screen_h: i32,
) -> String {
    if width <= 0 || height <= 0 || screen_w <= 0 || screen_h <= 0 {
        return "任意位置".into();
    }
    // 用窗口中心点判断，右下角的小弹窗不会因为宽度大就被算成「中间」
    let cx = left as f32 + width as f32 / 2.0;
    let cy = top as f32 + height as f32 / 2.0;
    let right = cx > screen_w as f32 * 0.6;
    let left_side = cx < screen_w as f32 * 0.4;
    let bottom = cy > screen_h as f32 * 0.6;
    let top_side = cy < screen_h as f32 * 0.4;
    let near_center = (cx - screen_w as f32 / 2.0).abs() < screen_w as f32 * 0.2
        && (cy - screen_h as f32 / 2.0).abs() < screen_h as f32 * 0.2;
    if near_center {
        return "中间窗口".into();
    }
    match (right, left_side, bottom, top_side) {
        (true, _, true, _) => "右下窗口".into(),
        (_, true, true, _) => "左下窗口".into(),
        (true, _, _, true) => "右上窗口".into(),
        (_, true, _, true) => "左上窗口".into(),
        (true, _, _, _) => "右侧窗口".into(),
        (_, true, _, _) => "左侧窗口".into(),
        (_, _, true, _) => "底部窗口".into(),
        (_, _, _, true) => "顶部窗口".into(),
        _ => "任意位置".into(),
    }
}

/// 规则展示名：从标题里挑一个火绒风格的短名，标题太怪就退回「广告弹窗」。
pub fn rule_name(scope: &str, title: &str, class_name: &str) -> String {
    if scope == "app" {
        return "全部弹窗".into();
    }
    let title = title.trim();
    for (needle, label) in [
        ("红包", "红包"),
        ("福利", "福利"),
        ("优惠", "优惠"),
        ("抽奖", "抽奖"),
        ("推荐", "软件推荐"),
        ("广告", "广告弹窗"),
        ("推广", "广告弹窗"),
        ("活动", "活动弹窗"),
    ] {
        if title.contains(needle) {
            return label.into();
        }
    }
    let clean = clean_text(title, 24);
    if clean.chars().count() >= 2 {
        return clean;
    }
    if !class_name.trim().is_empty() {
        return "广告弹窗".into();
    }
    "广告弹窗".into()
}

/// 已命中的窗口信息：规则命中 / 智能拦截时写进拦截记录。
#[derive(Debug, Clone, Serialize)]
pub struct PopupWindow {
    pub process: String,
    pub path: String,
    pub title: String,
    pub class_name: String,
    pub width: i32,
    pub height: i32,
    /// 相对屏幕的位置（右下窗口 / 中间窗口 / 任意位置）
    pub position: String,
    pub score: u32,
    pub reasons: Vec<String>,
    /// 是否为该进程的主窗口（主窗口永远不拦截）
    pub main: bool,
    /// 是否允许对它加拦截规则（系统外壳窗口 / 忽略名单里的程序为 false）
    pub blockable: bool,
    /// 是否在忽略名单里（界面用来区分「系统窗口」和「已忽略」）
    pub ignored: bool,
    /// 已经有一条启用中的规则能命中它
    pub ruled: bool,
}

/// 应用展示信息：文件名看不出是哪个软件时，用 exe 的版本信息补一个可读名字。
#[derive(Debug, Clone, Serialize, Default)]
pub struct PopupAppMeta {
    pub name: String,
    pub icon: Option<String>,
}

/// 图标提取要走一次 PowerShell，按路径缓存，面板反复刷新不重复算。
static APP_META_CACHE: LazyLock<Mutex<std::collections::HashMap<String, PopupAppMeta>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// 批量取应用展示信息（只查缓存里没有的路径）。
pub fn app_meta(paths: Vec<String>) -> std::collections::HashMap<String, PopupAppMeta> {
    use std::collections::HashMap;

    let mut out: HashMap<String, PopupAppMeta> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    {
        let cache = APP_META_CACHE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for path in paths {
            let path = path.trim();
            if path.is_empty() {
                continue;
            }
            match cache.get(path) {
                Some(meta) => {
                    out.insert(path.to_string(), meta.clone());
                }
                None => {
                    if !missing.iter().any(|existing| existing == path) {
                        missing.push(path.to_string());
                    }
                }
            }
        }
    }
    if missing.is_empty() {
        return out;
    }

    #[cfg(target_os = "windows")]
    {
        let refs: Vec<std::path::PathBuf> =
            missing.iter().map(std::path::PathBuf::from).collect();
        let borrowed: Vec<&std::path::Path> = refs.iter().map(|path| path.as_path()).collect();
        let found = crate::commands::openwith::windows_app_meta(&borrowed);
        let mut cache = APP_META_CACHE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for path in &missing {
            let meta = found
                .get(std::path::Path::new(path))
                .map(|meta| PopupAppMeta {
                    name: meta.name.clone(),
                    icon: meta.icon.clone(),
                })
                .unwrap_or_else(|| PopupAppMeta {
                    name: file_stem(path),
                    icon: None,
                });
            cache.insert(path.clone(), meta.clone());
            out.insert(path.clone(), meta);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cache = APP_META_CACHE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for path in missing {
            let meta = PopupAppMeta {
                name: file_stem(&path),
                icon: None,
            };
            cache.insert(path.clone(), meta.clone());
            out.insert(path, meta);
        }
    }
    out
}

fn file_stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// 永远不自动处理的进程：系统外壳 / 安全 / 输入法 / 本应用自己。
/// 命中这些进程的窗口只会记录，绝不自动关闭，也不允许加规则。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
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
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
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

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn is_protected_process(process: &str) -> bool {
    let lower = process.to_ascii_lowercase();
    if PROTECTED_PROCESSES
        .iter()
        .any(|protected| protected.eq_ignore_ascii_case(&lower))
    {
        return true;
    }
    // 本应用自己的进程名可能带平台后缀（ws-workbench-dev.exe），按当前 exe 名再兜一层
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
        })
        .is_some_and(|own| own == lower)
}

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn is_protected_class(class_name: &str) -> bool {
    let lower = class_name.to_ascii_lowercase();
    PROTECTED_CLASSES
        .iter()
        .any(|protected| lower == *protected || lower.starts_with(protected))
}

/// 拦截模式的判定结果：要动窗口时带上动作，否则给不处理的原因（调试 / 测试用）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub enum InterceptPlan {
    Intercept(&'static str),
    Skip(&'static str),
}

/// 智能拦截的纯逻辑闸门：设置开着、分数够、命中原因够多、没被保护、没被忽略、
/// 没超过每分钟预算时，才允许自动处理。**规则拦截不走这个闸门**（那是用户明确的意图）。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn should_intercept(
    features: &WindowFeatures,
    score: u32,
    reasons: &[String],
    settings: &PopupSettings,
    ignored_processes: &[String],
    actions_this_minute: u32,
) -> InterceptPlan {
    if !settings.smart {
        return InterceptPlan::Skip("智能拦截未开启");
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
    "广告", "活动", "推荐", "福利", "红包", "优惠", "抽奖", "弹窗",
];

/// 给窗口打分：分数只用于排序与「可疑」标记，规则不会看它。
///
/// 返回 (score, reasons)，score 越大越可疑；0 分表示没有命中任何可疑特征。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
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
    if features.is_owned {
        score += 2;
        reasons.push("应用弹出的附属窗口".into());
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

// ============================== 规则纯逻辑 ==============================

fn normalize_action(action: &str) -> &'static str {
    if action.eq_ignore_ascii_case("hide") {
        "hide"
    } else {
        "close"
    }
}

fn normalize_scope(scope: &str) -> &'static str {
    match scope {
        "window" => "window",
        "app" => "app",
        _ => "class",
    }
}

/// 标题、类名、进程名统一清洗：去首尾空白、去掉控制字符、限长。
fn clean_text(raw: &str, max_chars: usize) -> String {
    let trimmed = raw.trim();
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_control() {
            continue;
        }
        out.push(ch);
        if out.chars().count() >= max_chars {
            break;
        }
    }
    out
}

/// 规则是否命中一个窗口。`process` / `class_name` 忽略大小写，`title` 是子串匹配。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn rule_matches(rule: &PopupRule, process: &str, class_name: &str, title: &str) -> bool {
    if !rule.enabled || rule.process.trim().is_empty() {
        return false;
    }
    if !rule.process.eq_ignore_ascii_case(process.trim()) {
        return false;
    }
    if !rule.class_name.trim().is_empty()
        && !rule.class_name.eq_ignore_ascii_case(class_name.trim())
    {
        return false;
    }
    if !rule.title.trim().is_empty()
        && !title
            .to_lowercase()
            .contains(&rule.title.trim().to_lowercase())
    {
        return false;
    }
    true
}

impl PopupRuleDraft {
    /// 校验并转成规则：范围决定哪些匹配字段保留，动作只接受 close / hide。
    pub fn into_rule(self) -> Result<PopupRule, String> {
        let process = clean_text(&self.process, 120);
        if process.is_empty() {
            return Err("缺少进程名".into());
        }
        if is_protected_process(&process) {
            return Err(format!("{process} 是系统进程或本应用，不支持拦截"));
        }
        let scope = normalize_scope(&self.scope);
        let mut class_name = clean_text(&self.class_name, 200);
        let mut title = clean_text(&self.title, 60);
        if scope == "app" {
            class_name.clear();
            title.clear();
        } else if class_name.is_empty() && title.is_empty() {
            return Err("请选择拦截范围：至少按窗口类或标题匹配".into());
        }
        if scope == "class" {
            title.clear();
        }
        let name = if self.name.trim().is_empty() {
            rule_name(scope, &title, &class_name)
        } else {
            clean_text(&self.name, 40)
        };
        let position = if scope == "app" {
            "任意位置".to_string()
        } else {
            let raw = clean_text(&self.position, 20);
            if raw.is_empty() {
                "任意位置".to_string()
            } else {
                raw
            }
        };
        let now = now_ms();
        Ok(PopupRule {
            id: uuid::Uuid::new_v4().to_string(),
            process,
            path: clean_text(&self.path, 512),
            class_name,
            title,
            name,
            position,
            scope: scope.into(),
            action: normalize_action(&self.action).into(),
            enabled: true,
            hits: 0,
            last_hit_ms: 0,
            created_ms: now,
        })
    }
}

/// 把新规则并进列表：同一匹配条件已经存在时只更新动作并重新启用，返回规则 id。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn merge_rule(rules: &mut Vec<PopupRule>, rule: PopupRule) -> String {
    let same = rules.iter().position(|existing| {
        existing.process.eq_ignore_ascii_case(&rule.process)
            && existing
                .class_name
                .eq_ignore_ascii_case(&rule.class_name)
            && existing.title.eq_ignore_ascii_case(&rule.title)
    });
    if let Some(index) = same {
        let existing = &mut rules[index];
        existing.action = rule.action;
        existing.scope = rule.scope;
        existing.enabled = true;
        if !rule.name.is_empty() {
            existing.name = rule.name;
        }
        if !rule.position.is_empty() {
            existing.position = rule.position;
        }
        if !rule.path.is_empty() {
            existing.path = rule.path;
        }
        return existing.id.clone();
    }
    let id = rule.id.clone();
    rules.push(rule);
    id
}

/// 读规则文件时做的清洗：坏条目直接丢弃，不因为一条脏数据拖垮整张表。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn sanitize_rules(rules: Vec<PopupRule>) -> Vec<PopupRule> {
    let mut out = Vec::new();
    for mut rule in rules {
        if rule.process.trim().is_empty() {
            continue;
        }
        rule.process = clean_text(&rule.process, 120);
        rule.class_name = clean_text(&rule.class_name, 200);
        rule.title = clean_text(&rule.title, 60);
        rule.scope = normalize_scope(&rule.scope).to_string();
        rule.action = normalize_action(&rule.action).to_string();
        rule.path = clean_text(&rule.path, 512);
        rule.name = clean_text(&rule.name, 40);
        if rule.name.is_empty() {
            rule.name = rule_name(&rule.scope, &rule.title, &rule.class_name);
        }
        rule.position = clean_text(&rule.position, 20);
        if rule.position.is_empty() {
            rule.position = "任意位置".into();
        }
        if rule.id.trim().is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
        }
        out.push(rule);
    }
    out
}

/// 规则的一句话说明（写进拦截记录 / 调试日志）
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn rule_summary(rule: &PopupRule) -> String {
    if !rule.class_name.is_empty() && !rule.title.is_empty() {
        format!("类名 {} · 标题含「{}」", rule.class_name, rule.title)
    } else if !rule.class_name.is_empty() {
        format!("类名 {}", rule.class_name)
    } else if !rule.title.is_empty() {
        format!("标题含「{}」", rule.title)
    } else {
        "该程序的所有弹窗".into()
    }
}

// ============================== 全局状态 ==============================

static NEXT_HIT_ID: AtomicU64 = AtomicU64::new(1);

fn next_hit_id() -> u64 {
    NEXT_HIT_ID.fetch_add(1, Ordering::Relaxed)
}

struct Manager {
    #[cfg(target_os = "windows")]
    inner: Option<imp::Inner>,
    rules: Arc<Mutex<Vec<PopupRule>>>,
    settings: Arc<Mutex<PopupSettings>>,
    ignore: Arc<Mutex<Vec<String>>>,
}

impl Manager {
    fn load() -> Self {
        let dir = data_dir().ok();
        let rules = dir
            .as_deref()
            .map(load_rules)
            .unwrap_or_default();
        let settings = dir
            .as_deref()
            .map(load_settings)
            .unwrap_or_default();
        let ignore = dir
            .as_deref()
            .map(load_ignore)
            .unwrap_or_default();
        Self {
            #[cfg(target_os = "windows")]
            inner: None,
            rules: Arc::new(Mutex::new(rules)),
            settings: Arc::new(Mutex::new(settings)),
            ignore: Arc::new(Mutex::new(ignore)),
        }
    }
}

static MANAGER: LazyLock<Mutex<Manager>> = LazyLock::new(|| Mutex::new(Manager::load()));

/// 不自己拿锁的状态快照：`start` / `stop` 已经持有 `MANAGER` 的 guard 时调用。
///
/// 这里刻意只接收 `&Manager` 而不是内部 `MANAGER.lock()`——`std::sync::Mutex`
/// 不可重入，`start()` 持锁后调用 `status()` 会直接把自己锁死，命令永远不返回，
/// Tauri 主线程被占住，整个窗口变「未响应」。
fn status_of(manager: &Manager) -> PopupStatus {
    #[cfg(target_os = "windows")]
    {
        let running = manager.inner.is_some();
        let (count, blocked) = manager
            .inner
            .as_ref()
            .and_then(|inner| inner.buffer.lock().ok().map(|buffer| {
                let blocked = buffer
                    .iter()
                    .filter(|hit| hit.auto_action.is_some())
                    .count();
                (buffer.len(), blocked)
            }))
            .unwrap_or((0, 0));
        PopupStatus {
            supported: true,
            running,
            count,
            blocked,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = manager;
        PopupStatus {
            supported: false,
            running: false,
            count: 0,
            blocked: 0,
        }
    }
}

pub fn status() -> PopupStatus {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    status_of(&manager)
}

pub fn recent() -> Vec<PopupHit> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    {
        manager
            .inner
            .as_ref()
            .and_then(|inner| {
                inner
                    .buffer
                    .lock()
                    .ok()
                    .map(|buffer| buffer.iter().rev().cloned().collect())
            })
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
        // 清空后允许同一批窗口重新进列表（否则线程里的签名表会把它们静音到窗口关闭）
        if let Ok(mut known) = inner.known.lock() {
            known.clear();
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &manager;
}

#[cfg(target_os = "windows")]
pub fn start(app: tauri::AppHandle) -> Result<PopupStatus, String> {
    let mut manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    if manager.inner.is_none() {
        manager.inner = Some(imp::spawn(
            app,
            Arc::clone(&manager.rules),
            Arc::clone(&manager.settings),
            Arc::clone(&manager.ignore),
        )?);
    }
    // 总开关持久化：下次启动自动恢复
    if let Ok(mut settings) = manager.settings.lock() {
        if !settings.enabled {
            settings.enabled = true;
            let snapshot = settings.clone();
            let _ = save_settings(&snapshot);
        }
    }
    Ok(status_of(&manager))
}

#[cfg(not(target_os = "windows"))]
pub fn start(_app: tauri::AppHandle) -> Result<PopupStatus, String> {
    Err("仅 Windows 支持弹窗拦截".into())
}

pub fn stop() -> Result<PopupStatus, String> {
    let mut manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    #[cfg(target_os = "windows")]
    if let Some(inner) = manager.inner.take() {
        inner.stop();
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &mut manager;
    if let Ok(mut settings) = manager.settings.lock() {
        if settings.enabled {
            settings.enabled = false;
            let snapshot = settings.clone();
            let _ = save_settings(&snapshot);
        }
    }
    Ok(status_of(&manager))
}

/// 用户手动处理一条记录：close 发 WM_CLOSE，hide 只 SW_HIDE。
#[cfg(target_os = "windows")]
pub fn dismiss(id: u64, action: &str) -> Result<(), String> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let inner = manager
        .inner
        .as_ref()
        .ok_or_else(|| "弹窗拦截未开启".to_string())?;
    inner.dismiss(id, normalize_action(action))
}

#[cfg(not(target_os = "windows"))]
pub fn dismiss(_id: u64, _action: &str) -> Result<(), String> {
    Err("仅 Windows 支持弹窗拦截".into())
}

/// 把进程加入 / 移出忽略名单（白名单），返回最新名单。
pub fn ignore(process: &str, ignored: bool) -> Result<Vec<String>, String> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let needle = process.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Err("进程名不能为空".into());
    }
    let mut list = manager
        .ignore
        .lock()
        .map_err(|_| "忽略名单损坏".to_string())?;
    list.retain(|item| item.to_ascii_lowercase() != needle);
    if ignored {
        list.push(process.trim().to_string());
        list.sort_by_key(|item| item.to_ascii_lowercase());
        list.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    }
    let snapshot = list.clone();
    drop(list);
    save_ignore(&snapshot)?;
    Ok(snapshot)
}

pub fn ignored() -> Vec<String> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    manager
        .ignore
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default()
}

/// 读拦截设置；失败退回默认值。
pub fn settings_get() -> PopupSettings {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    manager
        .settings
        .lock()
        .map(|settings| settings.clone())
        .unwrap_or_default()
}

/// 写拦截设置：先落盘，再同步给正在跑的检测线程。
///
/// `enabled` 不接受前端传值：总开关由 `start` / `stop` 维护，避免前端拿旧的
/// 设置对象把「已开启」覆盖回去。
pub fn settings_set(settings: PopupSettings) -> Result<PopupSettings, String> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let normalized = settings.normalized();
    let snapshot = {
        let mut current = manager
            .settings
            .lock()
            .map_err(|_| "拦截设置损坏".to_string())?;
        current.action = normalized.action;
        current.smart = normalized.smart;
        current.min_score = normalized.min_score;
        current.max_per_minute = normalized.max_per_minute;
        current.clone()
    };
    save_settings(&snapshot)?;
    Ok(snapshot)
}

/// 全部规则，按启用状态 + 创建时间排序（启用在前，新的在前）。
pub fn rules_list() -> Vec<PopupRule> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut rules = manager
        .rules
        .lock()
        .map(|rules| rules.clone())
        .unwrap_or_default();
    sort_rules(&mut rules);
    rules
}

fn sort_rules(rules: &mut Vec<PopupRule>) {
    rules.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then_with(|| b.created_ms.cmp(&a.created_ms))
    });
}

/// 新增规则的结果：`id` 可能指向一条已存在的规则（重复添加会合并）。
#[derive(Debug, Clone, Serialize)]
pub struct PopupRuleAddResult {
    pub id: String,
    pub rules: Vec<PopupRule>,
}

/// 新增规则；匹配条件重复时合并到已有规则上。返回规则 id 与最新规则表。
pub fn rule_add(draft: PopupRuleDraft) -> Result<PopupRuleAddResult, String> {
    let rule = draft.into_rule()?;
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut rules = manager
        .rules
        .lock()
        .map_err(|_| "规则表损坏".to_string())?;
    let id = merge_rule(&mut rules, rule);
    sort_rules(&mut rules);
    let snapshot = rules.clone();
    drop(rules);
    save_rules(&snapshot)?;
    Ok(PopupRuleAddResult {
        id,
        rules: snapshot,
    })
}

pub fn rule_remove(id: &str) -> Result<Vec<PopupRule>, String> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut rules = manager
        .rules
        .lock()
        .map_err(|_| "规则表损坏".to_string())?;
    rules.retain(|rule| rule.id != id);
    let snapshot = rules.clone();
    drop(rules);
    save_rules(&snapshot)?;
    Ok(snapshot)
}

pub fn rule_set_enabled(id: &str, enabled: bool) -> Result<Vec<PopupRule>, String> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut rules = manager
        .rules
        .lock()
        .map_err(|_| "规则表损坏".to_string())?;
    if let Some(rule) = rules.iter_mut().find(|rule| rule.id == id) {
        rule.enabled = enabled;
    } else {
        return Err("规则不存在".into());
    }
    let snapshot = rules.clone();
    drop(rules);
    save_rules(&snapshot)?;
    Ok(snapshot)
}

/// 按应用整体启用 / 停用（火绒列表里每个软件一行开关）。
pub fn rule_set_app_enabled(process: &str, enabled: bool) -> Result<Vec<PopupRule>, String> {
    let process = process.trim();
    if process.is_empty() {
        return Err("进程名不能为空".into());
    }
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut rules = manager
        .rules
        .lock()
        .map_err(|_| "规则表损坏".to_string())?;
    let mut touched = false;
    for rule in rules
        .iter_mut()
        .filter(|rule| rule.process.eq_ignore_ascii_case(process))
    {
        rule.enabled = enabled;
        touched = true;
    }
    if !touched {
        return Err("该应用没有拦截规则".into());
    }
    sort_rules(&mut rules);
    let snapshot = rules.clone();
    drop(rules);
    save_rules(&snapshot)?;
    Ok(snapshot)
}

/// 规则保存后立即对「现在正开着的窗口」执行一遍，返回处理了几个窗口。
#[cfg(target_os = "windows")]
pub fn rule_apply(rule_id: &str, app: &tauri::AppHandle) -> Result<usize, String> {
    use tauri::Emitter;

    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let rule = manager
        .rules
        .lock()
        .ok()
        .and_then(|rules| rules.iter().find(|rule| rule.id == rule_id).cloned())
        .ok_or_else(|| "规则不存在".to_string())?;
    if !rule.enabled {
        return Err("规则已停用".into());
    }
    let inner = manager
        .inner
        .as_ref()
        .ok_or_else(|| "弹窗拦截未开启，规则会在下次开启后生效".to_string())?;
    let ignored = manager
        .ignore
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default();
    let foreground = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    let windows = imp::collect_windows(foreground);
    let mut applied = 0usize;
    let mut refused = 0usize;
    for (features, handle) in windows {
        if features.is_main {
            continue;
        }
        if is_protected_process(&features.process) || is_protected_class(&features.class_name) {
            continue;
        }
        if ignored
            .iter()
            .any(|process| process.eq_ignore_ascii_case(&features.process))
        {
            continue;
        }
        if !rule_matches(&rule, &features.process, &features.class_name, &features.title) {
            continue;
        }
        if !imp::apply_window_action(handle, &rule.action) {
            refused += 1;
            continue;
        }
        applied += 1;
        // 已经有同名窗口在「待处理」里时原地升级成拦截记录，避免重复行
        let existing = {
            let buffer = inner.buffer.lock().ok();
            buffer.and_then(|buffer| {
                buffer
                    .iter()
                    .find(|hit| hit.auto_action.is_none() && hit.process == features.process
                        && hit.class_name == features.class_name
                        && hit.title == features.display_title())
                    .map(|hit| hit.id)
            })
        };
        let hit = if let Some(id) = existing {
            let mut hit = inner
                .buffer
                .lock()
                .ok()
                .and_then(|buffer| buffer.iter().find(|hit| hit.id == id).cloned())
                .unwrap_or_default();
            hit.auto_action = Some(rule.action.clone());
            hit.rule_id = Some(rule.id.clone());
            hit.source = "rule".into();
            hit.at_ms = now_ms();
            hit.position = features.position.clone();
            hit.reasons = vec![format!("命中规则：{}", rule_summary(&rule))];
            if let Ok(mut buffer) = inner.buffer.lock() {
                if let Some(slot) = buffer.iter_mut().find(|hit| hit.id == id) {
                    *slot = hit.clone();
                }
            }
            hit
        } else {
            let hit = PopupHit {
                id: next_hit_id(),
                at_ms: now_ms(),
                process: features.process.clone(),
                path: features.path.clone(),
                title: features.display_title(),
                class_name: features.class_name.clone(),
                width: features.width,
                height: features.height,
                position: features.position.clone(),
                score: 0,
                reasons: vec![format!("命中规则：{}", rule_summary(&rule))],
                auto_action: Some(rule.action.clone()),
                rule_id: Some(rule.id.clone()),
                source: "rule".into(),
            };
            if let Ok(mut buffer) = inner.buffer.lock() {
                if buffer.len() >= HIT_LIMIT {
                    buffer.pop_front();
                }
                buffer.push_back(hit.clone());
            }
            hit
        };
        if let Ok(mut handles) = inner.handles.lock() {
            handles.insert(hit.id, handle);
        }
        if let Ok(mut known) = inner.known.lock() {
            known.insert(handle, format!("rule|{}", rule.id));
        }
        let _ = app.emit("popup-detected", &hit);
    }
    // 命中次数一次性累加
    if applied > 0 {
        if let Ok(mut rules) = manager.rules.lock() {
            if let Some(rule) = rules.iter_mut().find(|rule| rule.id == rule_id) {
                rule.hits = rule.hits.saturating_add(applied as u64);
                rule.last_hit_ms = now_ms();
            }
            let snapshot = rules.clone();
            drop(rules);
            let _ = save_rules(&snapshot);
        }
    }
    if applied == 0 && refused > 0 {
        return Err("窗口拒绝关闭：可能以管理员权限运行".into());
    }
    Ok(applied)
}

#[cfg(not(target_os = "windows"))]
pub fn rule_apply(_rule_id: &str, _app: &tauri::AppHandle) -> Result<usize, String> {
    Err("仅 Windows 支持弹窗拦截".into())
}

/// 列出当前可见的顶层窗口，供「从窗口添加规则」挑选。
#[cfg(target_os = "windows")]
pub fn windows() -> Vec<PopupWindow> {
    let manager = MANAGER.lock().unwrap_or_else(|poison| poison.into_inner());
    let rules = manager
        .rules
        .lock()
        .map(|rules| rules.clone())
        .unwrap_or_default();
    let ignored = manager
        .ignore
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default();
    drop(manager);

    let foreground = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    let mut out: Vec<PopupWindow> = imp::collect_windows(foreground)
        .into_iter()
        .filter_map(|(features, _)| {
            let score_reasons = score_window(&features);
            let visible = !features.title.trim().is_empty() || score_reasons.0 > 0;
            if !visible || features.width < 80 || features.height < 40 {
                return None;
            }
            if is_protected_process(&features.process) {
                return None;
            }
            let protected = is_protected_class(&features.class_name);
            let ignored_process = ignored
                .iter()
                .any(|process| process.eq_ignore_ascii_case(&features.process));
            Some(PopupWindow {
                process: features.process.clone(),
                path: features.path.clone(),
                title: features.display_title(),
                class_name: features.class_name.clone(),
                width: features.width,
                height: features.height,
                position: features.position.clone(),
                score: score_reasons.0,
                reasons: score_reasons.1,
                main: features.is_main,
                blockable: !protected && !ignored_process && !features.is_main,
                ignored: ignored_process,
                ruled: rules.iter().any(|rule| {
                    rule_matches(rule, &features.process, &features.class_name, &features.title)
                }),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.process.to_lowercase().cmp(&b.process.to_lowercase()))
            .then_with(|| a.title.cmp(&b.title))
    });
    out.truncate(200);
    out
}

#[cfg(not(target_os = "windows"))]
pub fn windows() -> Vec<PopupWindow> {
    Vec::new()
}

// ============================== 持久化 ==============================

fn settings_path() -> Result<std::path::PathBuf, String> {
    Ok(data_dir()?.join("popup-settings.json"))
}

fn rules_path() -> Result<std::path::PathBuf, String> {
    Ok(data_dir()?.join("popup-rules.json"))
}

fn ignore_path() -> Result<std::path::PathBuf, String> {
    Ok(data_dir()?.join("popup-ignore.json"))
}

fn load_settings(dir: &std::path::Path) -> PopupSettings {
    read_text(&dir.join("popup-settings.json"))
        .and_then(|raw| serde_json::from_str::<PopupSettings>(&raw).ok())
        .map(PopupSettings::normalized)
        .unwrap_or_default()
}

fn save_settings(settings: &PopupSettings) -> Result<(), String> {
    let path = settings_path()?;
    let raw =
        serde_json::to_string_pretty(settings).map_err(|err| format!("序列化拦截设置失败：{err}"))?;
    write_atomic(&path, raw.as_bytes())
}

fn load_rules(dir: &std::path::Path) -> Vec<PopupRule> {
    read_text(&dir.join("popup-rules.json"))
        .and_then(|raw| serde_json::from_str::<Vec<PopupRule>>(&raw).ok())
        .map(sanitize_rules)
        .unwrap_or_default()
}

/// 读忽略名单。纯文件读取，和平台无关，放在外层而不是两个 `imp` 里。
fn load_ignore(dir: &std::path::Path) -> Vec<String> {
    read_text(&dir.join("popup-ignore.json"))
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

/// 读文本并去掉 UTF-8 BOM。
///
/// 用户拿记事本 / PowerShell 5 的 `-Encoding UTF8` 改 `popup-*.json` 时很容易带上
/// BOM，而 `serde_json` 会直接报错；这里统一剥掉，避免规则「莫名消失」。
fn read_text(path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    Some(strip_bom(&raw).to_string())
}

/// 去掉 UTF-8 BOM；单独抽出来方便单测，不依赖临时目录权限。
fn strip_bom(raw: &str) -> &str {
    raw.strip_prefix('\u{feff}').unwrap_or(raw)
}

fn save_rules(rules: &[PopupRule]) -> Result<(), String> {
    let path = rules_path()?;
    let raw =
        serde_json::to_string_pretty(rules).map_err(|err| format!("序列化拦截规则失败：{err}"))?;
    write_atomic(&path, raw.as_bytes())
}

fn save_ignore(list: &[String]) -> Result<(), String> {
    let path = ignore_path()?;
    let raw =
        serde_json::to_string_pretty(list).map_err(|err| format!("序列化忽略名单失败：{err}"))?;
    write_atomic(&path, raw.as_bytes())
}

/// 先写临时文件再改名，避免写一半被读到半截 JSON。
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes).map_err(|err| format!("写入 {} 失败：{err}", path.display()))?;
    std::fs::rename(&temp, path).map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        format!("替换 {} 失败：{err}", path.display())
    })
}

fn data_dir() -> Result<std::path::PathBuf, String> {
    crate::cleaner::app_data_dir()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
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
        EnumWindows, GetClassNameW, GetForegroundWindow, GetSystemMetrics, GetWindow,
        GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindow,
        IsWindowVisible, PostMessageW, ShowWindow, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, SM_CXSCREEN,
        SM_CYSCREEN, SW_HIDE, WM_CLOSE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_THICKFRAME,
    };

    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
    /// 命中次数最多每 10 秒落一次盘（弹窗连发时不至于反复写文件）
    const RULE_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

    pub struct Inner {
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
        pub buffer: Arc<Mutex<VecDeque<PopupHit>>>,
        pub handles: Arc<Mutex<HashMap<u64, isize>>>,
        /// 窗口句柄 → 已记录的签名（避免同一窗口每轮都记一次）
        pub known: Arc<Mutex<HashMap<isize, String>>>,
        /// 窗口句柄 → 上次动作时间（同时写在检测线程里，供清除 / 手动处理复用）
        pub acted: Arc<Mutex<HashMap<isize, std::time::Instant>>>,
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
                .copied();
            let hit = self
                .buffer
                .lock()
                .ok()
                .and_then(|buffer| buffer.iter().find(|hit| hit.id == id).cloned());
            let Some(hit) = hit else {
                return Err("记录已过期".into());
            };
            if hit.auto_action.is_some() {
                return Err("该弹窗已经处理过".into());
            }
            let Some(handle) = handle else {
                return Err("窗口已关闭".into());
            };
            if !window_alive(handle) {
                return Err("窗口已经不在".into());
            }
            if !apply_window_action(handle, action) {
                // 最常见的原因是目标窗口以管理员权限运行，UIPI 不让普通进程发消息
                return Err(if action == "hide" {
                    "隐藏失败：窗口可能以管理员权限运行".into()
                } else {
                    "关闭失败：窗口可能以管理员权限运行".into()
                });
            }
            // 手动处理也记一下冷却：同进程 3 秒内不要被智能拦截立刻再动一次
            if let Ok(mut acted) = self.acted.lock() {
                acted.insert(handle, std::time::Instant::now());
            }
            Ok(())
        }
    }

    /// 对窗口执行动作：close 走 WM_CLOSE（正常关闭流程），hide 只是 SW_HIDE。
    ///
    /// 返回是否真的把请求发了出去。目标窗口以更高权限运行时 UIPI 会拒绝
    /// `PostMessage` / `ShowWindow`，这时不能记成「已拦截」。
    pub fn apply_window_action(handle: isize, action: &str) -> bool {
        let hwnd = HWND(handle as *mut _);
        unsafe {
            if action == "hide" {
                ShowWindow(hwnd, SW_HIDE).as_bool()
            } else {
                PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
            }
        }
    }

    /// 句柄对应的窗口是否还在：HWND 会被系统复用，不能只看记录里有没有
    fn window_alive(handle: isize) -> bool {
        unsafe { IsWindow(Some(HWND(handle as *mut _))).as_bool() }
    }

    /// 分片 sleep：`stop()` 要 join 这个线程，一次睡满 500ms 会让「停止」明显卡一下
    fn sleep_until_stop(stop: &AtomicBool) {
        let deadline = std::time::Instant::now() + POLL_INTERVAL;
        while !stop.load(Ordering::SeqCst) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            std::thread::sleep(left.min(std::time::Duration::from_millis(100)));
        }
    }

    fn signature_for_rule(rule_id: &str) -> String {
        format!("rule|{rule_id}")
    }

    fn signature_for_detect(features: &WindowFeatures) -> String {
        format!("detect|{}|{}", features.class_name, features.title)
    }

    pub fn spawn(
        app: tauri::AppHandle,
        rules: Arc<Mutex<Vec<PopupRule>>>,
        settings: Arc<Mutex<PopupSettings>>,
        ignore: Arc<Mutex<Vec<String>>>,
    ) -> Result<Inner, String> {
        use tauri::Emitter;

        let buffer = Arc::new(Mutex::new(VecDeque::<PopupHit>::new()));
        let handles = Arc::new(Mutex::new(HashMap::<u64, isize>::new()));
        let known = Arc::new(Mutex::new(HashMap::<isize, String>::new()));
        let acted = Arc::new(Mutex::new(HashMap::<isize, std::time::Instant>::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_buffer = Arc::clone(&buffer);
        let thread_handles = Arc::clone(&handles);
        let thread_known = Arc::clone(&known);
        let thread_acted = Arc::clone(&acted);
        let thread_stop = Arc::clone(&stop);
        let thread_rules = Arc::clone(&rules);
        let thread_settings = Arc::clone(&settings);
        let thread_ignore = Arc::clone(&ignore);

        let join = std::thread::Builder::new()
            .name("popup-watch".into())
            .spawn(move || {
                let mut minute_start = std::time::Instant::now();
                let mut minute_actions: u32 = 0;
                let mut dirty_rules = false;
                let mut last_flush = std::time::Instant::now();
                while !thread_stop.load(Ordering::SeqCst) {
                    let foreground = unsafe { GetForegroundWindow() };
                    let ignored = thread_ignore
                        .lock()
                        .map(|list| list.clone())
                        .unwrap_or_default();
                    let settings = thread_settings
                        .lock()
                        .map(|value| value.clone())
                        .unwrap_or_default();
                    let rules = thread_rules
                        .lock()
                        .map(|value| value.clone())
                        .unwrap_or_default();
                    if minute_start.elapsed() >= std::time::Duration::from_secs(60) {
                        minute_start = std::time::Instant::now();
                        minute_actions = 0;
                    }
                    let windows = collect_windows(foreground);
                    // 窗口销毁后 HWND 会被系统复用，旧句柄留着会让「关闭」点到别的窗口
                    let alive: std::collections::HashSet<isize> =
                        windows.iter().map(|(_, handle)| *handle).collect();
                    if let Ok(mut map) = thread_handles.lock() {
                        map.retain(|_, handle| window_alive(*handle));
                    }
                    if let Ok(mut map) = thread_known.lock() {
                        map.retain(|handle, _| alive.contains(handle));
                    }
                    if let Ok(mut map) = thread_acted.lock() {
                        map.retain(|handle, _| alive.contains(handle));
                    }
                    // 待处理记录只在窗口还开着时保留；拦截记录留作历史
                    let live_ids: std::collections::HashSet<u64> = thread_handles
                        .lock()
                        .map(|map| {
                            map.iter()
                                .filter(|(_, handle)| alive.contains(*handle))
                                .map(|(id, _)| *id)
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Ok(mut buffer) = thread_buffer.lock() {
                        buffer.retain(|hit| {
                            hit.auto_action.is_some()
                                || (live_ids.contains(&hit.id)
                                    && !ignored.iter().any(|process| {
                                        process.eq_ignore_ascii_case(&hit.process)
                                    }))
                        });
                    }

                    for (features, handle) in windows {
                        // 应用主窗口永远不动：即便规则是 scope=app / 类名恰好相同
                        if features.is_main {
                            continue;
                        }
                        if is_protected_process(&features.process)
                            || is_protected_class(&features.class_name)
                        {
                            continue;
                        }
                        if ignored
                            .iter()
                            .any(|process| process.eq_ignore_ascii_case(&features.process))
                        {
                            continue;
                        }

                        // 1) 规则优先：命中就动手并记一条拦截
                        if let Some(rule) = rules.iter().find(|rule| {
                            rule_matches(
                                rule,
                                &features.process,
                                &features.class_name,
                                &features.title,
                            )
                        }) {
                            let signature = signature_for_rule(&rule.id);
                            let logged = thread_known
                                .lock()
                                .map(|map| map.get(&handle) == Some(&signature))
                                .unwrap_or(false);
                            let ready = thread_acted
                                .lock()
                                .map(|map| {
                                    map.get(&handle).map_or(true, |at| {
                                        at.elapsed()
                                            >= std::time::Duration::from_millis(RETRY_AFTER_MS)
                                    })
                                })
                                .unwrap_or(true);
                            let mut intercepted = false;
                            if ready {
                                intercepted = apply_window_action(handle, &rule.action);
                                if let Ok(mut map) = thread_acted.lock() {
                                    map.insert(handle, std::time::Instant::now());
                                }
                            }
                            if !logged {
                                if let Ok(mut map) = thread_known.lock() {
                                    map.insert(handle, signature);
                                }
                                let mut reasons = vec![format!("命中规则：{}", rule_summary(rule))];
                                if ready && !intercepted {
                                    reasons.push(
                                        "自动处理被系统拒绝（窗口可能以管理员权限运行）".into(),
                                    );
                                }
                                let hit = PopupHit {
                                    id: next_hit_id(),
                                    at_ms: now_ms(),
                                    process: features.process.clone(),
                                    path: features.path.clone(),
                                    title: features.display_title(),
                                    class_name: features.class_name.clone(),
                                    width: features.width,
                                    height: features.height,
                                    position: features.position.clone(),
                                    score: 0,
                                    reasons,
                                    auto_action: if intercepted {
                                        Some(rule.action.clone())
                                    } else {
                                        None
                                    },
                                    rule_id: Some(rule.id.clone()),
                                    source: "rule".into(),
                                };
                                if let Ok(mut map) = thread_handles.lock() {
                                    map.insert(hit.id, handle);
                                }
                                push_hit(&thread_buffer, &hit);
                                let _ = app.emit("popup-detected", &hit);
                            }
                            if intercepted {
                                if let Ok(mut list) = thread_rules.lock() {
                                    if let Some(target) =
                                        list.iter_mut().find(|target| target.id == rule.id)
                                    {
                                        target.hits = target.hits.saturating_add(1);
                                        target.last_hit_ms = now_ms();
                                        dirty_rules = true;
                                    }
                                }
                            }
                            continue;
                        }

                        // 2) 没有规则才走启发式：分数 > 0 的可疑窗口进列表
                        let (score, reasons) = score_window(&features);
                        if score == 0 || reasons.is_empty() {
                            continue;
                        }
                        let signature = signature_for_detect(&features);
                        if thread_known
                            .lock()
                            .map(|map| map.get(&handle) == Some(&signature))
                            .unwrap_or(false)
                        {
                            continue;
                        }
                        if let Ok(mut map) = thread_known.lock() {
                            map.insert(handle, signature);
                        }
                        let mut hit = PopupHit {
                            id: next_hit_id(),
                            at_ms: now_ms(),
                            process: features.process.clone(),
                            path: features.path.clone(),
                            title: features.display_title(),
                            class_name: features.class_name.clone(),
                            width: features.width,
                            height: features.height,
                            position: features.position.clone(),
                            score,
                            reasons,
                            auto_action: None,
                            rule_id: None,
                            source: "detect".into(),
                        };
                        if let InterceptPlan::Intercept(action) = should_intercept(
                            &features,
                            hit.score,
                            &hit.reasons,
                            &settings,
                            &ignored,
                            minute_actions,
                        ) {
                            if apply_window_action(handle, action) {
                                if let Ok(mut map) = thread_acted.lock() {
                                    map.insert(handle, std::time::Instant::now());
                                }
                                minute_actions += 1;
                                hit.auto_action = Some(action.to_string());
                                hit.source = "smart".into();
                                hit.reasons.push("智能拦截".into());
                            } else {
                                hit.reasons.push(
                                    "智能拦截被系统拒绝（窗口可能以管理员权限运行）".into(),
                                );
                            }
                        }
                        if let Ok(mut map) = thread_handles.lock() {
                            map.insert(hit.id, handle);
                        }
                        push_hit(&thread_buffer, &hit);
                        let _ = app.emit("popup-detected", &hit);
                    }

                    if dirty_rules && last_flush.elapsed() >= RULE_FLUSH_INTERVAL {
                        let snapshot = thread_rules
                            .lock()
                            .map(|rules| rules.clone())
                            .unwrap_or_default();
                        let _ = save_rules(&snapshot);
                        dirty_rules = false;
                        last_flush = std::time::Instant::now();
                    }
                    sleep_until_stop(&thread_stop);
                }
                // 停线程前把命中次数落盘，别丢最后几十秒的统计
                let snapshot = thread_rules
                    .lock()
                    .map(|rules| rules.clone())
                    .unwrap_or_default();
                let _ = save_rules(&snapshot);
            })
            .map_err(|err| format!("启动弹窗拦截线程失败：{err}"))?;

        Ok(Inner {
            stop,
            join: Some(join),
            buffer,
            handles,
            known,
            acted,
        })
    }

    fn push_hit(buffer: &Arc<Mutex<VecDeque<PopupHit>>>, hit: &PopupHit) {
        if let Ok(mut buffer) = buffer.lock() {
            if buffer.len() >= HIT_LIMIT {
                buffer.pop_front();
            }
            buffer.push_back(hit.clone());
        }
    }

    /// 枚举所有可见顶层窗口，抓取打分需要的特征（带窗口句柄，供用户手动关闭时用）
    pub fn collect_windows(foreground: HWND) -> Vec<(WindowFeatures, isize)> {
        struct Collect {
            foreground: HWND,
            screen_w: i32,
            screen_h: i32,
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
            let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
            // GetWindow 在没有 owner 时返回 NULL；不要把它当成「有 owner」
            let is_owned = unsafe { GetWindow(hwnd, GW_OWNER) }
                .map(|owner| !owner.0.is_null())
                .unwrap_or(false);
            let mut rect = RECT::default();
            let has_rect = unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok();
            let width = if has_rect { rect.right - rect.left } else { 0 };
            let height = if has_rect { rect.bottom - rect.top } else { 0 };
            let foreground_pid = {
                let mut fg_pid = 0u32;
                unsafe { GetWindowThreadProcessId(collect.foreground, Some(&mut fg_pid)) };
                fg_pid
            };
            let (process, path) = process_info(pid);
            let features = WindowFeatures {
                has_title: title_len > 0,
                is_tool_window: (ex_style & WS_EX_TOOLWINDOW.0 as i32) != 0,
                is_no_activate: (ex_style & WS_EX_NOACTIVATE.0 as i32) != 0,
                is_topmost: (ex_style & WS_EX_TOPMOST.0 as i32) != 0,
                is_foreground_process: pid == foreground_pid,
                is_owned,
                is_resizable: (style & WS_THICKFRAME.0 as i32) != 0,
                is_main: false,
                position: if has_rect {
                    position_of(rect.left, rect.top, width, height, collect.screen_w, collect.screen_h)
                } else {
                    "任意位置".into()
                },
                width,
                height,
                class_name: String::from_utf16_lossy(&class_buf[..class_len.min(class_buf.len())]),
                title: String::from_utf16_lossy(&title_buf[..title_len.min(title_buf.len())]),
                process,
                path,
            };
            collect.items.push((features, hwnd.0 as isize));
            BOOL(1)
        }

        let mut collect = Collect {
            foreground,
            screen_w: unsafe { GetSystemMetrics(SM_CXSCREEN) },
            screen_h: unsafe { GetSystemMetrics(SM_CYSCREEN) },
            items: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(
                Some(callback),
                LPARAM(&mut collect as *mut Collect as isize),
            );
        }
        mark_main_windows(&mut collect.items);
        collect.items
    }

    /// 进程 pid → (进程名, 完整路径)；打不开进程时退回 `pid N`
    fn process_info(pid: u32) -> (String, String) {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return (format!("pid {pid}"), String::new());
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
                return (format!("pid {pid}"), String::new());
            }
            let path = String::from_utf16_lossy(&buffer[..size as usize]);
            let name = path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(&path)
                .to_string();
            (name, path)
        }
    }
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
            path: r"C:\Program Files\Chrome\chrome.exe".into(),
            is_foreground_process: true,
            ..Default::default()
        }
    }

    fn rule(scope: &str, class_name: &str, title: &str) -> PopupRule {
        PopupRule {
            id: "rule-1".into(),
            process: "PotPlayerMini64.exe".into(),
            path: String::new(),
            class_name: class_name.into(),
            title: title.into(),
            name: String::new(),
            position: String::new(),
            scope: scope.into(),
            action: "close".into(),
            enabled: true,
            hits: 0,
            last_hit_ms: 0,
            created_ms: 1,
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

    // ============================ 规则匹配 ============================

    #[test]
    fn rule_matches_process_and_class() {
        let rule = rule("class", "AdvertPopupWnd", "");
        assert!(rule_matches(
            &rule,
            "potplayermini64.exe",
            "advertpopupwnd",
            "随便什么标题"
        ));
        assert!(!rule_matches(&rule, "wps.exe", "AdvertPopupWnd", "标题"));
        assert!(!rule_matches(
            &rule,
            "PotPlayerMini64.exe",
            "AnotherClass",
            "标题"
        ));
    }

    #[test]
    fn rule_title_is_substring_and_optional() {
        let rule = rule("window", "AdvertPopupWnd", "活动");
        assert!(rule_matches(
            &rule,
            "PotPlayerMini64.exe",
            "AdvertPopupWnd",
            "限时活动弹窗"
        ));
        assert!(!rule_matches(
            &rule,
            "PotPlayerMini64.exe",
            "AdvertPopupWnd",
            "更新提示"
        ));

        let mut class_only = rule.clone();
        class_only.title.clear();
        assert!(rule_matches(
            &class_only,
            "PotPlayerMini64.exe",
            "AdvertPopupWnd",
            "任何标题"
        ));
    }

    #[test]
    fn app_scope_matches_any_window_of_process() {
        let rule = rule("app", "", "");
        assert!(rule_matches(&rule, "PotPlayerMini64.exe", "AnyClass", "AnyTitle"));
        assert!(rule_matches(&rule, "potplayermini64.exe", "", ""));
        assert!(!rule_matches(&rule, "wps.exe", "", ""));
    }

    #[test]
    fn disabled_rule_never_matches() {
        let mut rule = rule("app", "", "");
        rule.enabled = false;
        assert!(!rule_matches(&rule, "PotPlayerMini64.exe", "", ""));
    }

    // ============================ 规则落库 ============================

    fn draft(scope: &str, class_name: &str, title: &str) -> PopupRuleDraft {
        PopupRuleDraft {
            process: "  wps.exe ".into(),
            path: r"C:\WPS\wps.exe".into(),
            class_name: class_name.into(),
            title: title.into(),
            name: String::new(),
            position: String::new(),
            scope: scope.into(),
            action: "hide".into(),
        }
    }

    #[test]
    fn draft_trims_and_normalizes() {
        let rule = draft("class", " wpsadwnd ", "会被丢掉").into_rule().unwrap();
        assert_eq!(rule.process, "wps.exe");
        assert_eq!(rule.class_name, "wpsadwnd");
        assert_eq!(rule.title, "", "class 范围不应保留标题");
        assert_eq!(rule.action, "hide");
        assert_eq!(rule.scope, "class");
        assert!(rule.enabled);
        assert!(!rule.id.is_empty());
    }

    #[test]
    fn draft_app_scope_drops_matchers() {
        let rule = draft("app", "SomeClass", "SomeTitle").into_rule().unwrap();
        assert_eq!(rule.class_name, "");
        assert_eq!(rule.title, "");
        assert_eq!(rule.scope, "app");
        assert_eq!(rule.name, "全部弹窗");
        assert_eq!(rule.position, "任意位置");
    }

    #[test]
    fn draft_requires_matchers_for_class_scope() {
        let err = draft("class", "", "").into_rule().unwrap_err();
        assert!(err.contains("拦截范围"), "{err}");
    }

    #[test]
    fn draft_rejects_protected_processes() {
        let mut protected = draft("app", "", "");
        protected.process = "explorer.exe".into();
        let err = protected.into_rule().unwrap_err();
        assert!(err.contains("系统进程"), "{err}");

        let mut blank = draft("app", "", "");
        blank.process = "   ".into();
        assert!(blank.into_rule().is_err());
    }

    #[test]
    fn duplicate_rule_is_merged() {
        let mut rules = vec![];
        let first = rule("class", "AdvertPopupWnd", "");
        let id = merge_rule(&mut rules, first);
        assert_eq!(rules.len(), 1);

        let mut again = rule("class", "advertpopupwnd", "");
        again.action = "hide".into();
        again.enabled = false;
        let merged = merge_rule(&mut rules, again);
        assert_eq!(rules.len(), 1, "同样的进程 + 类名 + 标题不应重复");
        assert_eq!(merged, id);
        assert_eq!(rules[0].action, "hide");
        assert!(rules[0].enabled);
    }

    #[test]
    fn json_with_utf8_bom_is_accepted() {
        assert_eq!(strip_bom("\u{feff}[]"), "[]");
        assert_eq!(strip_bom("[]"), "[]");
    }

    #[test]
    fn rules_are_sanitized_on_load() {
        let rules = sanitize_rules(vec![
            PopupRule {
                process: "  qq.exe ".into(),
                scope: "weird".into(),
                action: "HIDE".into(),
                class_name: "  ".into(),
                ..Default::default()
            },
            PopupRule {
                process: "  ".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(rules.len(), 1, "空进程名的坏条目要丢掉");
        assert_eq!(rules[0].process, "qq.exe");
        assert_eq!(rules[0].scope, "class");
        assert_eq!(rules[0].action, "hide");
    }

    // ============================ 智能拦截闸门 ============================

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
            enabled: true,
            action: "close".into(),
            smart: true,
            min_score: 8,
            max_per_minute: 6,
        }
    }

    #[test]
    fn gate_is_closed_when_smart_disabled() {
        let features = features_for_gate();
        let (score, reasons) = score_window(&features);
        let mut settings = intercept_settings();
        settings.smart = false;
        assert_eq!(
            should_intercept(&features, score, &reasons, &settings, &[], 0),
            InterceptPlan::Skip("智能拦截未开启")
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
            should_intercept(
                &features,
                20,
                &["只有一个原因".to_string()],
                &intercept_settings(),
                &[],
                0
            ),
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
            enabled: true,
            action: "whatever".into(),
            smart: true,
            min_score: 0,
            max_per_minute: 999,
        }
        .normalized();
        assert_eq!(settings.action, "close");
        assert_eq!(settings.min_score, 5);
        assert_eq!(settings.max_per_minute, 30);

        let settings = PopupSettings {
            enabled: false,
            action: "HIDE".into(),
            smart: false,
            min_score: 100,
            max_per_minute: 0,
        }
        .normalized();
        assert_eq!(settings.action, "hide", "动作大小写不敏感，手改 JSON 也能识别");
        assert_eq!(settings.min_score, 30);
        assert_eq!(settings.max_per_minute, 1);
    }

    #[test]
    fn default_settings_are_off() {
        let settings = PopupSettings::default();
        assert!(!settings.enabled, "默认必须是关闭状态");
        assert!(!settings.smart, "默认不开智能拦截");
        assert_eq!(settings.action, "close");
        assert_eq!(settings.min_score, 10);
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

    #[test]
    fn rule_summary_is_readable() {
        assert_eq!(
            rule_summary(&rule("class", "WpsAdWnd", "")),
            "类名 WpsAdWnd"
        );
        assert_eq!(
            rule_summary(&rule("window", "WpsAdWnd", "热点")),
            "类名 WpsAdWnd · 标题含「热点」"
        );
        assert_eq!(
            rule_summary(&rule("app", "", "")),
            "该程序的所有弹窗"
        );
    }

    // ======================== 主窗口保护 / 展示字段 ========================

    fn sample_window(
        process: &str,
        title: &str,
        width: i32,
        height: i32,
        owned: bool,
        tool: bool,
    ) -> WindowFeatures {
        WindowFeatures {
            has_title: true,
            width,
            height,
            process: process.into(),
            title: title.into(),
            is_owned: owned,
            is_tool_window: tool,
            ..Default::default()
        }
    }

    #[test]
    fn main_window_is_protected_within_process() {
        let mut items = vec![
            (
                sample_window("wps.exe", "WPS Office 主窗口", 1200, 800, false, false),
                1,
            ),
            (
                sample_window("wps.exe", "限时红包", 420, 260, true, true),
                2,
            ),
            (
                sample_window("chrome.exe", "新标签页", 1400, 900, false, false),
                3,
            ),
        ];
        mark_main_windows(&mut items);
        assert!(items[0].0.is_main, "面积最大的无 owner 窗口是主窗口");
        assert!(!items[1].0.is_main, "工具窗口 / 附属窗口不能当主窗口");
        assert!(items[2].0.is_main);
    }

    #[test]
    fn main_window_protection_survives_app_scope_rule() {
        let main = sample_window("wps.exe", "WPS Office 主窗口", 1200, 800, false, false);
        let ad = sample_window("wps.exe", "限时红包", 420, 260, true, true);
        let mut app_scope = rule("app", "", "");
        app_scope.process = "wps.exe".into();
        assert!(rule_matches(
            &app_scope,
            &main.process,
            &main.class_name,
            &main.title
        ));
        // 匹配上了也没用：调用方必须先看 is_main 再动手
        let mut items = vec![(main, 1), (ad, 2)];
        mark_main_windows(&mut items);
        assert!(items[0].0.is_main);
        assert!(!items[1].0.is_main);
    }

    #[test]
    fn position_is_classified_from_window_center() {
        assert_eq!(
            position_of(1500, 800, 400, 260, 1920, 1080),
            "右下窗口"
        );
        assert_eq!(
            position_of(760, 400, 400, 280, 1920, 1080),
            "中间窗口"
        );
        assert_eq!(
            position_of(60, 60, 400, 260, 1920, 1080),
            "左上窗口"
        );
        assert_eq!(position_of(0, 0, 0, 0, 1920, 1080), "任意位置");
    }

    #[test]
    fn rule_name_uses_huorong_style_labels() {
        assert_eq!(rule_name("window", "限时红包", "X"), "红包");
        assert_eq!(rule_name("class", "热点推荐", "X"), "软件推荐");
        assert_eq!(rule_name("class", "今日特惠广告", "X"), "广告弹窗");
        assert_eq!(rule_name("app", "", ""), "全部弹窗");
        assert_eq!(rule_name("class", "", "WpsAdWnd"), "广告弹窗");
    }

    #[test]
    fn old_rules_get_name_and_position_backfilled() {
        let mut legacy = rule("class", "WpsAdWnd", "");
        legacy.name.clear();
        legacy.position.clear();
        let sanitized = sanitize_rules(vec![legacy]);
        assert_eq!(sanitized[0].name, "广告弹窗");
        assert_eq!(sanitized[0].position, "任意位置");
    }
}
