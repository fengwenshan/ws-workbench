# 内置 OpenConnect 运行时

这里是随应用一起打包的 openconnect 命令行运行时，也是 VPN 唯一的通道：
程序拼好参数后直接调用本目录的 `openconnect.exe`。

## 版本与来源

| 项 | 值 |
| --- | --- |
| 版本 | OpenConnect v9.21（GnuTLS 3.8.11，MinGW64 构建） |
| 来源 | 官方 GitLab CI 产物 `openconnect-installer-MinGW64-GnuTLS.exe` |
| 下载地址 | `https://gitlab.com/openconnect/openconnect/-/jobs/artifacts/v9.21/raw/openconnect-installer-MinGW64-GnuTLS.exe?job=MinGW64/GnuTLS` |
| SHA256 | `6ee9e8eb9bc59ef70bb0717df7f99703f8a2ccd11d8e45d58a61f9a2e6ef7d00` |
| 大小 | 3412708 字节 |

官方下载页（https://www.infradead.org/openconnect/download/ ）只放源码 tarball，
Windows 二进制只存在于 CI 产物里，所以上面那条 `gitlab.com` 的链接才是真正能拿到 exe 的地方。
拿到的是一个 NSIS 安装包，**不要直接双击安装**，用 7z 解开即可：

```powershell
7z x openconnect-installer-MinGW64-GnuTLS.exe -o<目标目录> -y
```

解出来的 `$PLUGINSDIR\`（NSIS 自己的插件）丢弃，其余文件按下面的清单放进来。

## 文件清单

可执行文件与动态库必须**放在同一层目录**，openconnect 靠同目录查找依赖：

- `openconnect.exe` — 主程序，`--version` 应报出 `v9.21`
- `libopenconnect-5.dll` — 核心库，Wintun / TAP 两套网卡支持都编译在里面
- `libgnutls-30.dll`、`libnettle-8.dll`、`libhogweed-6.dll`、`libgmp-10.dll`、`libtasn1-6.dll` — TLS 栈
- `libxml2-2.dll`、`zlib1.dll`、`liblz4.dll` — 协议解析与压缩
- `libintl-8.dll`、`iconv.dll` — 本地化
- `libgcc_s_seh-1.dll`、`libwinpthread-1.dll` — MinGW 运行库
- `libstoken-1.dll` — 软件令牌（RSA/TOTP/HOTP）
- `wintun.dll` — 虚拟网卡驱动（见下）
- `vpnc-script-win.js` — 官方默认配置脚本；实际连接时会被 `--script=` 覆盖成我们自己生成的那份
- `list-system-keys.exe` — 官方发行包附带的辅助程序，当前功能没用到，保留以求与官方包一致

## 关于网卡驱动

**不需要**用户单独安装 TAP-Windows。

openconnect 9.21 建虚拟网卡时的顺序是：先枚举现成的网卡 → 找不到就**用 `wintun.dll`
动态创建一个 Wintun 网卡** → 再失败才回退到 TAP-Windows。`wintun.dll` 就在本目录，
所以开箱即用。注意两点：

1. **Wintun 没有 `--wintun` 这个命令行选项**，它是自动启用的（9.21 对 Wintun 已不再标记为实验特性）。
2. 创建 Wintun 网卡需要**管理员权限**，被拒绝时报的是
   `Access denied creating Wintun adapter. Are you running with Administrator privileges?`
   本程序通过计划任务以管理员身份拉起 worker，正常情况下不会踩到。

Wintun 网卡在 `Get-NetAdapter` 里显示的驱动描述是 `Wintun Userspace Tunnel`，
名字则来自主机名。`vpn/route.rs` 判断「这块网卡是不是 VPN 网卡」时看的是**驱动描述**而不是
名字，正是为了认得出它（中文系统上 Cisco 的隧道网卡叫「以太网 2」，光看名字永远认不出来）。

## macOS

本目录只用于 Windows。macOS 上走系统里已有的 openconnect（Homebrew 等），
不打包二进制，见 `vpn/guide.rs` 里的安装引导。

## 升级步骤

1. 按上面的下载地址换成新版本的 tag，重新算 SHA256 并核对来源清单。
2. 按本文件更新下表与文件清单。
3. 跑 `cargo test`，其中 `vpn::detect` 里有用例会在本目录存在 `openconnect.exe` 时
   断言「自动」模式选中的是内置 openconnect。
