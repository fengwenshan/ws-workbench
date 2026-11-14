//! 「屏幕休眠」页签：防休眠（阻止系统空闲睡眠 / 显示器睡眠）。
//!
//! 设计见 docs/superpowers/specs/2026-11-07-system-tools-design.md 4.1。
//!
//! 状态只在本进程内有效：应用退出（含崩溃）即释放，不做登录级保活。
//!
//! 自动规则（Triggers）在 `sleep::triggers` 里：指定 App 运行 / 接通电源 /
//! 连接外接显示器时自动开启保持唤醒，条件不满足自动释放；手动开启优先。
//!
//! 本文件只放共享会话状态机与门面：平台实现在 `macos.rs` / `windows.rs` /
//! `unsupported.rs`，两个平台文件互不引用。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub mod triggers;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;
#[cfg(target_os = "windows")]
use windows as platform;

#[derive(Debug, Clone, Serialize, Default)]
pub struct KeepAwakeStatus {
    /// 是否正在阻止空闲睡眠
    pub active: bool,
    /// true 表示「允许显示器睡眠」，即只拦系统空闲睡眠
    pub allow_display_sleep: bool,
    /// 开始时刻（epoch 毫秒）；未激活为 None
    pub started_at_ms: Option<u64>,
    /// 剩余秒数；无限时长与未激活都是 None
    pub remaining_secs: Option<u64>,
    /// 平台限制说明（直接展示给用户）
    pub platform_hint: String,
    /// 当前会话由哪条自动规则开启（手动开启为 None）
    pub active_trigger: Option<String>,
}

/// 会话来源：手动开启优先，自动规则只能在没有会话时接管
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Manual,
    Trigger(String),
}

struct Session {
    started_at_ms: u64,
    allow_display_sleep: bool,
    expires_at: Option<Instant>,
    /// 每次 start/stop 自增；定时任务只释放自己那一代
    generation: u64,
    /// 平台资源，Drop 时释放
    _guard: platform::Guard,
    origin: Origin,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn hint(allow_display_sleep: bool) -> String {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        return "当前平台不支持保持唤醒。".to_string();
    }
    let base = if cfg!(target_os = "windows") {
        "屏保、合盖与主动睡眠不受控，现代待机下可能被系统挂起。"
    } else {
        "合盖休眠不在支持范围。"
    };
    let head = if allow_display_sleep {
        "已阻止系统空闲睡眠，显示器仍会正常睡眠"
    } else if cfg!(target_os = "windows") {
        "已阻止系统空闲睡眠，显示器保持常亮"
    } else {
        "已阻止系统空闲睡眠和显示器关闭，显示器保持常亮"
    };
    format!("{head}；{base}")
}

fn status_from(session: &Session) -> KeepAwakeStatus {
    KeepAwakeStatus {
        active: true,
        allow_display_sleep: session.allow_display_sleep,
        started_at_ms: Some(session.started_at_ms),
        remaining_secs: session
            .expires_at
            .map(|expires_at| expires_at.saturating_duration_since(Instant::now()).as_secs()),
        platform_hint: hint(session.allow_display_sleep),
        active_trigger: match &session.origin {
            Origin::Manual => None,
            Origin::Trigger(id) => Some(id.clone()),
        },
    }
}

fn inactive_status() -> KeepAwakeStatus {
    KeepAwakeStatus {
        active: false,
        allow_display_sleep: false,
        started_at_ms: None,
        remaining_secs: None,
        platform_hint: hint(false),
        active_trigger: None,
    }
}

/// 取状态；顺带惰性回收已到期的会话（定时任务被挂起/延迟时也不会有残留断言）。
pub fn status() -> KeepAwakeStatus {
    let mut slot = SESSION.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(session) = slot.as_ref() {
        if session
            .expires_at
            .is_some_and(|expires_at| expires_at <= Instant::now())
        {
            // Drop 掉会话即释放平台断言
            *slot = None;
            return inactive_status();
        }
        return status_from(session);
    }
    inactive_status()
}

/// 开始保持唤醒。已在保持时先释放旧断言再按新参数重建。
///
/// `duration_secs` 为 None 或 0 表示无限时长。
pub fn start(
    duration_secs: Option<u64>,
    allow_display_sleep: bool,
) -> Result<KeepAwakeStatus, String> {
    start_inner(duration_secs, allow_display_sleep, Origin::Manual)
}

/// 自动规则触发：没有会话时才开启（手动会话优先，不覆盖）。
///
/// 返回本次是否真的新开了会话，方便调用方决定是否提示。
pub fn start_from_trigger(trigger_id: &str) -> Result<bool, String> {
    if status().active {
        return Ok(false);
    }
    start_inner(None, false, Origin::Trigger(trigger_id.to_string()))?;
    Ok(true)
}

/// 自动规则释放：只有当前会话正是这条规则开的才停。
///
/// 返回是否真的停了，避免规则之间互相误停。
pub fn release_trigger(trigger_id: &str) -> bool {
    let mut slot = SESSION.lock().unwrap_or_else(|poison| poison.into_inner());
    let matched = slot.as_ref().is_some_and(|session| {
        session.origin == Origin::Trigger(trigger_id.to_string())
    });
    if matched {
        *slot = None;
        return true;
    }
    false
}

