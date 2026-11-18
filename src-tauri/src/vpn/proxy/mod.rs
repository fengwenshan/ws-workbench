//! 系统代理检测：判断「按域名分流」会不会被本机的 HTTP 代理绕开。
//!
//! 现场踩过：浏览器走 Clash 的 `127.0.0.1:7897`，Clash 用它**自己的** DNS
//! （`8.8.8.8` / DoH）解析 `oa.example.com`，拿到公网记录；我们写进系统的
//! NRPT / `/etc/resolver` 只作用于系统解析器，管不到代理内部。表现就是
//! 「按 IP 能开、按域名打不开」。
//!
//! 这里只做检测和提示，**不改用户的代理设置**。平台差异（注册表 / `scutil`）
//! 在 `windows.rs` / `macos.rs`，匹配与文案在 `common.rs`（可单测）。

#[cfg(any(target_os = "macos", test))]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

pub mod common;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;
#[cfg(target_os = "windows")]
use windows as platform;

pub use common::{bypass_covers, uncovered_domains, warning_for, ProxyState};

/// 读当前系统的代理设置（失败时返回「没开代理」，不阻塞连接）。
pub fn detect() -> ProxyState {
    platform::detect()
}
