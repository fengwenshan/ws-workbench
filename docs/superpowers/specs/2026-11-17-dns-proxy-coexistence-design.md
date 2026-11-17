# DNS 分流与本地代理/其它 DNS 服务的共存问题（设计）

日期：2026-11-17
状态：已实现

## 背景

现场（Windows 11 + Clash Verge 系统代理）出现两个现象：

1. **按域名分流会被本机 HTTP 代理绕开**：浏览器把 `https://oa.risencn.com:9508/...`
   交给 `127.0.0.1:7897`（Clash），Clash 用**它自己的** DNS（`8.8.8.8` + DoH）解析，
   拿到公网记录 `60.191.64.3`，而内网 VPN DNS 给的是 `192.168.18.41`。
   结果：**按 IP 能开、按域名打不开**（绕过列表只按主机名字面匹配 `192.168.*`，
   域名不命中）。
   我们写的 NRPT 只作用于 Windows DNS 客户端，管不到 Clash 内置解析器。
2. **本机 53 端口被我们占了**：域名分流启动的转发器绑 `127.0.0.1:53`（Windows
   NRPT 只能写 IP、不能写端口，所以必须 53）。Clash 的 `dns.listen` 也是 `:53`，
   两边同时存在时谁收到查询不确定；macOS 侧虽然是「先试 53、失败再随机端口」，
   但只要我们先起来，同样是把 53 从 Clash/dnsmasq 手里拿走。

此外系统日志在连接瞬间记录过
`Name resolution policy table has been corrupted … read policy table for rule {…} failed with error 0x57`：
NRPT 表在「删旧规则 + 写新规则」的窗口里被 DNS 客户端读到，属于可减少但不能完全消除的瞬态。

## 目标

- 任何平台上都**不抢占** `127.0.0.1:53`：别的 DNS/代理服务继续用它。
- 域名分流依然工作：NRPT（Windows）与 `/etc/resolver`（macOS）继续把命中域名
  交给我们的转发器。
- 「系统代理会绕开按域名分流」这类问题**在界面上可见**，不再靠用户自己猜。
- 减少 NRPT 重写次数，并在写完后校验命名空间真的可读。

## 方案

### 1. 转发器与 53 端口（`vpn/local_dns/`）

| 平台 | 本机转发器 | 系统侧指向 |
| --- | --- | --- |
| macOS | `127.0.0.1:0`（随机高位端口，**不再尝试 53**） | `/etc/resolver/<域名>` 的 `nameserver` + `port` |
| Windows | **不用**（也不绑 53） | NRPT 直接指向 VPN DNS |

Windows 原设计是「NRPT 指向本机转发器 → 转发器按应答装 `/32` → 再回包」，这样
`DOMAIN-SUFFIX` 的任意子域都能在应用拿到地址之前完成布线。实机两次证伪：

1. 转发器绑 `127.0.0.2:53`：`Get-NetUDPEndpoint` 看得到监听，NRPT 也写进去了，
   但随机子域探针 2.5 秒内零查询；
2. 换回 `127.0.0.1:53`（当时空闲）再试：结果一样，零查询。

结论：Windows DNS Client 只把 NRPT 里的服务器当作「某个接口上的 DNS」来用，
回环地址没有接口，规则写完等于没用。继续绑 53 只剩副作用 —— 连接瞬间把端口从
Clash / dnsmasq / AdGuard 手里抢走 2.5 秒。于是 Windows 侧改为**不用转发器、不占
53**，直接走已经不存在的「回退」路径（现在它是正路）：NRPT 指向 VPN DNS，子域路由
靠扫系统 DNS 缓存 + 主动解析补齐。macOS 的 `/etc/resolver` 支持 `port`，随机高位
端口照旧。平台差异写在 `vpn/local_dns/` 里（`mod.rs` 只做分派，Windows 文件只记录
原因，不导出常量）。

### 2. 系统代理检测（`vpn/proxy/`）

- 采集：Windows 读 `HKCU\…\Internet Settings` 的 `ProxyEnable` / `ProxyServer` /
  `ProxyOverride`；macOS 解析 `scutil --proxy`。
- 判定：纯函数 `bypass_covers(entries, domain)`（Windows 的 `;` 分隔、`*.x.com`、
  `<local>`、`192.168.*`；macOS 的数组项同样处理）。
- 结论：当本次会话有「走 VPN 的域名规则」、系统代理开着、且这些域名**不在**
  绕过列表里时，生成一条警告：
  「系统代理 `127.0.0.1:7897` 会绕开按域名分流，请把 `*.risencn.com` 加进代理绕过」。
- 出口：写进 worker 日志（VPN 抽屉可见）+ `VpnStatus.warnings`，界面在代理页顶部
  用一条 Alert 展示（不改布局、不加常驻小字）。

### 3. NRPT 写入收敛

- 命名空间与 `NameServers` 都没变时**不重写**（避免每次连接/规则热更新都删一遍再写
  一遍）。
- 写完用 `installed_rules()` 校验：缺后缀命名空间、或服务器不是本次要写的那组，就
  重写一次；仍不行则降到「只给隧道网卡配 DNS」，并把实际规则打进日志。

## 不做

- 不改用户的代理配置（我们不写别人的设置）。
- 不试图劫持浏览器的代理流量；只把「为什么按域名打不开」说清楚。

## 验证要点

- Windows：连上 VPN 后 `Get-NetUDPEndpoint -LocalPort 53` 里**没有**我们的进程；
  日志有「不用本机转发器…也不占用 53」；`Get-DnsClientNrptRule` 的 `NameServers`
  是 VPN DNS；`Resolve-DnsName oa.risencn.com` 返回内网地址。
- 代理警告：本机开着 Clash 时日志与界面都应出现警告；把域名加进绕过列表后重启连接，
  警告消失。
- 既有单测（NRPT 脚本、规则编译、路由账本）保持绿。
