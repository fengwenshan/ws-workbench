//! Windows 的自动规则探测：`GetSystemPowerStatus` + `GetSystemMetrics(SM_CMONITORS)`。
//!
//! 都是只读查询，不需要管理员权限。
//!
//! 平台验证状态：本文件只在 Windows 上编译，Windows CI（test-windows）验证。

pub(super) fn probe_on_ac_power() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.is_ok() && status.ACLineStatus == 1
}

pub(super) fn probe_external_display() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
    unsafe { GetSystemMetrics(SM_CMONITORS) > 1 }
}
