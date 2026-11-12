//! VPN 连接器平台层
//!
//! 这里是连接器「检测 / 导入 / 候选来源」的唯一入口，也是整个 vpn 模块里唯一做
//! 平台分派的地方：检测、导入、候选来源由 `macos.rs` / `windows.rs` 各自实现，
//! 二进制里只会编进当前平台那一份；另一端平台的代码不会被编译，改一边不会影响另一边。
//!
//! - `common.rs`：共享层，只留 probe（真跑 `--version`）、版本号解析、来源展示名、
//!   连接器挑选与 [`ImportedOpenconnect`] 模型，不写任何平台分支；
//! - `macos.rs`：向外找系统安装的 openconnect、只记路径的导入、旧副本/软链清理；
//! - `windows.rs`：只认安装包内置副本、拒绝导入外部副本。
//!
//! 调用方（`vpn::commands`）只依赖本模块导出的统一 API，不感知平台。

mod common;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{detect_all, import_openconnect, remove_managed_openconnect};
#[cfg(target_os = "windows")]
pub use windows::{detect_all, import_openconnect, remove_managed_openconnect};

pub use common::{fetch_version, pick_connector, ImportedOpenconnect};

// 产品只支持 macOS / Windows（README「平台与限制」）。其它目标上不要悄悄编出一份
// 「半套实现」：直接给出明确信息，避免像旧 detect.rs 那样在 cfg 组合里缺函数。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!("VPN 连接器平台层目前只实现 macOS 与 Windows");
