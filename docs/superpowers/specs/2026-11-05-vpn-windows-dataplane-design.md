# VPN Windows 数据面：规则 / 全局两种模式的路由注入实现

- 日期：2026-11-05
- 状态：已实现（代码完成；NRPT 与 on-link 路由需在管理员会话里做真机验收，见第 9 节）
- 范围：`src-tauri/src/vpn/split.rs`、`win_dns.rs`、`fulltunnel.rs`、`route.rs`、`worker.rs`、`script.rs`
- 前置：`2026-11-05-vpn-proxy-mode-hot-switch-design.md`（模式与规则的热切换）、commit `77af4d8`（macOS 侧改为路由注入，删除捕获引擎）

## 1. 背景

`77af4d8` 把 macOS 侧的数据面从「捕获 TUN + 用户态协议栈 + fake-ip」换成了**路由注入**：

- 规则模式：`/etc/resolver/<域名>` + 本机 DNS 转发器 + 解析结果 `/32` 指向隧道；默认路由与系统 DNS 都不动。
- 全局模式：`0.0.0.0/1` + `128.0.0.0/1` 指向隧道 + 系统 DNS 改成 VPN DNS，带验证与回滚。
- 直连模式：只建隧道。

捕获引擎删除后，Windows 侧没有再实现数据面：`worker.rs` 里两个分支直接把 `degraded_reason` 设成
「路由注入式分流目前只支持 macOS」/「全隧道接管目前只实现了 macOS」，连接能建立，但一个字节的流量都不接管。
本设计把同样的数据面在 Windows 上补齐，机制换、语义与安全性质不变。

## 2. 目标

1. 规则模式在 Windows 上生效：命中「走 VPN」的域名由 VPN DNS 解析，解析出的内网 IP 走隧道；`IP-CIDR` 与服务端 split-include 网段整段走隧道。
2. 全局模式在 Windows 上生效：整机流量走隧道，装完立刻验证，不通就整段回滚。
3. 规则模式**不动默认路由、不动物理网卡 DNS**，结构上不可能「一连上就全网断」。
4. 断开、异常退出、崩溃后重启都能把改过的东西收干净（NRPT 规则、隧道路由、半默认路由）。
5. 规则 / 模式仍然运行期热切换（`SplitTunnel::teardown` + 按新模式重新布线）。
6. 服务端下发的默认域名与 split-include 网段在 Windows 上也自动并入（之前 Windows 脚本根本没回报这些变量）。

## 3. 非目标

- 不做按连接、按进程、按端口的判定（那是被删掉的捕获引擎的能力，规则类型已收窄到 `DOMAIN` / `DOMAIN-SUFFIX` / `IP-CIDR`）。
- 不改 macOS 侧行为；两个平台只共享 `plan`、`dns_client`、`route` 的纯逻辑部分。
- 不支持 IPv6 分流（IPv6 目标仍走系统默认路由）。
- 不做 Windows 服务常驻；仍是「提权 worker 进程 + 文件 IPC」的模型。

## 4. 关键事实与约束

### 4.1 Windows 没有 `/etc/resolver` 的等价文件

按命名空间指定 DNS 的官方机制是 **NRPT（Name Resolution Policy Table）**：

```
Add-DnsClientNrptRule -Namespace 'risencn.com' -NameServers '172.18.160.12' `
  -DisplayName 'ws-workbench:risencn.com' -Comment 'ws-workbench'
```

- 只影响命中命名空间的查询，不改系统 DNS、不改默认路由。
- 命名空间按 DNS 后缀匹配，规则 `example.com` 覆盖它自己和所有子域，和 `/etc/resolver/example.com` 等价。
- 但**路由注入只解析规则里的字面域名**：`DOMAIN-SUFFIX example.com` 不会自动解析 `oa.example.com`。
  如果 `example.com` 本身没有 A 记录，子域即使 DNS 能解析，也不会有 `/32` 路由。
  子域要么逐条写 `DOMAIN oa.example.com`，要么使用全局模式。
- 由 `Dnscache`（DNS Client）服务执行，服务没跑规则不生效。

### 4.2 NRPT 的应答在 DNS Client 里，用户态看不到

macOS 的转发器是「应用 → 127.0.0.1:53 → 转发器 → VPN DNS」同一条链路，所以能在**把应答回给应用之前**注入 `/32`，没有时间差。

Windows 的 NRPT 由 DNS Client 直接向 VPN DNS 发查询并缓存应答，用户态没有「应答即将返回」的钩子。
两条路都试过、都不可靠：

- 把 NRPT 指向本机转发器（`127.0.0.1`）：DNS Client 对 loopback DNS 的行为没有文档保证，实测资料互相矛盾；
- 拦 UDP/53：需要 WFP 或 WinDivert，等于把刚删掉的捕获引擎再引回来一半。

**结论**：按域名解析出的地址改为**主动解析线程**注入 —— 每 15 秒按规则里的域名直接问 VPN DNS，把 A 记录逐个装 `/32`。
代价是新地址最多晚一个周期（15 秒）生效；服务端下发的 split-include 网段不受影响，一开始就整段注入。

### 4.3 隧道侧路由一律 on-link

Wintun 是虚拟网卡，没有可用的下一跳。`New-NetRoute -NextHop 0.0.0.0` 在文档里的含义就是「目标在本地子网」（on-link），
正好是「从这块网卡出去」：

```
New-NetRoute -DestinationPrefix '192.168.136.0/22' -InterfaceIndex 42 -NextHop '0.0.0.0' `
  -RouteMetric 1 -PolicyStore ActiveStore
```