fn start_inner(
    duration_secs: Option<u64>,
    allow_display_sleep: bool,
    origin: Origin,
) -> Result<KeepAwakeStatus, String> {
    let duration = match duration_secs {
        Some(secs) if secs > 0 => Some(secs),
        _ => None,
    };

    // 先建新断言：失败时保持原会话不变，避免「点了没反应还把旧的停了」
    let guard = platform::acquire(allow_display_sleep)?;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let started_at_ms = now_ms();
    let expires_at = duration.map(|secs| Instant::now() + std::time::Duration::from_secs(secs));

    let mut slot = SESSION.lock().unwrap_or_else(|poison| poison.into_inner());
    *slot = Some(Session {
        started_at_ms,
        allow_display_sleep,
        expires_at,
        generation,
        _guard: guard,
        origin,
    });
    let status = slot.as_ref().map(status_from).unwrap_or_else(inactive_status);
    drop(slot);

    // 到点自动释放；status() 里还有一道惰性回收兜底
    if let Some(secs) = duration {
        let _ = std::thread::Builder::new()
            .name("keepawake-timer".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                release_if_generation(generation);
            });
    }

    Ok(status)
}

fn release_if_generation(generation: u64) {
    let mut slot = SESSION.lock().unwrap_or_else(|poison| poison.into_inner());
    if slot
        .as_ref()
        .is_some_and(|session| session.generation == generation)
    {
        *slot = None;
    }
}

/// 停止保持唤醒（幂等：本来就没在保持时也返回当前状态）。
pub fn stop() -> Result<KeepAwakeStatus, String> {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let mut slot = SESSION.lock().unwrap_or_else(|poison| poison.into_inner());
    let had_session = slot.take();
    drop(slot);
    drop(had_session); // 释放平台断言
    Ok(inactive_status())
}

// ============================== 测试辅助 ==============================

/// 会话状态是进程级全局量，测试必须串行跑，否则互相把对方的会话停掉。
/// 放在 `sleep` 模块层是因为平台文件里的测试也要用同一把锁。
#[cfg(test)]
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
fn serial() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

// ============================== 测试 ==============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_session_is_released_by_same_trigger() {
        let _serial = serial();
        // 先清掉可能残留的会话
        let _ = stop();
        assert!(start_from_trigger("app-1").expect("trigger start"));
        assert!(status().active);
        assert_eq!(status().active_trigger.as_deref(), Some("app-1"));

        // 别的规则不能停它
        assert!(!release_trigger("ac-1"));
        assert!(status().active);

        // 自己可以停
        assert!(release_trigger("app-1"));
        assert!(!status().active);
    }

    #[test]
    fn trigger_does_not_override_manual_session() {
        let _serial = serial();
        let _ = stop();
        start(Some(60), false).expect("manual start");
        // 手动会话在跑时，规则不接管也不覆盖
        assert!(!start_from_trigger("app-1").expect("trigger start"));
        assert_eq!(status().active_trigger, None, "仍然算手动会话");
        let _ = stop();
    }

    #[test]
    fn manual_session_survives_trigger_release() {
        let _serial = serial();
        let _ = stop();
        start(None, false).expect("manual start");
        // 规则退出时不应把手动会话一起停掉
        assert!(!release_trigger("app-1"));
        assert!(status().active);
        assert_eq!(status().active_trigger, None);
        let _ = stop();
    }

    #[test]
    fn starts_and_stops() {
        let _serial = serial();
        let started = start(None, false).expect("start");
        assert!(started.active);
        assert!(started.started_at_ms.is_some());
        assert!(started.remaining_secs.is_none(), "无限时长没有剩余时间");
        assert!(!started.allow_display_sleep);

        let current = status();
        assert!(current.active);

        let stopped = stop().expect("stop");
        assert!(!stopped.active);
        assert!(stopped.started_at_ms.is_none());
        assert!(!status().active);
    }

    #[test]
    fn zero_duration_means_infinite() {
        let _serial = serial();
        let started = start(Some(0), true).expect("start");
        assert!(started.active);
        assert!(started.allow_display_sleep);
        assert!(started.remaining_secs.is_none(), "0 秒按无限处理");
        stop().expect("stop");
    }

    #[test]
    fn expired_session_is_reclaimed_lazily() {
        let _serial = serial();
        let started = start(Some(1), false).expect("start");
        // 刚算出来时可能已经过了几百毫秒，截断成 1 或 0 秒都算正常
        assert!(matches!(started.remaining_secs, Some(0) | Some(1)));
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let current = status();
        assert!(!current.active, "到期后 status() 应惰性回收");
        stop().expect("stop");
    }

    #[test]
    fn restart_while_active_keeps_single_session() {
        let _serial = serial();
        start(None, false).expect("first start");
        let second = start(Some(60), true).expect("second start");
        assert!(second.active);
        assert!(second.allow_display_sleep);
        assert!(second.remaining_secs.is_some());
        stop().expect("stop");
    }

    #[test]
    fn hint_mentions_platform_limits() {
        let _serial = serial();
        let current = status();
        assert!(!current.platform_hint.is_empty());
    }
}
