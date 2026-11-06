//! Tauri 命令按业务域拆分，统一在这里对外导出。
//!
//! `lib.rs` 的 `generate_handler!` 与 VPN 模块仍按 `crate::commands::xxx`
//! 访问，因此保持扁平重导出。

mod attachments;
mod cleaner;
mod config;
// `pub(crate)`：cleaner 模块要复用 openwith 里的图标提取（mac_icon / windows_icons）
pub(crate) mod openwith;
mod project;
mod system;

pub use attachments::*;
pub use cleaner::*;
pub use config::*;
pub use openwith::*;
pub use project::*;
pub use system::*;
