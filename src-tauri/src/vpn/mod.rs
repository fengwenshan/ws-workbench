//! VPN 模块
//!
//! 分两层看：
//!
//! **主进程（普通权限）** —— `commands.rs`
//! 负责配置读写、连接器探测、把连接请求写进会话目录、拉起提权 worker、
//! 轮询状态文件并推给界面。
//!
//! **worker（管理员权限）** —— `worker.rs`
//! 独立进程，真正做「建隧道 -> 按分流模式下发路由 -> 守护 -> 回滚」。
//!
//! 拆成两个进程的原因：openconnect 建虚拟网卡、改路由表都需要管理员权限，
//! 但让整个 GUI 常驻管理员权限既危险又每次启动都弹窗。
//!
//! 模块分工：
//! - `models`      数据结构
//! - `credential`  密码加密存储（Windows DPAPI / macOS 钥匙串）
//! - `detect`      连接器探测（内置 -> 系统 -> PATH）
//! - `route`       路由表读写（Windows 走 PowerShell JSON，macOS 走 route 命令）
//! - `script`      生成自定义 vpnc-script，抢占「默认路由是否走 VPN」这个开关
//! - `connector`   真正拉起隧道（openconnect）
//! - `session`     会话目录 IPC（提权进程拿不到父进程管道，只能靠文件）
//! - `learn`       学习模式：采样实际连通的远端 IP，反推候选分流网段
//! - `elevate`     提权启动（Windows 计划任务免 UAC / macOS 系统授权框）
//! - `worker`      提权进程入口，连接生命周期主体
//! - `guide`       openconnect 不可用时的安装引导

/// 应用数据目录名，与 `tauri.conf.json` 里的 identifier 保持一致。
/// 回退到临时目录时会用它建一个同级子目录，避免把会话文件散在临时目录根下。
pub const APP_DIR_NAME: &str = "com.ws.tools";

pub mod commands;
pub mod connector;
pub mod credential;
pub mod detect;
pub mod elevate;
pub mod guide;
pub mod learn;
pub mod models;
pub mod route;
pub mod script;
pub mod session;
pub mod worker;
