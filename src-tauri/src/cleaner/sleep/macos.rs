//! macOS 的保持唤醒实现：IOKit `IOPMAssertionCreateWithName` 电源断言。
//!
//! 系统断言 `PreventUserIdleSystemSleep` 常建；「允许显示器睡眠」关闭时再加
//! `PreventUserIdleDisplaySleep`。断言随进程退出自动回收，不需要 root/TCC。
//!
//! 平台验证状态：本文件只在 macOS 上编译，macOS CI（test-macos）验证。

use std::ffi::{c_void, CString};
use std::os::raw::c_char;

type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type IOReturn = i32;

/// IOPMAssertionLevel = 255（kIOPMAssertionLevelOn）
const LEVEL_ON: u32 = 255;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
/// 断言名：`pmset -g assertions` 里能看到，便于人工核对
const ASSERTION_NAME: &str = "ws-workbench Keep Awake";

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(
        allocator: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFStringRef,
        level: u32,
        assertion_name: CFStringRef,
        assertion_id: *mut u32,
    ) -> IOReturn;
    fn IOPMAssertionRelease(assertion_id: u32) -> IOReturn;
}

fn cfstring(value: &str) -> Option<CFStringRef> {
    let c_str = CString::new(value).ok()?;
    let handle =
        unsafe { CFStringCreateWithCString(std::ptr::null(), c_str.as_ptr(), CF_STRING_ENCODING_UTF8) };
    (!handle.is_null()).then_some(handle)
}

fn create_assertion(assertion_type: &str) -> Option<u32> {
    let type_ref = cfstring(assertion_type)?;
    let name_ref = cfstring(ASSERTION_NAME)?;
    let mut id: u32 = 0;
    let result = unsafe { IOPMAssertionCreateWithName(type_ref, LEVEL_ON, name_ref, &mut id) };
    unsafe {
        CFRelease(type_ref);
        CFRelease(name_ref);
    }
    (result == 0).then_some(id)
}

pub(super) struct Guard {
    system: u32,
    display: Option<u32>,
}

pub(super) fn acquire(allow_display_sleep: bool) -> Result<Guard, String> {
    let system = create_assertion("PreventUserIdleSystemSleep")
        .ok_or_else(|| "创建系统电源断言失败（IOPMAssertionCreateWithName）".to_string())?;
    if allow_display_sleep {
        return Ok(Guard {
            system,
            display: None,
        });
    }
    match create_assertion("PreventUserIdleDisplaySleep") {
        Some(display) => Ok(Guard {
            system,
            display: Some(display),
        }),
        None => {
            unsafe { IOPMAssertionRelease(system) };
            Err("创建屏幕电源断言失败（IOPMAssertionCreateWithName）".to_string())
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            if let Some(display) = self.display.take() {
                IOPMAssertionRelease(display);
            }
            IOPMAssertionRelease(self.system);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{serial, start, stop};

    /// 手动验证：真实电源断言必须出现在 `pmset -g assertions` 里。
    /// 跑法：`cargo test --lib cleaner::sleep::macos::tests::manual_assertion_shows_in_pmset -- --ignored --nocapture`
    #[test]
    #[ignore = "需要真实系统电源断言，默认不跑"]
    fn manual_assertion_shows_in_pmset() {
        let _serial = serial();
        let started = start(None, false).expect("start");
        assert!(started.active);

        let output = std::process::Command::new("pmset")
            .args(["-g", "assertions"])
            .output()
            .expect("跑 pmset 失败");
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        println!("{text}");

        assert!(
            text.contains("ws-workbench Keep Awake"),
            "pmset 里应能看到断言名，实际输出：\n{text}"
        );
        assert!(text.contains("PreventUserIdleSystemSleep"));
        assert!(text.contains("PreventUserIdleDisplaySleep"));
        assert!(text.contains("PreventUserIdle"), "断言类型名应完整展示");
        stop().expect("stop");
    }
}
