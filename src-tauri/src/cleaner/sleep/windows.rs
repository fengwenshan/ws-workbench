//! Windows 的保持唤醒实现：`SetThreadExecutionState` 由专用线程持有。
//!
//! `ES_CONTINUOUS | ES_SYSTEM_REQUIRED [| ES_DISPLAY_REQUIRED]`，
//! 停止时复位线程状态。不需要管理员；不拦屏保/合盖/主动睡眠。
//!
//! 平台验证状态：本文件只在 Windows 上编译，Windows CI（test-windows）验证。

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};

use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
    EXECUTION_STATE,
};

/// 持有 `ES_*` 状态的线程：`ES_CONTINUOUS` 要求线程活着并周期性重置
pub(super) struct Guard {
    tx: Option<Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

pub(super) fn acquire(allow_display_sleep: bool) -> Result<Guard, String> {
    let (tx, rx) = channel::<()>();
    let flags = if allow_display_sleep {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
    };
    let join = std::thread::Builder::new()
        .name("keepawake".into())
        .spawn(move || {
            let apply = |state: EXECUTION_STATE| unsafe {
                SetThreadExecutionState(state);
            };
            apply(flags);
            loop {
                match rx.recv_timeout(std::time::Duration::from_secs(25)) {
                    // 停止信号或调用方已释放：复位后退出
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => apply(flags),
                }
            }
            apply(ES_CONTINUOUS);
        })
        .map_err(|err| format!("启动保持唤醒线程失败：{err}"))?;
    Ok(Guard {
        tx: Some(tx),
        join: Some(join),
    })
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
