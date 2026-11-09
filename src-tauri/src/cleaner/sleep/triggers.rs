//! 屏幕休眠的自动保持规则（对齐 Amphetamine 的 Triggers）。
//!
//! 三类条件，任一满足即自动保持唤醒（多条规则是「或」关系）：
//! - `app_running`：指定进程在运行（按进程名，忽略大小写，包含即可）
//! - `ac_power`：接通电源（笔记本合盖外场景）
//! - `external_display`：连接了外接显示器
//!
//! 规则存 `<data_dir>/sleep-triggers.json`，不动 AppConfig。
//!
//! 交互约定：
//! - 规则只会在「当前没有手动会话」时接管（手动开启优先，不会被规则停掉）；
//! - 条件不再满足时，只会释放「规则自己开」的会话，不会误停手动会话；
//! - 监控线程 5 秒一轮，命中/失效时通过事件 `sleep-triggers-changed` 通知界面。
//!
//! 平台探测：进程列表用 `sysinfo`（跨平台）；电源用 `pmset -g batt`（macOS）/
//! `GetSystemPowerStatus`（Windows）；外接显示器用 CoreGraphics 活动屏幕数（macOS）/
//! `GetSystemMetrics(SM_CMONITORS)`（Windows）。都是只读查询，不需要管理员权限。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{release_trigger, start_from_trigger};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const FILE_NAME: &str = "sleep-triggers.json";
const MAX_TRIGGERS: usize = 20;

// ============================== 数据模型 ==============================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// 指定进程在运行时
    AppRunning,
    /// 接通电源时
    AcPower,
    /// 连接外接显示器时
    ExternalDisplay,
}

