//! Windows 侧安装列表、卸载执行与残留规则。
//!
//! 数据源是注册表三处 Uninstall 键（设计 4.1）：HKLM 64 位视图、HKLM 的
//! Wow6432Node（32 位视图）、HKCU。用 `KEY_WOW64_32KEY/64KEY` 显式指定视图，
//! 否则 32 位进程看到的是重定向后的视图，会漏掉一半条目。
//!
//! 本模块分两层，按职责拆文件：
//! - `parse.rs`：**与 winreg 无关的纯逻辑**（过滤 / 去重 / 大小换算 /
//!   UninstallString 解析 / 自身安装目录判定），不依赖 Windows，
//!   `cargo test` 在 macOS 上也能跑；
//! - `registry.rs` / `uninstall.rs` / `leftovers.rs` / `related.rs`：
//!   真正读注册表、起卸载程序、扫残留、组装关联文件的实现，全部锁在
//!   `#[cfg(target_os = "windows")]` 下（见下方模块声明）。
//!
//! 平台实现共用的小 helper（`read_string` / `open_entry_key` / `parse_app_id` /
//! `display_icon_path` / `is_forbidden_install_dir`）只有 `registry.rs` 一份，
//! 以 `pub(super)` 暴露给同目录的其它实现文件，不复制第二份。
//!
//! 对外符号沿用拆分前的路径（`crate::cleaner::installer_windows::…`）：`list_apps` /
//! `uninstall` / `scan_leftovers` / `related_files` / `install_root_candidates` 与
//! `is_self_install` 在这里按原可见性再导出，调用方（`installer` / `running` /
//! `related`）无需改动；`parse` 的纯逻辑由同目录文件用 `super::parse::…` 直接引用，
//! 不经本模块转手（转出去只会是多一层没人用的 re-export）。

mod parse;

#[cfg(target_os = "windows")]
mod leftovers;
#[cfg(target_os = "windows")]
mod registry;
#[cfg(target_os = "windows")]
mod related;
#[cfg(target_os = "windows")]
mod uninstall;

// `is_self_install` 原本就是 `pub(crate)`（`installer.rs` 的 Windows 分支在用）：
// macOS 的测试构建里没有调用方，单独放行未使用导入，理由与 `cleaner/mod.rs` 给
// 本模块的 `allow(dead_code)` 相同。
#[cfg_attr(not(target_os = "windows"), allow(unused_imports))]
pub(crate) use parse::is_self_install;

#[cfg(target_os = "windows")]
pub use leftovers::scan_leftovers;
#[cfg(target_os = "windows")]
pub use registry::{install_root_candidates, list_apps};
#[cfg(target_os = "windows")]
pub use related::related_files;
#[cfg(target_os = "windows")]
pub use uninstall::uninstall;
