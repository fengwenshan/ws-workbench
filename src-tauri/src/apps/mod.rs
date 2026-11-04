//! 外部应用定位与启动
//!
//! 全项目只有这一处知道「去哪里找某个应用」。原来这份知识分了两份：
//! - `commands.rs` 的 win_launcher：VS Code / IDEA / Trae
//! - `vpn/detect.rs`：openconnect
//!
//! 现在统一到 `models` 的元数据表 + `finder` 的查找流程，
//! Windows 的注册表线索在 `registry`。

pub mod finder;
pub mod models;
pub mod registry;

pub use finder::{locate, locate_all, managed_dir};
pub use models::{AppCandidate, AppContext, AppId, AppLocation, AppPathOverride};
