//! macOS：转发器绑 `127.0.0.1` 的**随机高位端口**，不再碰 53。
//!
//! `/etc/resolver/<域名>` 支持 `port` 关键字，端口绑完再写进 resolver 文件即可。
//! 这样 Clash / dnsmasq / AdGuard 继续用 53，我们不抢；现场踩过
//! `绑定 127.0.0.1:53 失败：Address already in use`，本质就是抢端口。

/// 转发器绑定地址：端口 0 = 由系统分配一个空闲高位端口。
pub const BIND_ADDR: &str = "127.0.0.1:0";
/// 转发器监听的主机部分（日志与 resolver 文件用）。
pub const HOST: &str = "127.0.0.1";
