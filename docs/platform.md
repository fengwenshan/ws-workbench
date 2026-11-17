# 平台与限制

> 来源：原 [README](../README.md) 的「平台与限制」章节；平台代码规则见 [AGENTS.md](../AGENTS.md) 第 7 节。

| 项 | Windows | macOS |
| --- | --- | --- |
| VPN 提权 | 计划任务免 UAC，失败回退 UAC 弹窗 | 系统授权框 |
| 虚拟网卡 | Wintun 驱动 | 系统 utun |
| 分流实现 | 路由注入：NRPT 按域名解析 + 隧道路由（不抓包） | 路由注入：`/etc/resolver` + 本机转发器 + 隧道路由（不抓包） |
| 按域名解析 | NRPT（`Add-DnsClientNrptRule`） | `/etc/resolver` + 本机转发器（53 优先，被占用则随机高位端口 + `port` 字段） |
| 路由查询 | PowerShell JSON（`Get-NetRoute` 等） | `route` / `ifconfig` / `netstat` |

其余已知限制：

- 打包只支持 Layui 项目
- macOS 最低系统版本 11.0：内置 openconnect 是 arm64 + x86_64 的静态产物（`minos 11.0`），`tauri.macos.conf.json` 把 `LSMinimumSystemVersion` 固定为 11.0；10.13–10.15 的旧机器不再支持
- 相对路径含空格的文件会被跳过
- 分流只处理 IPv4：域名解析可能返回 AAAA，IPv6 目标不进隧道、仍走系统默认路由
- 域名分流只覆盖系统解析器（getaddrinfo）：自带 DoH/DoT 的应用不受控
- Windows 上按域名解析出来的 `/32` 由本机转发器在回包前注入；NRPT 退回旧路径时最多晚约 3~15 秒生效。服务端下发的内网范围（默认域名 / split-include）只展示、不参与分流，所以不会再有「服务端一推就接管」的隐式路由
- 规则与代理模式的改动都会热生效（worker 轮询 `rules.json` / `mode` 后重新布线），不需要断开重连
- 系统清理的 Windows 运行时行为已补实机冒烟：注册表三视图枚举、MSI/exe 卸载退出码映射、残留扫描与回收站删除、硬拒名单与 HKCU 键删除都有对应用例（`src-tauri/src/cleaner/windows_smoke.rs`，与 `macos_smoke.rs` 对称），2026-11-13 在 Windows 11 全绿；用法见下面「实机冒烟」。仍需人工验收的只剩需要点 UAC 的路径（提权批量删除、HKLM 键删除）与真实 MSI 应用卸载，按项目规则保留 `TODO(platform-verify:windows)` 并写明了验证步骤；安装包的 NSIS 卸载钩子已实现并通过 `makensis` 编译检查（含新的 PowerShell 三类勾选界面），进程未退出则中止、勾选删除/保留数据、自定义附件目录提示仍需在 Windows 实机走一遍（计划见 `docs/superpowers/plans/2026-11-10-installer-uninstall.md` 的 Phase W）
- `~/.Trash` 内容受 TCC 保护，删除结果以「原路径消失 + trash API 成功」为准
- 卸载残留只做精确匹配（bundle id / 注册表键 / 精确应用名），存在非标准目录的数据需要手动清理
- 托盘快捷面板的原生弹出（真实点菜单栏图标）受 macOS 辅助功能/屏幕录制权限限制，自动化测试只覆盖了页面渲染、位置计算与 `cargo check`，首次使用请手动点一下确认弹窗位置与失焦收起
- 快捷面板的透明圆角窗口在 macOS 上依赖 `macos-private-api`，因此不支持 Mac App Store 渠道
- 剪贴板历史在 macOS / Windows 启用（Linux 不启用）；历史加密落 SQLite、图片附件单独密文存放，**不联网**；默认快捷键在应用运行期间全局生效，会占用其它 app 的同款组合，可在「系统 → 剪贴板」齿轮里改。富文本回贴是尽力而为：依赖 source app 是否写入 HTML、目标 app 是否接受 HTML，图片过多或过大时可能降级成纯文本
- 局域网分享只做 IPv4、只在本应用之间互通（自定义协议，不与官方 LocalSend 兼容），传输明文、无配对密码；发现端口固定 UDP 53317 并开了端口复用（`SO_REUSEADDR` + `SO_REUSEPORT`），同机同时跑开发版和安装版可以互相发现（HTTP 端口各自从 53317 起顺延）
- Hosts 在线编辑需要管理员授权；Windows 的提权写入 / UAC 取消 / DNS flush 只做了代码审查，需实机验收；非 UTF-8 的 hosts 文件会被拒绝编辑；没有语法校验，写错条目需自己负责（可随时从备份还原）
- 更新包的断点续传只在「同版本 + 同下载 URL」内有效；服务端不支持 Range 时会退化为整包重下，系统清掉应用缓存目录后重试也会从 0 开始。Windows 上的「下载 → 退出 → 安装 → 重启」全链路只做了代码审查，需实机验收
