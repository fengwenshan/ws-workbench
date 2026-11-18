//! macOS：转发器绑 `127.0.0.1` 的**随机高位端口**，不再碰 53。
//!
//! `/etc/resolver/<域名>` 支持 `port` 关键字，端口绑完再写进 resolver 文件即可。
//! 这样 Clash / dnsmasq / AdGuard 继续用 53，我们不抢；现场踩过
//! `绑定 127.0.0.1:53 失败：Address already in use`，本质就是抢端口。
//!
//! 已经由单测锁住的部分（`vpn::split` 的 `resolver_file_points_at_local_forwarder_port`）：
//! 文件内容就是 `nameserver 127.0.0.1` + `port <实际端口>` 两行，旧格式（只有 nameserver）
//! 仍被识别为我们写的，`nameserver` 不是回环地址的文件一律不碰。
//!
//! TODO(platform-verify:macos): 真机确认系统真的按这个文件把该域名的查询送到随机端口；验证步骤：连上 VPN 后 `cat /etc/resolver/risencn.com`，应含 `nameserver 127.0.0.1` 与 `port <高位端口>`，且与日志 `[分流] 转发器 127.0.0.1:<端口>` 一致；`sudo lsof -iUDP:53` 不应出现 ws-workbench；`dig +short oa.risencn.com` 应得到内网地址；期望：域名解析出内网地址、53 未被占用、断开后 `/etc/resolver` 里我们写的文件被清空。

/// 转发器绑定地址：端口 0 = 由系统分配一个空闲高位端口。
pub const BIND_ADDR: &str = "127.0.0.1:0";
/// 转发器监听的主机部分（日志与 resolver 文件用）。
pub const HOST: &str = "127.0.0.1";