- `-PolicyStore ActiveStore`：只写内存路由表，重启自然消失，不污染持久化配置。
- 先 `Get-NetRoute -DestinationPrefix ... -InterfaceIndex ...` 判存在再建，整批路由只起一次 PowerShell，重复调用幂等。
- 不用 `route.exe`：中文 Windows 的报错是本地化的（「对象已存在。」），按文本容错必然在别的语言版本上失效。

### 4.4 DNS 查询走哪块网卡

Windows 的 DNS Client 按**路由**选出口（不是按网卡绑定）。VPN DNS 的 `/32` 已经指向隧道，查询自然进隧道，
所以**不把查询钉在隧道网卡上也能走对路**；macOS 需要 `OverridePrimary` + 改活动网络服务，是因为 mDNSResponder 会把 socket 钉在物理网卡上，Windows 没有这个问题。
这是两个平台在全局模式 DNS 上唯一的语义差别。

### 4.5 崩溃后必须能收拾

worker 断开走 SIGKILL，脚本的 `disconnect` 分支不会执行。可能残留：

| 残留物 | 谁清理 |
| --- | --- |
| NRPT 规则（`ws-workbench:` 前缀） | 下次 worker 启动时 `split::cleanup_stale` 全部删除；`teardown` 也删 |
| 规则模式的静态 `/32` / 网段路由 | 挂在 Wintun 网卡上，网卡随隧道销毁而消失；`teardown` 主动删 |
| 全局模式的 `/1` 半默认路由 | `fulltunnel::restore_stale` 只删「前缀是 `/1` 且网卡驱动描述像 VPN」的条目 |
| 隧道网卡的 DNS | 网卡随隧道销毁；`teardown` 主动复位成自动获取 |

半默认路由的清理**必须同时用「前缀 + 网卡身份」两个条件**：只按前缀删会误伤 WireGuard / OpenVPN（它们也用 `/1`），
只按网卡删会把别人正常的分流路由删掉。网卡身份用 `InterfaceDescription + Name` 匹配
`(?i)wintun|openconnect|tap-windows|tap adapter|anyconnect|vpn`。

### 4.6 vpnc-script 只负责网卡和回报

捕获引擎删除后，脚本不再碰默认路由和 DNS。Windows 脚本（cscript/JScript，必须纯 ASCII）现在做三件事：

1. 配接口地址与 MTU（`netsh ... store=active`）；
2. 给 VPN 服务器本身加防环主机路由（走物理网关）；
3. 把 `TUNIDX`、`INTERNAL_IP4_*`、`CISCO_DEF_DOMAIN`、`CISCO_SPLIT_INC_*` 回报给 worker。

第 3 条里的 `CISCO_*` 是这次补上的：之前 Windows 脚本不回报，服务端下发的「公司内网范围」在 Windows 上完全丢失。

## 5. 机制对照

| 能力 | macOS | Windows |
| --- | --- | --- |
| 按域名指定 DNS | `/etc/resolver/<域名>` | NRPT（`Add-DnsClientNrptRule`） |
| 域名 → 内网 IP 的注入时机 | 转发器在回包前注入 | 后台线程每 15 秒主动查询后注入 |
| 隧道路由 | `route -n add -interface utunN` | `New-NetRoute -NextHop 0.0.0.0`（on-link） |
| 删路由 | `route -n delete` | `Remove-NetRoute` |
| 全局模式 DNS | `scutil` + `networksetup`（两处） | 只改隧道网卡（`Set-DnsClientServerAddress`） |
| 刷新缓存 | `dscacheutil -flushcache` + HUP mDNSResponder | `Clear-DnsClientCache` |
| 路由 / 网卡查询 | `route` / `ifconfig` / `netstat` 文本 | PowerShell + JSON（`Get-NetRoute` / `Get-NetAdapter` / `Get-DnsClientNrptRule`） |
| 崩溃清理判据 | 清单文件 + 内容校验 | `DisplayName` 前缀 `ws-workbench:` |

