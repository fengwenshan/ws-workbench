//! Tauri 命令按业务域拆分，统一在这里对外导出。
//!
//! `lib.rs` 的 `generate_handler!` 与 VPN 模块仍按 `crate::commands::xxx`
//! 访问，因此保持扁平重导出。

mod attachments;
mod cleaner;
mod cleaner_defaultapps;
mod cleaner_popup;
mod cleaner_sleep;
mod cleaner_startup;
mod config;
// `pub(crate)`：cleaner 模块要复用 openwith 里的图标提取（mac_icon / windows_icons）
pub(crate) mod openwith;
mod project;
mod quick;
mod system;

pub use attachments::*;
pub use cleaner::*;
pub use cleaner_defaultapps::*;
pub use cleaner_popup::*;
pub use cleaner_sleep::*;
pub use cleaner_startup::*;
pub use config::*;
pub use openwith::*;
pub use project::*;
pub use quick::*;
pub use system::*;
