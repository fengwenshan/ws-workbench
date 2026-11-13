//! macOS 的自动规则探测：`pmset -g batt` 电源 + CoreGraphics 活动屏幕数。
//!
//! 都是只读查询，不需要管理员权限。
//!
//! 平台验证状态：本文件只在 macOS 上编译，macOS CI（test-macos）验证。

pub(super) fn probe_on_ac_power() -> bool {
    std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("AC Power"))
        .unwrap_or(false)
}

pub(super) fn probe_external_display() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max_displays: u32, displays: *mut u32, count: *mut u32) -> i32;
    }
    let mut count: u32 = 0;
    // 只问数量：不传数组，避免多一次分配
    let status = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
    status == 0 && count > 1
}
