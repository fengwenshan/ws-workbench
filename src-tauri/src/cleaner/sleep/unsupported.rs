//! 非 macOS / Windows 平台的占位实现。
//!
//! 本项目当前只支持 macOS / Windows；其它平台能编译但功能不保证。

/// 其它平台：没有实现，直接拒绝
pub(super) struct Guard;

pub(super) fn acquire(_allow_display_sleep: bool) -> Result<Guard, String> {
    Err("当前平台不支持保持唤醒".into())
}