## 6. 规则模式（`split.rs`）

### 6.1 apply

```
cleanup_stale(session)              // 先清崩溃残留（只清自己的）
install_routes(plan.cidrs)          // VPN DNS /32 + IP-CIDR + split-include 网段 -> 隧道
  └ 失败：teardown 已做的部分，返回错误
install_resolver()                  // 有域名规则才做
  ├ NRPT 成功：nrpt_installed = true
  └ NRPT 失败：退回「只给隧道网卡配 DNS」+ 记日志；两者都失败才报错
start_resolver()                    // 主动解析线程
flush_dns_cache()
```

- 只有域名规则才需要 DNS；纯 `IP-CIDR` 的分流不碰解析器。
- 有域名规则但服务端没下发 DNS → 直接报错，不进入降级（否则域名永远解析不了，用户会以为规则没生效）。
- `teardown` 幂等：断开、切模式、apply 中途失败都走它。

### 6.2 主动解析线程

- 每个域名一个线程并发查（UDP 直接问 VPN DNS，1.5 秒超时），一轮最多等 5 秒。
- 只接受 `is_response_to(id)` 的应答，过滤掉伪造 / 串包。
- 每轮把「本轮所有可用地址」交给 `ensure_onlink_routes`（幂等）：路由被别的软件删掉也能在下一轮补回来。
- 只注入可路由的单播地址（过滤 `0.0.0.0`、回环、组播、链路本地、广播）。
- 失败日志只在错误文本变化时记一行，避免每 15 秒刷同一条把 `worker.log` 冲垮。
- 第一轮一个地址都没拿到时给一条可执行的提示（VPN DNS 不可达 / 域名不是完整内网域名）。

### 6.3 守护

每 15 秒：

- `ensure_onlink_routes(static_routes)` 一条 PowerShell 补全缺的路由；
- `installed_domains()` 比对 NRPT 规则，缺了就**重下全部域名**（`apply_namespace_rules` 的脚本会先清掉自己所有规则再重建，只传缺的那几个会把还在的规则一起删掉）；
- 退回到「隧道网卡 DNS」时检查网卡 DNS 有没有被改掉。

### 6.4 已知取舍

- 按域名解析出的 `/32` 最晚约 15 秒生效；这段时间里应用可能已经拿着新 IP 发起连接，会走系统默认路由（大概率失败一次重试）。这是 NRPT 不给钩子的必然结果。
- 动态 `/32` 只增不减：DNS 轮换出的旧地址会留到 teardown。路由表条目量级是「域名数 × 轮换次数」，实际很小。

## 7. 全局模式（`fulltunnel.rs`）

1. 快照：macOS 记活动网络服务 + 原 DNS（并落标记文件）；Windows 只需要接口索引。
2. 路由：`0.0.0.0/1` + `128.0.0.0/1` on-link 指向隧道，metric 1。
3. DNS：Windows 只 `Set-DnsClientServerAddress -InterfaceIndex <隧道> -ServerAddresses <VPN DNS>`，物理网卡保持原样。
4. 验证：
   - 接管前先测一次系统解析器（基线）；
   - 基线正常 → 接管后最多验两次（每次 2.5 秒）；两次都失败 → **整段回滚**（路由 + DNS 一起还原）并报错；
   - 基线本来就不通 → 记录「验证结果不作数」，不因为网络本身不通而回滚。
5. 断开：删 `/1`、复位隧道网卡 DNS、刷缓存；macOS 额外还原 `scutil` 与 `networksetup` 两处。

为什么 Windows 不像 macOS 那样改物理网卡 DNS：Windows 的 DNS Client 按路由选出口，VPN DNS 的 `/32` 已经在隧道上；
改物理网卡只会在 VPN DNS 暂时不可达时把整机解析拖死，没有任何收益。

## 8. 与 worker 的接线

