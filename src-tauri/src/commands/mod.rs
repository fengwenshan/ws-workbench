//! Tauri 命令按业务域拆分，统一在这里对外导出。
//!
//! `lib.rs` 的 `generate_handler!` 与 VPN 模块仍按 `crate::commands::xxx`
//! 访问，因此保持扁平重导出。

pub(crate) mod attachments;
mod cleaner;
mod cleaner_popup;
mod cleaner_sleep;
// 剪贴板历史（macOS）
mod clipboard;
mod config;
// 「用 DeepSeek 打开」：工作区请求文件 + DSH 插件安装
mod deepseek;
// Hosts 在线编辑
mod hosts;
// 局域网分享（发现 + HTTP 收发）
mod lanshare;
// 「设置 → 资源」：DSH 插件等资源的检查/下载/安装/卸载
mod resources;
// `pub(crate)`：cleaner 模块要复用 openwith 里的图标提取（mac_icon / windows_icons）
pub(crate) mod openwith;
mod project;
mod quick;
mod system;

pub use attachments::*;
pub use cleaner::*;
pub use cleaner_popup::*;
pub use cleaner_sleep::*;
pub use clipboard::*;
pub use config::*;
pub use deepseek::*;
pub use hosts::*;
pub use lanshare::*;
pub use openwith::*;
pub use project::*;
pub use resources::*;
pub use quick::*;
pub use system::*;
