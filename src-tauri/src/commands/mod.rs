//! Tauri 命令按业务域拆分，统一在这里对外导出。
//!
//! `lib.rs` 的 `generate_handler!` 与 VPN 模块仍按 `crate::commands::xxx`
//! 访问，因此保持扁平重导出。

mod attachments;
mod config;
mod project;
mod system;

pub use attachments::*;
pub use config::*;
pub use project::*;
pub use system::*;