- `start_split_rules()` / `start_full_tunnel()` 去掉 `#[cfg(not(target_os = "macos"))]` 降级分支，两个平台走同一段代码。
- Windows 额外要求 `status.tun_index > 0`（接口索引来自脚本回报的 `TUNIDX`）；拿不到就明确报错，不再静默降级。
- 降级信息仍保留：规则/全局布线失败只让「本次不接管」，隧道与网络不受影响，`degraded_reason` 会显示在界面上。
- `start_engine()` 按 `applied_mode` 分派；非 macOS / Windows 平台保留「只建隧道」的降级。

## 9. 真机验收清单（需要管理员会话）

自动化测试只覆盖纯逻辑与脚本编译（`cargo test --lib`，含用 cscript 真跑一遍脚本回报）。下面这些是 Windows 专有行为，
必须在管理员会话里手工确认（当前开发环境是非管理员，`Get-NetAdapter` / `Get-DnsClientNrptRule` 一律「拒绝访问」）：

1. **NRPT 命名空间匹配**：连上后
   `Get-DnsClientNrptRule | Where-Object DisplayName -like 'ws-workbench:*' | Format-List Namespace,NameServers`，
   再 `Resolve-DnsName oa.<默认域名>` 应返回内网地址；`Resolve-DnsName www.baidu.com` 仍返回公网地址。
2. **NRPT 生效时机**：`Add-DnsClientNrptRule` 之后是否需要 `Restart-Service Dnscache` 才生效。若需要，在
   `win_dns::apply_namespace_rules` 里加一次重启并记日志（本设计假设不需要，cmdlet 会通知服务）。
3. **on-link 路由**：`Get-NetRoute -InterfaceIndex <隧道> | Format-Table DestinationPrefix,NextHop,RouteMetric`，
   确认 `/32` 与网段的 `NextHop` 是 `0.0.0.0`、metric 是 1；`tracert -d <内网 IP>` 第一跳应是隧道对端。
4. **全局模式 DNS**：切到全局后 `Get-DnsClientServerAddress -InterfaceIndex <隧道>` 应是 VPN DNS，物理网卡不变；
   浏览器能开公网页面、内网域名也能解析；断开后隧道网卡 DNS 复位。
5. **回滚**：全局模式接管后手工把隧道网卡 DNS 清空，观察守护是否补回；再手工删掉 `/1` 路由，观察守护是否重建。
6. **崩溃清理**：连着全局模式直接结束 worker 进程（任务管理器），确认 `/1` 路由仍指向隧道；再重新连接，
   日志里应出现「上次异常退出：已清掉 N 条残留在 VPN 网卡上的半默认路由」。
7. **中文系统**：确认没有解析任何 `netsh` / `route` 的本地化文本（实现里已全部改用 CIM JSON）。
8. **IPv6**：确认内网域名若返回 AAAA，应用连不上时能在日志 / 文档里找到「IPv6 不分流」的解释。
9. **服务端范围回报**：确认 `worker.log` 里有 `[分流] 服务端下发默认域名 ...` / `服务端下发 N 条分流网段`。
   没有的话先看 `script.log` 里有没有 `defdomain=` / `split=` 行：脚本侧已用 cscript 单测覆盖，
   缺的只可能是 openconnect 没把 `CISCO_*` 变量传进脚本环境（需要换 openconnect 版本或改用 `--script` 包装）。
10. **全局模式的物理网卡 DNS**：切到全局后物理网卡仍保留原 DNS，而发往它的查询会被 `/1` 路由带进隧道。
    验收时观察公网域名首次解析是否明显变慢（预期每个冷缓存域名多一次超时）；若不可接受，
    再评估「全局模式下临时把物理网卡 DNS 也换成 VPN DNS」的收益与还原风险。

## 10. 测试

- `route.rs`：on-link 脚本幂等且落点正确、崩溃清理只碰 VPN 网卡（纯函数单测）。
- `win_dns.rs`：NRPT 脚本只按自己的前缀清理、不写 GPO、不按命名空间猜；没有 DNS 时不写规则。
- `split.rs`：plan 归一化、不支持的规则给提示、动态 IP 过滤、`query_a_records` 解析真实应答、服务端范围并入。
- `script.rs`：Windows 脚本纯 ASCII、cscript 能编译；**新增**一条用 cscript 真跑 `reason=connect` 的测试，
  断言 `CISCO_DEF_DOMAIN` / `CISCO_SPLIT_INC_*` 被正确回报并写进 `script.log`。
- 全量：`cargo test --lib`（265 通过 / 3 条需要管理员权限的集成测试在当前环境失败，与本次改动无关）。
