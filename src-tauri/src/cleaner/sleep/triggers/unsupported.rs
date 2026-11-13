//! 非 macOS / Windows 平台的占位实现：电源与外接显示器探测一律返回 false。
//!
//! 本项目当前只支持 macOS / Windows；其它平台能编译但功能不保证。

pub(super) fn probe_on_ac_power() -> bool {
    false
}

pub(super) fn probe_external_display() -> bool {
    false
}
