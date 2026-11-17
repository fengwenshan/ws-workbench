//! 本机 DNS 转发器的监听地址策略（平台差异集中在这里）。
//!
//! 域名分流要把「命中规则的域名」交给本机转发器，再由转发器问 VPN DNS。
//! 转发器监听在哪，两个平台能写的东西不一样：
//!
//! - **macOS**：`/etc/resolver` 支持 `port`，转发器直接绑 `127.0.0.1` 的随机
//!   高位端口，**完全不碰 53**（之前「先试 53、失败再换端口」的写法只要我们先
//!   起来就把 53 从 Clash/dnsmasq 手里拿走了）。
//! - **Windows**：不用本机转发器，也不占 53 —— 原因写在 `windows.rs` 里
//!   （NRPT 指到回环地址时 DNS Client 不会把查询送出来，实机两次证伪）。
//!
//! `mod.rs` 只做平台分派，规则本身在各平台文件里。

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// 转发器实际绑定的地址（`host:port`，端口为 0 表示随机高位端口）。
#[cfg(target_os = "macos")]
pub use macos::BIND_ADDR;
/// 转发器监听的主机部分（写日志用）。
#[cfg(target_os = "macos")]
pub use macos::HOST;