impl TriggerKind {
    fn slug(self) -> &'static str {
        match self {
            TriggerKind::AppRunning => "app",
            TriggerKind::AcPower => "ac",
            TriggerKind::ExternalDisplay => "display",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepTrigger {
    pub id: String,
    pub kind: TriggerKind,
    /// `app_running` 的进程名/片段；另外两类为空
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct DetectedConditions {
    /// 当前是否接着电源
    pub on_ac_power: bool,
    /// 当前是否连接了外接显示器
    pub external_display: bool,
    /// 当前命中的规则 id（即使被手动会话挡住也照样上报）
    pub matched_trigger: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggersState {
    pub armed: bool,
    pub triggers: Vec<SleepTrigger>,
    /// 当前由哪条规则真正生效（None 表示没有规则在保持）
    pub active_trigger: Option<String>,
    pub detected: DetectedConditions,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredTriggers {
    #[serde(default)]
    armed: bool,
    #[serde(default)]
    triggers: Vec<SleepTrigger>,
}

// ============================== 纯逻辑 ==============================

/// 进程名归一化：去空白、转小写、去掉 `.exe` / `.app` / `.desktop` 后缀，
/// 方便用户直接填「Typora」「Code」这种直观名字。
fn normalize_name(raw: &str) -> String {
    let mut value = raw.trim().to_ascii_lowercase();
    for suffix in [".exe", ".app", ".desktop"] {
        if let Some(stripped) = value.strip_suffix(suffix) {
            value = stripped.to_string();
        }
    }
    value
}

/// 进程名是否满足规则：完全相等或包含（规则值是子串）。
pub fn app_matches(value: &str, process: &str) -> bool {
    let needle = normalize_name(value);
    if needle.is_empty() {
        return false;
    }
    let hay = normalize_name(process);
    hay == needle || hay.contains(&needle)
}

/// 当前命中的第一条启用规则（规则之间是「或」）。
pub fn evaluate(triggers: &[SleepTrigger], probe: &ProbeResult) -> Option<String> {
    triggers
        .iter()
        .filter(|trigger| trigger.enabled)
        .find(|trigger| match trigger.kind {
            TriggerKind::AppRunning => probe
                .processes
                .iter()
                .any(|process| app_matches(&trigger.value, process)),
            TriggerKind::AcPower => probe.on_ac_power,
            TriggerKind::ExternalDisplay => probe.external_display,
        })
        .map(|trigger| trigger.id.clone())
}

fn add_trigger(
    stored: &mut StoredTriggers,
    kind: TriggerKind,
    value: &str,
) -> Result<String, String> {
    if stored.triggers.len() >= MAX_TRIGGERS {
        return Err(format!("自动规则最多 {MAX_TRIGGERS} 条"));
    }
    let value = value.trim().to_string();
    if kind == TriggerKind::AppRunning && value.is_empty() {
        return Err("请填进程名（如 Typora、Code）".into());
    }
    let duplicate = stored.triggers.iter().any(|trigger| {
        trigger.kind == kind
            && (kind != TriggerKind::AppRunning
                || normalize_name(&trigger.value) == normalize_name(&value))
    });
    if duplicate {
        return Err("这条规则已经存在".into());
    }
    let id = format!("{}-{}", kind.slug(), now_ms());
    stored.triggers.push(SleepTrigger {
        id: id.clone(),
        kind,
        value: if kind == TriggerKind::AppRunning { value } else { String::new() },
        enabled: true,
    });
    Ok(id)
}

fn remove_trigger(stored: &mut StoredTriggers, id: &str) -> bool {
    let before = stored.triggers.len();
    stored.triggers.retain(|trigger| trigger.id != id);
    stored.triggers.len() != before
}

fn set_trigger_enabled(stored: &mut StoredTriggers, id: &str, enabled: bool) -> bool {
    let Some(trigger) = stored.triggers.iter_mut().find(|trigger| trigger.id == id) else {
        return false;
    };
    trigger.enabled = enabled;
    true
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

// ============================== 探测 ==============================

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeResult {
    pub on_ac_power: bool,
    pub external_display: bool,
    pub processes: Vec<String>,
}

fn probe_conditions() -> ProbeResult {
    ProbeResult {
        on_ac_power: probe_on_ac_power(),
        external_display: probe_external_display(),
        processes: running_processes(),
    }
}

/// 进程名列表（只读刷新，复用同一个 `System`，避免每轮重新分配）。
fn running_processes() -> Vec<String> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

    static SYSTEM: Mutex<Option<System>> = Mutex::new(None);
    let mut guard = match SYSTEM.lock() {
        Ok(guard) => guard,
        Err(poison) => poison.into_inner(),
    };
    if guard.is_none() {
        let Ok(system) = System::new() else {
            return Vec::new();
        };
        *guard = Some(system);
    }
    let Some(system) = guard.as_mut() else {
        return Vec::new();
    };
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing(),
    );
    system
        .processes()
        .values()
        .map(|process| process.name().to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

#[cfg(target_os = "macos")]
fn probe_on_ac_power() -> bool {
    std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("AC Power"))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn probe_external_display() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max_displays: u32, displays: *mut u32, count: *mut u32) -> i32;
    }
    let mut count: u32 = 0;
    // 只问数量：不传数组，避免多一次分配
    let status = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
    status == 0 && count > 1
}

#[cfg(target_os = "windows")]
fn probe_on_ac_power() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.is_ok() && status.ACLineStatus == 1
}

#[cfg(target_os = "windows")]
fn probe_external_display() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
    unsafe { GetSystemMetrics(SM_CMONITORS) > 1 }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn probe_on_ac_power() -> bool {
    false
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn probe_external_display() -> bool {
    false
}

// ============================== 持久化 + 全局状态 ==============================

static STORE: Mutex<Option<Arc<Mutex<StoredTriggers>>>> = Mutex::new(None);
static MONITOR: Mutex<Option<Inner>> = Mutex::new(None);
/// 监控线程最近一次探测结果，供 `state()` 复用（不在监控时按需探测）
static LAST_PROBE: Mutex<Option<ProbeResult>> = Mutex::new(None);

struct Inner {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Inner {
    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn config_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::cleaner::app_data_dir()?.join(FILE_NAME))
}

fn load_stored() -> StoredTriggers {
    let Ok(path) = config_path() else {
        return StoredTriggers::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return StoredTriggers::default();
    };
    serde_json::from_str::<StoredTriggers>(&raw).unwrap_or_default()
}

fn save_stored(stored: &StoredTriggers) -> Result<(), String> {
    let path = config_path()?;
    let raw = serde_json::to_string_pretty(stored).map_err(|err| format!("序列化自动规则失败：{err}"))?;
    std::fs::write(&path, raw).map_err(|err| format!("写入自动规则失败：{err}"))
}

fn store() -> Arc<Mutex<StoredTriggers>> {
    let mut slot = STORE.lock().unwrap_or_else(|poison| poison.into_inner());
    if slot.is_none() {
        *slot = Some(Arc::new(Mutex::new(load_stored())));
    }
    slot.as_ref().expect("刚初始化过").clone()
}

fn snapshot() -> StoredTriggers {
    store()
        .lock()
        .map(|stored| stored.clone())
        .unwrap_or_default()
}

fn detected_snapshot() -> DetectedConditions {
    // 用 try_lock 而不是 lock：set_armed 正在停监控/启监控时这里可以现场探测，
    // 避免 state() 与监控线程退出路径互相等待。
    let monitor_running = MONITOR
        .try_lock()
        .map(|monitor| monitor.is_some())
        .unwrap_or(false);
    let probe = if monitor_running {
        LAST_PROBE
            .lock()
            .ok()
            .and_then(|probe| probe.clone())
            .unwrap_or_else(probe_conditions)
    } else {
        probe_conditions()
    };
    let matched_trigger = evaluate(&snapshot().triggers, &probe);
    DetectedConditions {
        on_ac_power: probe.on_ac_power,
        external_display: probe.external_display,
        matched_trigger,
    }
}

pub fn state() -> TriggersState {
    let stored = snapshot();
    TriggersState {
        armed: stored.armed,
        triggers: stored.triggers,
        active_trigger: super::status().active_trigger,
        detected: detected_snapshot(),
    }
}

// ============================== 命令入口 ==============================

/// 开 / 关自动规则总开关；开启时拉起监控线程，关闭时释放规则开的会话。
pub fn set_armed(app: tauri::AppHandle, armed: bool) -> Result<TriggersState, String> {
    let shared = store();
    {
        let mut stored = shared.lock().map_err(|_| "自动规则状态损坏".to_string())?;
        stored.armed = armed;
        save_stored(&stored)?;
    }

    // 注意：先把 MONITOR 里的 Inner 取出来并释放锁，再 stop()。
    // stop() 会 join 监控线程，而线程退出前可能调用 emit_state → state() → 这里再锁
    // MONITOR；如果持有锁 join 就会死锁（表现为「开了就关不掉 / 页面卡住」）。
    let to_stop = {
        let mut monitor = MONITOR.lock().unwrap_or_else(|poison| poison.into_inner());
        if armed {
            if monitor.is_none() {
                *monitor = Some(spawn_monitor(Arc::clone(&shared), app)?);
            }
            None
        } else {
            monitor.take()
        }
    };
    if let Some(inner) = to_stop {
        inner.stop();
    }
    Ok(state())
}

pub fn add(app: tauri::AppHandle, kind: TriggerKind, value: &str) -> Result<TriggersState, String> {
    {
        let shared = store();
        let mut stored = shared.lock().map_err(|_| "自动规则状态损坏".to_string())?;
        add_trigger(&mut stored, kind, value)?;
        save_stored(&stored)?;
    }
    // 规则改了立刻让界面拿到新状态；监控线程下一轮自动用新规则
    let _ = app;
    Ok(state())
}

pub fn remove(app: tauri::AppHandle, id: &str) -> Result<TriggersState, String> {
    {
        let shared = store();
        let mut stored = shared.lock().map_err(|_| "自动规则状态损坏".to_string())?;
        if !remove_trigger(&mut stored, id) {
            return Err("规则不存在".into());
        }
        save_stored(&stored)?;
    }
    let _ = app;
    Ok(state())
}

pub fn set_enabled(
    app: tauri::AppHandle,
    id: &str,
    enabled: bool,
) -> Result<TriggersState, String> {
    {
        let shared = store();
        let mut stored = shared.lock().map_err(|_| "自动规则状态损坏".to_string())?;
        if !set_trigger_enabled(&mut stored, id, enabled) {
            return Err("规则不存在".into());
        }
        save_stored(&stored)?;
    }
    let _ = app;
    Ok(state())
}

fn spawn_monitor(
    shared: Arc<Mutex<StoredTriggers>>,
    app: tauri::AppHandle,
) -> Result<Inner, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let join = std::thread::Builder::new()
        .name("sleep-triggers".into())
        .spawn(move || {
            let mut current: Option<String> = None;
            while !thread_stop.load(Ordering::SeqCst) {
                let (armed, triggers) = {
                    let stored = match shared.lock() {
                        Ok(stored) => stored.clone(),
                        Err(poison) => poison.into_inner().clone(),
                    };
                    (stored.armed, stored.triggers)
                };
                if !armed {
                    break;
                }
                let probe = probe_conditions();
                if let Ok(mut last) = LAST_PROBE.lock() {
                    *last = Some(probe.clone());
                }
                let matched = evaluate(&triggers, &probe);
                if matched != current {
                    if let Some(previous) = current.take() {
                        release_trigger(&previous);
                    }
                    if let Some(next) = matched {
                        if start_from_trigger(&next).unwrap_or(false) {
                            current = Some(next);
                        }
                    }
                    emit_state(&app);
                }
                // 分段睡：关开关不用等满 5 秒
                for _ in 0..(POLL_INTERVAL.as_millis() / 100) {
                    if thread_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            // 线程退出（关开关/应用退出）时把自己开的会话还回去
            if let Some(previous) = current.take() {
                release_trigger(&previous);
            }
            if let Ok(mut last) = LAST_PROBE.lock() {
                *last = None;
            }
        })
        .map_err(|err| format!("启动自动规则监控失败：{err}"))?;
    Ok(Inner {
        stop,
        join: Some(join),
    })
}

fn emit_state(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("sleep-triggers-changed", state());
}

// ============================== 测试 ==============================

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(with_apps: &[&str]) -> ProbeResult {
        ProbeResult {
            on_ac_power: false,
            external_display: false,
            processes: with_apps.iter().map(|name| name.to_string()).collect(),
        }
    }

    fn rule(id: &str, kind: TriggerKind, value: &str) -> SleepTrigger {
        SleepTrigger {
            id: id.to_string(),
            kind,
            value: value.to_string(),
            enabled: true,
        }
    }

    #[test]
    fn app_name_matching_is_forgiving() {
        assert!(app_matches("Typora", "Typora"));
        assert!(app_matches("typora", "Typora"));
        assert!(app_matches("Typora.app", "Typora"));
        assert!(app_matches("Code", "Code Helper (Renderer)"));
        // 规则值是子串：填 Code 也会命中 Visual Studio Code / Code Helper
        assert!(app_matches("Code", "Visual Studio Code"));
        assert!(!app_matches("", "Typora"));
        assert!(!app_matches("Safari", "Typora"));
    }

    #[test]
    fn evaluate_uses_or_semantics() {
        let triggers = vec![
            rule("app-1", TriggerKind::AppRunning, "Typora"),
            rule("ac-1", TriggerKind::AcPower, ""),
        ];
        assert_eq!(
            evaluate(&triggers, &probe(&["Typora"])),
            Some("app-1".into())
        );
        let mut on_ac = probe(&[]);
        on_ac.on_ac_power = true;
        assert_eq!(evaluate(&triggers, &on_ac), Some("ac-1".into()));
        assert_eq!(evaluate(&triggers, &probe(&[])), None);
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let mut triggers = vec![rule("app-1", TriggerKind::AppRunning, "Typora")];
        triggers[0].enabled = false;
        assert_eq!(evaluate(&triggers, &probe(&["Typora"])), None);
    }

    #[test]
    fn add_trigger_validates_and_dedupes() {
        let mut stored = StoredTriggers::default();
        assert!(add_trigger(&mut stored, TriggerKind::AppRunning, "  ").is_err());
        let id = add_trigger(&mut stored, TriggerKind::AppRunning, "Typora").unwrap();
        assert!(!id.is_empty());
        assert_eq!(stored.triggers.len(), 1);
        // 同一个进程（大小写不同）不允许重复
        assert!(add_trigger(&mut stored, TriggerKind::AppRunning, "typora").is_err());
        // 环境类规则只允许一条
        add_trigger(&mut stored, TriggerKind::AcPower, "").unwrap();
        assert!(add_trigger(&mut stored, TriggerKind::AcPower, "").is_err());
        add_trigger(&mut stored, TriggerKind::ExternalDisplay, "").unwrap();
    }

    #[test]
    fn remove_and_toggle_rules() {
        let mut stored = StoredTriggers::default();
        let id = add_trigger(&mut stored, TriggerKind::AppRunning, "Typora").unwrap();
        assert!(set_trigger_enabled(&mut stored, &id, false));
        assert!(!stored.triggers[0].enabled);
        assert!(remove_trigger(&mut stored, &id));
        assert!(stored.triggers.is_empty());
        assert!(!remove_trigger(&mut stored, &id));
        assert!(!set_trigger_enabled(&mut stored, "missing", true));
    }

    #[test]
    fn stored_triggers_round_trip() {
        let mut stored = StoredTriggers::default();
        stored.armed = true;
        add_trigger(&mut stored, TriggerKind::AppRunning, "Typora").unwrap();
        let raw = serde_json::to_string(&stored).unwrap();
        let parsed: StoredTriggers = serde_json::from_str(&raw).unwrap();
        assert!(parsed.armed);
        assert_eq!(parsed.triggers, stored.triggers);
        // 老文件缺字段也能读（armed / enabled 有默认值）
        let legacy: StoredTriggers = serde_json::from_str("{}").unwrap();
        assert!(!legacy.armed);
        assert!(legacy.triggers.is_empty());
    }

    #[test]
    fn detected_snapshot_does_not_wait_for_monitor_lock() {
        // 模拟 set_armed 持有 MONITOR 的瞬间：state()/detected_snapshot 不能被卡住，
        // 否则监控线程退出路径（join 前 emit_state）会与它互相等待，表现为开关卡死。
        let guard = MONITOR.lock().unwrap_or_else(|poison| poison.into_inner());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = detected_snapshot();
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "detected_snapshot 不应等待 MONITOR 锁"
        );
        drop(guard);
        worker.join().unwrap();
    }

    #[test]
    fn default_state_is_disarmed() {
        assert!(!StoredTriggers::default().armed);
        assert!(StoredTriggers::default().triggers.is_empty());
    }

    /// 手动探针：打印当前机器探测到的条件（`cargo test -- --ignored --nocapture`）
    #[test]
    #[ignore]
    fn manual_print_conditions() {
        let probe = probe_conditions();
        println!(
            "on_ac_power={} external_display={} processes={}",
            probe.on_ac_power,
            probe.external_display,
            probe.processes.len()
        );
        assert!(!probe.processes.is_empty(), "进程列表不应该为空");
    }
}
