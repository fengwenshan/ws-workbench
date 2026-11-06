# Windows VPN 方案可行性调研（证据链版）

- 日期：2026-11-06
- 范围：只审 Windows 实现；macOS 代码未做任何修改
- 结论先行：**协议层已经证明能通；数据面还没有证明能通。**
- 被调研的“方法”：内置 openconnect v9.21 + 自带 `wintun.dll` + 生成的 `vpnc-script-win.js`（只配网卡地址/MTU、加防环主机路由、回报 `CISCO_*`）+ worker 用 `New-NetRoute -NextHop 0.0.0.0` 注入路由 + NRPT 做按域名 DNS + 全局模式用 `0.0.0.0/1`、`128.0.0.0/1`。

---

## 0. 一句话结论

| 问题 | 结论 | 证据强度 |
| --- | --- | --- |
| Windows 上能不能完成 TLS、`/auth`、CSTP，连上这台 AnyConnect 服务？ | **能。现场日志已经出现 `CSTP connected. DPD 90, Keepalive 32400`** | A（现场） |
| Windows 上 openconnect 会不会执行我们生成的脚本、把 `TUNIDX`/`CISCO_*` 传进来？ | **会。** 官方 v9.21 源码与二进制字符串都确认走 `cscript.exe /e:JScript`，环境变量齐全 | A（源码 + 二进制 + 本机 cscript 实测） |
| 内置库到底有没有 Wintun 支持？ | **有。** `wintun.dll` 与全部 Wintun API 都在；但上游 9.21 明确标注 **experimental** | A（源码 + 二进制） |
| 连上之后内网流量能不能真的从 Wintun 进出？ | **机制正确，但还没有在这台 Windows 上观察到数据包。** 缺一次管理员会话里的端到端实测 | C（待验收） |
| 现场 09:58:34 的断开是不是“VPN 不通”？ | **不是。** 是旧 worker 的心跳误杀 + 计划任务指向旧 exe；现场用的构建早于修复 | A（日志 + 文件时间 + 代码） |

> 因此，当前最准确的说法是：
>
> **“Windows 客户端能连上该服务”已经有硬证据；“连上后内网能通”还不能签字。**
>
> 不能拿 macOS 的结果直接替代 Windows 的数据面验收：两端共用服务端行为，但不共用网卡、路由和 DNS 的实现。

---

## 1. 先把“通”拆成三层

| 层 | 要回答的问题 | 现状 |
| --- | --- | --- |
| L1 服务可达 | DNS/TCP/TLS 到 `vpn.risencn.com:1218` 是否通？账号能否通过 `/auth`？ | **已证明**（现场日志） |
| L2 隧道建立 | 服务端是否接受 AnyConnect/CSTP？Windows 客户端是否创建 Wintun、执行脚本、拿到地址？ | **协议/CSTP 已证明；Wintun/脚本只证明“能执行”，未证明“配置成功”** |
| L3 数据面 | 路由是否真的把包送进 Wintun？NRPT 是否真的把内网域名交给 VPN DNS？TCP 能否到达内网服务？ | **未证明** |

现场日志只走到 L2 的 CSTP，L3 一个字节都没有被观察过。把 L1/L2 的证据当成 L3 的结论，是这次调研要避免的错误。

---

## 2. 方案在 Windows 上的真实调用链

代码里的实际顺序（不是设计意图）：

1. 提权 worker 生成脚本，拼参数：
   ```text
   openconnect.exe --protocol=anyconnect --user=<user> --passwd-on-stdin \
     --script=<session>\script\vpnc-script-win.js <server>
   ```
   见 `src-tauri/src/vpn/connector.rs:34-39`。密码走 stdin，不出现在命令行。

2. openconnect 完成 TLS/`/auth`，收到 CSTP 配置后创建 Wintun 网卡（没有现成 TAP 时），再执行脚本：
   ```text
   cscript.exe /e:JScript "<script>"
   ```
   见官方 v9.21 `script.c:543-553,578`；`CreateProcessW(..., FALSE, ...)` 表示**不继承句柄**，所以脚本 stdout 接不到 worker 的管道（`script.c:600`）。

3. 脚本从环境变量拿 `TUNIDX`、`TUNDEV`、`INTERNAL_IP4_ADDRESS`、`INTERNAL_IP4_NETMASK`、`INTERNAL_IP4_MTU`、`INTERNAL_IP4_DNS`、`VPNGATEWAY`、`CISCO_DEF_DOMAIN`、`CISCO_SPLIT_INC_*`、`reason` 等；这些由 `library.c:1572,1583,1589` 和 `script.c:294-391` 设置。

4. 脚本用 `netsh` 配地址/MTU、加 VPN 服务器防环主机路由，并把 `WS-WORKBENCH key=value` 写进 `script.log`。

5. worker 增量读 `script.log`，拿到 `tunidx`/`ip4` 后调用 `SplitTunnel::apply` 或 `FullTunnel::apply`，用 `New-NetRoute -NextHop 0.0.0.0` 装路由，用 `Add-DnsClientNrptRule` 装按域名 DNS。

这条链的每一段都能在源码里对上；问题在于第 4/5 段的“成功”没有被现场日志证明。

---

## 3. 证据矩阵

等级说明：

- **A**：现场日志、实际二进制、官方 v9.21 源码、本机可复现实验。
- **B**：微软官方文档、同机生产级客户端（Cisco AnyConnect）的路由表、官方脚本同分支。
- **C**：只靠代码阅读/设计推理，尚未在该 Windows 机器上观察到结果。

| # | 结论 | 证据 | 等级 |
| --- | --- | --- | --- |
| 1 | 内置 openconnect 是官方 v9.21 MinGW64/GnuTLS 构建 | `openconnect.exe --version` 输出 `OpenConnect version v9.21`、`GnuTLS 3.8.11`；`src-tauri/resources/apps/openconnect/README.md` 记录了 GitLab CI 产物与 SHA256 | A |
| 2 | AnyConnect 协议受支持 | `--version` 的 `Supported protocols: anyconnect (default), ...`；现场日志的 `POST /auth` 与 CSTP | A |
| 3 | 该服务在 Windows 上能完成认证与 CSTP | 现场日志 09:58:31 `CSTP connected. DPD 90, Keepalive 32400` | A |
| 4 | Windows 用 `cscript.exe /e:JScript` 执行 `--script` | v9.21 `script.c:543-553,578`；二进制字符串 `cscript.exe /e:JScript` | A |
| 5 | 脚本 stdout 不继承，必须写文件 | `script.c:600` 的 `CreateProcessW(..., FALSE, ...)`；`worker.rs:188-213` 从 `script.log` 读 | A |
| 6 | 脚本环境变量包含 `TUNIDX`/`TUNDEV`/`CISCO_*` | `library.c:1572,1583,1589`；`script.c:294-391`；本机 cscript 测试通过 | A |
| 7 | 脚本有 10 秒硬超时 | `script.c:602,620-623`；二进制字符串 `Script did not complete within 10 seconds.` | A |
| 8 | 脚本非零退出会让 openconnect 失败 | `script.c:608-616`：`Script '%s' returned error %ld` → `-EIO` | A |
| 9 | 内置库编译了 Wintun | `libopenconnect-5.dll` 含 `wintun.dll`、`WintunCreateAdapter`、`WintunStartSession`、`WintunReceivePacket` 等字符串；`wintun.c` 全套实现 | A |
| 10 | Wintun 在 9.21 被上游标为 experimental | `tun-win32.c:750-755`：`WARNING: Support for Wintun is experimental and may be unstable...` | A |
| 11 | 创建 Wintun 需要管理员 | `wintun.c:107-112` 把 `ERROR_ACCESS_DENIED` 映射成 `-EPERM`；`tun-win32.c:427-430` 打印 `Access denied creating Wintun adapter...` | A |
| 12 | 分流模式的接口配置与官方脚本一致 | 我们的 `script.rs:119-127` 与官方 `vpnc-script-win.js:189-202` 都是 `netsh interface ip set address <TUNIDX> static <ip> <mask> store=active`（不带网关） | A |
| 13 | 分流路由用 on-link 在 Windows 上语义正确 | 微软 `MIB_IPFORWARD_ROW2` 文档：本地链路路由的 `NextHop` 为全零；同机 Cisco AnyConnect 的 `192.168.136.0/22` 就是 `On-link` | B |
| 14 | 官方脚本的分流路由用 `INTERNAL_IP4_ADDRESS` 作下一跳，不是 0.0.0.0 | `vpnc-script-win.js:234-241`：`route add <net> mask <mask> <internal_gw> if <TUNIDX>` | A |
| 15 | NRPT 是 Windows 官方的按命名空间 DNS 机制 | `win_dns.rs:61-91`；微软 `Add-DnsClientNrptRule` 文档；同机 Cisco 适配器 DNS 仍是 223.5.5.5，说明内网域名分流很可能也走 NRPT（需管理员才能列规则） | B |
| 16 | VPN DNS 自己也有 `/32` 隧道路由 | `split.rs:330-338` 把每个 VPN DNS 加进 `static_routes` | A |
| 17 | 现场断开是旧 worker 心跳误杀 | 现场消息没有新格式 `（{}s 未刷新）`；worker 路径与主程序路径不一致；现场构建早于修复 | A |
| 18 | 连上后数据包真的从 Wintun 进出 | **没有现场观测**；本机非管理员、无可用测试账号、Cisco AnyConnect 正占用默认路由 | **缺** |

---

## 4. 证据一：服务端可达性（现场日志）

现场关键行（09:58:25–09:58:35）：

```text
09:58:31  CSTP connected. DPD 90, Keepalive 32400
09:58:34  [worker] 主进程心跳已停止，自动收尾并退出
09:58:34  [状态] 断开中
09:58:34  [状态] 未连接
```

这说明：

- TLS 已经协商成功（现场前面的日志有 `SSL negotiation`、`Connected to HTTPS`）。
- `/auth` 已经通过（否则会出现 `Login failed` / `Failed to complete authentication`；本机 11-05 的失败日志就是这两句）。
- 服务端接受了这个账号，并进入了 CSTP；`DPD 90, Keepalive 32400` 是服务端下发的参数。
- 这不是“协议不兼容”“服务端拒绝 Windows 客户端”之类的问题。

**它不能说明**：

- Wintun 网卡是否创建成功；
- `netsh` 是否配置成功；
- `New-NetRoute` 是否装进去；
- NRPT 是否生效；
- 内网 TCP 是否可达。

因为 CSTP 之后 3 秒 worker 就被心跳逻辑收掉了，脚本阶段大概率还没完成。

---

## 5. 证据二：脚本执行链路

### 5.1 官方源码确认的调用方式

v9.21 `script.c`：

```c
static const char *script_engine(const char *path) {
    const char *dot = strrchr(path, '.');
    if (dot && strcasecmp(dot, ".js") == 0)
        return "cscript.exe /e:JScript";
    return "cscript.exe";
}
...
if (asprintf(&cmd, "%s \"%s\"", script_engine(vpninfo->vpnc_script),
             vpninfo->vpnc_script) == -1)
...
if (CreateProcessW(NULL, script_w, NULL, NULL, FALSE, cpflags,
                   script_env, NULL, &si, &pi)) {
    ret = WaitForSingleObject(pi.hProcess, 10000);
```

这解释了三件事：

1. 为什么 `.js` 脚本能跑：cscript 被显式指定 JScript 引擎。
2. 为什么脚本里的 `WScript.echo` 在 worker 日志里看不到：`bInheritHandles=FALSE`。
3. 为什么脚本必须瘦：**10 秒**内不结束，openconnect 直接判 `Script did not complete within 10 seconds.`。

### 5.2 环境变量确认

`library.c`：

```c
script_setenv(vpninfo, "TUNDEV", legacy_ifname, 0, 0);
...
if (vpninfo->tun_idx != -1)
    script_setenv_int(vpninfo, "TUNIDX", vpninfo->tun_idx);
...
script_setenv(vpninfo, "TUNDEV", legacy_ifname, 0, 0);
```

`script.c` 的 `prepare_script_env()` 设置 `CISCO_SPLIT_INC`、`CISCO_SPLIT_EXC`、`CISCO_DEF_DOMAIN`、`CISCO_SPLIT_INC_n_ADDR/MASK/MASKLEN`、`INTERNAL_IP4_*`、`reason` 等。

本机实测（`cargo test --lib vpn::script::tests::windows_script_reports_split_scope_through_cscript`）通过：用真正的 `cscript.exe` 跑 `reason=connect`，脚本把

```text
WS-WORKBENCH defdomain=risencn.com
WS-WORKBENCH split=192.168.136.0/22
WS-WORKBENCH split=10.8.0.0/16
```

同时写进 stdout 和 `script.log`。

### 5.3 但我们的脚本把退出码丢了

我们的脚本：

```javascript
var accumulatedExitCode = 0;
function run(cmd) {
    ...
    accumulatedExitCode += oExec.ExitCode;
    ...
}
...
if (reason == "connect") {
    ...
    WScript.Quit(0);   // 140 行
}
...
WScript.Quit(0);       // 161 行
```

`accumulatedExitCode` 只被声明和累加，**从来没有被使用**。官方脚本最后一行是：

```javascript
WScript.Quit(accumulatedExitCode);
```

后果：如果 `netsh interface ip set address` 失败（Wintun 网卡还没就绪、被安全软件挡住、接口索引暂时无效），脚本仍然以 0 退出，openconnect 认为脚本成功，worker 继续装路由，UI 进入“已连接”，但网卡没有地址，流量出不去。**这正是“CSTP connected 但内网不通”最容易出现的静默失败。**

注意：不能简单改成 `WScript.Quit(accumulatedExitCode)`。`route add` 在“路由已存在”时会返回非零，异常退出后重连可能因此直接失败。要先给命令分级：接口地址/MTU 失败必须报错；防环主机路由“已存在”可容忍。

### 5.4 日志编码与非 ASCII 路径（已修复）

调研时发现三层问题，都会让“日志看起来有、其实读不到”：

1. **FSO 默认写 ANSI**：脚本原来用 `OpenTextFile(..., 8, true)`，中文 Windows 上写 GBK。`tundev` 是 Wintun 适配器名，而适配器名来自**主机名**（`tun-win32.c:824-831` 用 `vpninfo->hostname`），所以中文计算机名会让 `script.log` 出现 GBK 字节。
2. **worker 用 `read_to_string`**：对非 UTF-8 返回 `Err`，整份日志一条都不处理，`tunidx`/`ip4` 永远拿不到，界面停在“连接中”直到 120 秒超时。
3. **JScript 里的会话路径没有转义非 ASCII**：`escape_js_string()` 只处理 `\` 和 `"`。如果 Windows 用户名是中文，`C:\Users\张三\AppData\...` 会原样拼进 `.js`；cscript 无 BOM 时按 GBK 解码，整份脚本编译失败，openconnect 只留下一句 `Failed to spawn script`。

本机复现（旧脚本，`TUNDEV=测试网卡`，其余地址为空，不执行 `netsh`）：

```text
WS-WORKBENCH tundev=��������
...
UnicodeDecodeError: 'utf-8' codec can't decode byte 0xb2 in position 97
```

修复（本次已落地）：

- `script.log` 改为 **UTF-16LE**（FSO `TristateTrue`，`-1` 参数），中文网卡名和本地化 `netsh` 输出都能原样保存；
- worker 改用 `script::read_log()`：识别 UTF-16LE/UTF-16BE BOM，旧 ANSI/UTF-8 文件走 `from_utf8_lossy` 回退，至少保证 ASCII marker 还能解析；
- `escape_js_string()` 把非 ASCII 字符转成 `\uXXXX`，即使会话路径含中文，生成的 `.js` 仍是纯 ASCII；
- 新增真实 cscript 测试：中文会话目录 + `TUNDEV=测试网卡`，读回 `tundev=测试网卡` 和 `DONE reason=connect` 均通过。

### 5.5 `done` 标记与路由竞态（已修复）

脚本的顺序是：

```text
report tunidx / ip4 / ...   ← 先写日志
netsh ... set address ...   ← 再配地址
route add ...               ← 再加防环路由
report done                 ← 最后写 done
```

旧 worker 把 `"done" => {}` 忽略掉，`tunnel_ready()` 只看 `vpn_ip` 和 `tun_index`；这两个在 `netsh` 之前就已经写进 `script.log`。主循环一旦读到它们就会调用 `start_engine()`，此时 cscript 可能还在执行 `netsh`。多数机器上 PowerShell 启动的几百毫秒会把时间差盖过去，但慢机器/安全软件拦截时，`New-NetRoute` 可能作用在还没配地址的接口上。

现在 worker 用 `script_done` 门控：只有收到 `done=connect` 才允许 `tunnel_ready()` 为真；`done=disconnect` 会清掉这个标志。`tunnel_ready_requires_ip_and_interface` 已覆盖“有 IP、有索引、但 done 未到”这一档。

---

## 6. 证据三：Wintun 能用，但上游自己说它 experimental

### 6.1 二进制里确实有 Wintun

`libopenconnect-5.dll` 中的字符串：

```text
wintun.dll
Could not load wintun.dll
openconnect.WintunCreateAdapter
WintunOpenAdapter
WintunCloseAdapter
WintunGetAdapterLUID
WintunStartSession
WintunEndSession
WintunGetReadWaitEvent
WintunReceivePacket
WintunReleaseReceivePacket
WintunAllocateSendPacket
WintunSendPacket
```

`wintun.c` 的实现路径：

- `LoadLibraryExW(L"wintun.dll", NULL, LOAD_LIBRARY_SEARCH_APPLICATION_DIR)` —— 只从 exe 所在目录找，所以 `wintun.dll` 必须和 `openconnect.exe` 同层（现在满足）。
- `WintunCreateAdapter(vpninfo->ifname_w, L"OpenConnect", NULL)`。
- `ERROR_ACCESS_DENIED` → `-EPERM`，并打印管理员提示。

### 6.2 但 9.21 的 Wintun 是 experimental

`tun-win32.c:750-755`：

```c
if (adapter_type == ADAPTER_WINTUN)
    vpn_progress(vpninfo, PRG_ERR,
                 _("WARNING: Support for Wintun is experimental and may be unstable. If you\n"
                   "  encounter problems, install the TAP-Windows driver instead. See\n"
                   "  %s\n"),
                 "https://www.infradead.org/openconnect/building.html");
```

这条警告会以 **PRG_ERR** 打到 stderr，worker 会把它当普通日志收进 `tail`。它不是我们的 bug，但它是方案最大的外部不确定性：上游自己都不保证稳定。

### 6.3 适配器名字和描述

- 名字：没有 `-i` 时用主机名（`tun-win32.c:824-831`），冲突时加后缀。
- 描述：Wintun 把 `TunnelType` 拼成 `"<TunnelType> Tunnel"`（`wintun api/adapter.c:558-559`），openconnect 传的是 `L"OpenConnect"`，所以设备描述是 **`OpenConnect Tunnel`**，不是 `README.md` 里写的 `Wintun Userspace Tunnel`。

`route.rs:732` 的识别正则包含 `openconnect`，所以仍能认出来；但文档需要改，否则以后有人按描述排查会被带偏。

### 6.4 管理员是硬要求

非管理员时 `WintunCreateAdapter` 返回 `ERROR_ACCESS_DENIED`，openconnect 打印：

```text
Access denied creating Wintun adapter. Are you running with Administrator privileges?
```

当前 worker 通过计划任务提权，正常情况下满足；但如果计划任务没注册、被 UAC 拒绝、或被安全软件降权，就会走到这里。现场日志里没有这句，说明当时至少已经过了 openconnect 启动阶段（或者还没到创建网卡就被心跳杀了）。

---

## 7. 证据四：路由与 DNS 语义

### 7.1 `NextHop 0.0.0.0` 的官方含义

微软 `MIB_IPFORWARD_ROW2` 文档对 `NextHop` 的说明：

> For a remote route, the IP address of the next system or gateway. If the route is to a local loopback address or an IP address on the local link, the next hop is unspecified (all zeros).

也就是说，`New-NetRoute -NextHop 0.0.0.0` 表达的是“目标在本地链路”，对应 `route print` 里的 `On-link`。这正是“从这块网卡出去”的语义。

### 7.2 同机 Cisco AnyConnect 的路由表是旁证

本机当前有一条 Cisco AnyConnect 的隧道（非管理员只能看 `route print`）：

```text
0.0.0.0          0.0.0.0      192.168.136.1   192.168.136.98     2
192.168.136.0    255.255.252.0    On-link      192.168.136.98   257
```

- 默认路由指向隧道网关（全局模式）。
- `192.168.136.0/22` 是 `On-link`，接口是隧道自己的 IP `192.168.136.98`。

这说明：

- 生产级 AnyConnect 客户端在同一台 Windows 上就是用 on-link 的方式把内网网段指向虚拟网卡的；
- 我们的 `New-NetRoute ... -NextHop 0.0.0.0` 产生的就是同一种 `On-link` 条目；
- 服务端下发的内网范围很可能就是 `192.168.136.0/22`（与脚本测试里的示例一致）。

### 7.3 但官方脚本的下一跳写法不同

同目录自带的官方 `vpnc-script-win.js` 在 split 模式里：

```javascript
run("route add " + network + " mask " + netmask +
    " " + internal_gw + " if " + env("TUNIDX"));
```

`internal_gw` 是 `INTERNAL_IP4_ADDRESS`（隧道自己的地址），不是 `0.0.0.0`。它和我们一样把接口地址配成不带网关的 split 形态，但下一跳填的是“假网关”。

结论：

- 两种写法都指向同一块 Wintun 网卡；Wintun 是 L3，不解析 ARP，下一跳值本身不参与转发。
- 官方脚本证明“用隧道自己的地址作下一跳”在 Wintun 上是被接受的；微软文档证明 `0.0.0.0` 是 on-link 的标准表达；Cisco 的 `On-link` 条目证明这种条目在真实客户端上工作。
- 但**没有一条证据证明我们的 `0.0.0.0` 条目在 Wintun 上真的把包送进了驱动**。这是 L3 验收要看的第一个点。

### 7.4 DNS：NRPT + VPN DNS 的 `/32`

- `win_dns.rs:61-91` 生成 `Add-DnsClientNrptRule -Namespace '<domain>' -NameServers '<vpn dns>'`，只删自己 `DisplayName` 前缀的规则。
- `split.rs:330-338` 先把每个 VPN DNS 加成 `{dns}/32` 的隧道路由，再装 NRPT。
- `Dnscache` 没跑时 `ensure_dns_client()` 会尝试 `Start-Service`；失败则退回“只给隧道网卡配 DNS”。

这条链路在代码上是闭合的：域名查询命中 NRPT → 交给 VPN DNS → 到 VPN DNS 的 `/32` 路由指向 Wintun → 查询进隧道。缺的仍然是“实际查询有没有回来”。

### 7.5 全局模式的 `/1`

`fulltunnel.rs:154-162` 装 `0.0.0.0/1` + `128.0.0.0/1`，`route.rs` 用 `-RouteMetric 1`。官方脚本在“无 split-include 的全局模式”会先：

```javascript
run("netsh interface ip set interface " + env("TUNIDX") + " metric=1 store=active");
```

我们没设接口 metric。因为 `/1` 比默认路由更具体，路由选择上通常不受接口 metric 影响；但 `New-NetRoute -RouteMetric 1` 在接口 metric 较大时是否会被系统调整，需要在真机 `Get-NetRoute` 看生效值。这是 P1 验收项，不是当前的阻塞项。

---

## 8. 现场 09:58:34 到底发生了什么

### 8.1 证据

1. 现场消息是：
   ```text
   [worker] 主进程心跳已停止，自动收尾并退出
   ```
   新代码的格式是：
   ```text
   [worker] 主进程心跳已停止（{}s 未刷新），自动收尾并退出
   ```
   现场少了秒数，说明跑的是旧 worker。

2. 现场 worker 路径与主程序路径不一致：
   - worker：`D:\ws-workbench\ws-workbench.exe`
   - 主程序/openconnect：`D:\软件\VPN\ws-workbench`

   这是计划任务 `/TR` 还指向旧目录的典型表现。旧 worker 用旧心跳逻辑，主程序用新脚本/新 openconnect，版本混跑。

3. 现场测试时间 09:58，而当前 release exe 的构建时间是 **10:08:40**；便携 zip 是 **09:45:32**。09:58 这次测试不可能包含 10:08 的修复。

4. 新 release exe 里已经能搜到 `未刷新` 字符串，说明修复确实编进去了。

### 8.2 新代码做了什么

- 主进程心跳线程：`commands.rs:725-755`，启动阶段先看到 `Disconnected` 不再停止续命；只有 `Failed`，或“已经见过活动阶段后的 `Disconnected`”才停。
- worker：`worker.rs:344-374`，心跳过期阈值 10 秒，且要求连续两次（间隔 ≥1 秒）都过期；日志写出实际秒数。
- 计划任务：`elevate.rs` 的 `worker_task_matches` 现在要求任务 XML 里的 exe 和 session 目录都匹配；不匹配就重新注册，不再复用旧任务。

本机测试通过：

```text
vpn::worker::tests::single_stale_heartbeat_does_not_kill_the_tunnel ... ok
vpn::commands::tests::heartbeat_ignores_the_startup_disconnected_status ... ok
```

### 8.3 结论

**现场那次断开不是 VPN 不通，是部署/版本问题。** 它证明了“旧 worker 会在 CSTP 后 3 秒收掉隧道”，但没有证明新代码能通，也没有证明数据面能通。

下一步必须先让现场跑上 **10:08 之后重新打包的构建**，否则再测一次仍然是旧逻辑。

---

## 9. 目前不能签字的缺口

| 缺口 | 为什么现在不能下结论 | 怎么补 |
| --- | --- | --- |
| Wintun 网卡是否创建成功 | 现场日志停在 CSTP 后 3 秒，没有 `Using Wintun device ...` 行 | 新构建连一次，看 worker.log 是否有 `Using Wintun device '<hostname>', index N` |
| `netsh` 配地址是否成功 | 旧脚本退出码被忽略；现在地址失败会非零退出，但仍需真机确认 | 看 worker.log 的 `[脚本] 脚本报告错误` / `EXIT`；`Get-NetIPAddress -InterfaceIndex N` |
| `New-NetRoute -NextHop 0.0.0.0` 是否真的转发 | 只有文档/旁证，没有 Wintun 上的包观测 | `Get-NetRoute` + `tracert` + `ping`/`Test-NetConnection` |
| NRPT 是否命中 | 规则可能装了但未生效；`Dnscache` 可能需要重启 | `Get-DnsClientNrptRule` + `Resolve-DnsName` 内网/公网对比 |
| VPN DNS 是否可达 | `/32` 路由装了，但包可能没进隧道 | `Resolve-DnsName` 超时/失败即是线索 |
| 服务端 split-exclude | 脚本回报了 `splitexc`，worker 没处理 | 看服务端是否下发；需要时补 `CISCO_SPLIT_EXC_*` 路由 |
| 全局模式 `/1` 的接口 metric | 官方脚本会设接口 metric=1，我们没设 | `Get-NetRoute` 看生效 metric；必要时补 `Set-NetIPInterface` |

---

## 10. 代码级风险清单

### P0：不修就不建议对外宣称“已通”

| ID | 风险 | 证据 | 影响 | 建议 |
| --- | --- | --- | --- | --- |
| P0-1 | 脚本退出码被丢弃 | 旧 `script.rs` 的 `run()`；官方 `vpnc-script-win.js:345` | 配地址失败仍显示“已连接”，流量黑洞 | **已修**：`set-address` 走 `mustRun`，失败非零退出并回报 `error`；MTU/防环主机路由只告警 |
| P0-2 | 非 ASCII `TUNDEV` / 会话路径 | 本机复现 `0xB2` 字节；旧 `read_to_string`；旧 `escape_js_string` 不转义非 ASCII | 中文主机名/用户名机器上读不到 `tunidx`，或脚本直接编译失败 | **已修**：`script.log` UTF-16LE + `script::read_log`；`escape_js_string` 输出 `\uXXXX`；真实 cscript 测试通过 |
| P0-3 | `done` 被忽略，路由可能早于地址配置 | 旧 `worker.rs` 的 `"done" => {}` 与 `tunnel_ready()` | 慢机器上 `New-NetRoute` 作用在未配置接口，连接失败 | **已修**：`script_done` 门控，只有 `done=connect` 才 ready |

### P1：必须在真机验收

| ID | 风险 | 证据 | 影响 | 建议 |
| --- | --- | --- | --- | --- |
| P1-1 | Wintun 上游 experimental | `tun-win32.c:750-755` | 驱动/转发不稳定，上游不保证 | 目标机型实测；失败时回退 TAP-Windows |
| P1-2 | `0.0.0.0` 下一跳未在 Wintun 上验证 | 微软文档 + Cisco 旁证；官方脚本用 `internal_gw` | 路由装了但包不出去 | 验收时 `Get-NetRoute` + `tracert`；必要时切到官方写法 |
| P1-3 | split-exclude 未处理 | `script.rs:107` 回报，`worker.rs:179` 落到 `_ => {}` | 服务端排除的网段仍被送进隧道 | 补 `CISCO_SPLIT_EXC_*` 回报与绕行路由 |
| P1-4 | 全隧道未设接口 metric=1 | `fulltunnel.rs:154-165`；官方脚本 189-192 | `/1` 路由生效值可能被接口 metric 拉高 | `Get-NetRoute` 看实际 metric；必要时 `Set-NetIPInterface` |
| P1-5 | 脚本 10 秒预算 | `script.c:602,620-623` | 慢机器上 3 条 `netsh` 可能超时 | 实测脚本耗时；保持脚本精简 |

### P2：文档/稳健性

| ID | 问题 | 建议 |
| --- | --- | --- |
| P2-1 | `resources/apps/openconnect/README.md` 说“9.21 对 Wintun 已不再标记为实验特性” | **已改**：改为“仍是 experimental，上游警告可能不稳定” |
| P2-2 | 同文件说描述是 `Wintun Userspace Tunnel` | **已改**：改为 `OpenConnect Tunnel`（Wintun 拼 `TunnelType + " Tunnel"`） |
| P2-3 | `route.rs:732` 清理正则含 `anyconnect|vpn` | 不要与其它 VPN 客户端同时运行；清理前同时校验 `/1` 前缀 + 描述 |
| P2-4 | NRPT 是否要重启 `Dnscache` 才生效 | 验收项 2；需要时在 `apply_namespace_rules` 后重启并记日志 |

---

## 11. 最小端到端验收（建议直接照做）

### 11.1 前置

1. 用 **10:08 之后**的代码重新打包，确认 zip/exe 时间晚于 `faef9eb`；不要用 09:45 的 zip。
2. 退出 Cisco AnyConnect 或至少断开它的隧道，避免两条默认路由/两个 VPN 客户端打架。
3. 以管理员身份运行，或先移除再重新注册 `WsWorkbenchVpnWorker` 计划任务（会弹一次 UAC）。
4. 准备一个可用的测试账号；本机 11-05 保存的 `fengwenshan` 密码认证失败，不能作为验收账号。

### 11.2 连接后看 worker.log

必须出现（顺序大致）：

```text
[worker] 提权状态：管理员，程序 <path>，主程序 <path>，会话目录 ...
[worker] PowerShell 完整日志：...powershell.log；脚本完整日志：...script.log（UTF-16LE）
[隧道] openconnect 版本：OpenConnect version v9.21
[隧道] openconnect 已启动 ... --script=...vpnc-script-win.js ...
Using Wintun device '<hostname>', index N
WARNING: Support for Wintun is experimental and may be unstable...
WS-WORKBENCH tunidx=N
WS-WORKBENCH ip4=<server-assigned>
[分流] 路由注入分流已生效...
```

`程序` 与 `主程序` 必须是同一个路径；不是就说明计划任务还在跑旧 exe。

### 11.3 管理员 PowerShell 核对

把 `N` 换成日志里的 `tunidx`：

```powershell
Get-NetAdapter -InterfaceIndex N | Format-List Name,InterfaceDescription,Status
Get-NetIPAddress -InterfaceIndex N -AddressFamily IPv4
Get-NetRoute -InterfaceIndex N -AddressFamily IPv4 |
  Format-Table DestinationPrefix,NextHop,RouteMetric
Get-DnsClientNrptRule |
  Where-Object DisplayName -like 'ws-workbench:*' |
  Format-List DisplayName,Namespace,NameServers
Resolve-DnsName oa.risencn.com
Resolve-DnsName www.baidu.com
Test-NetConnection <内网IP> -Port 443
tracert -d <内网IP>
```

通过标准：

- 网卡 `Status = Up`，IPv4 地址等于服务端下发；
- 路由表里有服务端 split-include 网段和 VPN DNS 的 `/32`，`NextHop = 0.0.0.0`，metric 为 1；
- NRPT 规则存在，`oa.risencn.com` 返回内网地址，`www.baidu.com` 仍返回公网地址；
- `Test-NetConnection` 到内网 443 成功；
- `tracert` 第一跳是隧道对端，不是物理网关。

### 11.4 失败时的分诊

| 现象 | 先看什么 | 可能原因 |
| --- | --- | --- |
| 没有 `Using Wintun device` | worker.log 是否有 `Access denied` / `Could not load wintun.dll` | 非管理员、`wintun.dll` 不在 exe 同层、驱动安装失败 |
| 有网卡但无 IPv4 | `script.log` 是否有 `ip4=`；`Get-NetIPAddress` | `netsh` 失败但退出码被吞（P0-1） |
| 有 IP 但无路由 | `Get-NetRoute`；worker.log 的 `[分流]` | `New-NetRoute` 失败/竞态（P0-3） |
| 有路由但 ping 不通 | `tracert`、`Get-NetRoute` 的 `NextHop` | Wintun 转发问题（P1-1/P1-2）；试官方 `internal_gw` 写法 |
| 内网域名解析失败 | `Get-DnsClientNrptRule`、`Resolve-DnsName` | NRPT 未生效（重启 Dnscache）、VPN DNS `/32` 路由没装、DNS 查询没进隧道 |
| 全局模式公网也断 | `Get-NetRoute` 的 `/1`、`Get-DnsClientServerAddress` | `/1` 路由未生效、VPN DNS 不可达 |

### 11.5 可选：不依赖 VPN 服务端的 Wintun 数据面自测

如果暂时没有可用账号，可以单独验证“Wintun + 地址 + on-link 路由”这一段：

1. 用 `wintun.dll` 创建临时适配器（管理员）；
2. `netsh` 配一个测试地址（如 `198.18.0.1/24`）；
3. `New-NetRoute -DestinationPrefix 203.0.113.0/24 -InterfaceIndex <idx> -NextHop 0.0.0.0`；
4. 用户态读 Wintun 包并回显 ICMP；
5. `ping 203.0.113.1`；
6. 删除路由和适配器。

这能证明 on-link 路由确实把包送进 Wintun，但不能替代真实服务端的 CSTP/脚本链路。当前机器非管理员，且 Cisco AnyConnect 占用默认路由，所以本次没有执行。

---

## 12. 结论与建议

1. **方案方向是对的。** 它没有绕开 openconnect 的官方 Windows 路径，而是用了官方支持的 `--script` + Wintun + 路由注入；脚本的 split 模式接口配置与官方脚本同形。
2. **“服务能通”已经有硬证据，但只到 CSTP。** 现场日志排除了协议/账号/服务端拒绝 Windows 的根本性问题。
3. **“内网能通”还没有证据。** 目前所有 L3 结论都来自源码、文档和旁证；没有一条来自这块 Wintun 网卡上的实际数据包。
4. **现场那次失败是版本混跑，不是方案失败。** 旧 worker + 旧计划任务在 CSTP 后 3 秒收掉隧道；新构建（10:08 之后）才有 10 秒/两次判定的心跳修复。
5. **在重新验收前，先修 P0-1/P0-2/P0-3。** 否则即使网络是通的，也可能因为脚本退出码、中文主机名、路由竞态而表现为“已连接但没流量”，把验收信号弄脏。
6. **不要对外承诺“Windows 一定能通”。** 当前准确的对外口径是：“Windows 已能完成认证与 CSTP；数据面已实现，待目标机型端到端验收；Wintun 在 openconnect 9.21 仍属 experimental。”
7. **macOS 不动。** 本次没有修改任何 macOS 分支；所有风险与修复都限定在 Windows 脚本/worker/文档。

---

## 13. 为现场排查补的日志（2026-11-06 追加）

这次是在别人的电脑上跑，拿不到交互式排查环境，所以日志按“只看一个目录就能定位”来补。

### 13.1 `script.log`：完整脚本执行记录

新脚本每个关键动作都写两遍：stdout（人工调试）和 `script.log`（worker 读取）。

- 启动：`reason`、`MODE`、脚本路径、`WScript.FullName/Version`、计算机名、用户名、架构；
- 环境：`TUNIDX`、`TUNDEV`、`INTERNAL_IP4_*`、`VPNGATEWAY`、`ORIGINAL_GW`、`CISCO_DEF_DOMAIN`、`CISCO_SPLIT_INC/EXC` 的每一条；
- 每条命令：`RUN <cmd>`、`EXIT <code> (<ms>)`、`OUT <stdout+stderr>`（单条最多 16 KB，超出标记截断）；
- 收尾：`done=connect` / `done=disconnect`、总耗时、累计退出码；
- 失败：`error=<label> exit <code>` + `FATAL ...`，随后 `WScript.Quit` 非零，openconnect 会明确报 `Script ... returned error`。

`script.log` 用 UTF-16LE 写，中文 `TUNDEV`、本地化 `netsh` 输出不会再变成 GBK；`script::read_log()` 同时兼容旧的 ANSI/UTF-8 文件。connect 时上一份日志会改名为 `script.prev.log`，worker 检测到行数变短会自动从头读。

### 13.2 `powershell.log`：所有 PowerShell 调用的命令与完整输出

worker 启动时设置 `WS_VPN_PS_LOG=<session>\powershell.log`；`winps::run()` 在这一层统一落盘，因此 `route.rs`、`split.rs`、`fulltunnel.rs`、`win_dns.rs` 的调用全部覆盖，不需要逐个改。每条记录包含：

```text
===== #序号 pid=... =====
CMD:
<完整 PowerShell 脚本>
RESULT:
OK / FAIL
<完整输出或错误>
```

失败时错误文本也带命令（当前错误信息从“PowerShell 执行失败: ”空字符串，改成“失败原因 + 命令”）。

### 13.3 worker 网络状态快照

`start_engine()` 成功后、以及任何 `fail()` 之前，worker 会跑一次只读诊断并写进 `worker.log`：

```text
Get-NetAdapter -InterfaceIndex N
Get-NetIPAddress -InterfaceIndex N -AddressFamily IPv4
Get-NetRoute -InterfaceIndex N -AddressFamily IPv4
Get-DnsClientNrptRule | Where-Object -Property DisplayName -Like 'ws-workbench:*'
Get-DnsClientServerAddress -InterfaceIndex N
Get-Service Dnscache
```

这样即使现场只发回一个 `worker.log`，也能看到“网卡 up 没 up、地址有没有、路由 next hop 是什么、NRPT 有没有”。

### 13.4 一键收集脚本

`scripts/collect-vpn-logs.ps1`（便携包根目录也放了一份）：

```powershell
powershell -ExecutionPolicy Bypass -File .\collect-vpn-logs.ps1
```

在目标机器上运行后，桌面会生成 `ws-workbench-vpn-logs-<时间>.zip`，包含：

- 每个会话的 `worker.log`、`script.log`、`script.prev.log`、`powershell.log`、`status.json`、生成后的 `vpnc-script-win.js`；
- `request.redacted.json`：`password` 字段替换为 `***REDACTED***`，原始 `request.json` 不复制；
- `system\`：`ipconfig /all`、`route print -4/-6`、网卡、IP、路由、NRPT、DNS、Dnscache、openconnect 进程、OS 版本。

本机已用开发会话实测：zip 能生成，`request.redacted.json` 中密码为 `***REDACTED***`，未出现原始密码。非管理员运行时，`Get-NetAdapter`/NRPT 部分会写“拒绝访问”；脚本会照常打包已拿到的部分。

### 13.5 本机验证

```text
vpn::script::tests::windows_script_handles_chinese_session_dir_and_tundev ... ok
vpn::script::tests::read_log_decodes_utf16le_with_bom ... ok
vpn::script::tests::read_log_keeps_ascii_markers_in_legacy_ansi_file ... ok
winps::tests::powershell_trace_records_command_and_output ... ok
vpn::worker::tests::tunnel_ready_requires_ip_and_interface ... ok
vpn::worker::tests::ingest_script_log_reads_utf16_and_recovers_after_rotation ... ok
vpn::elevate::tests::waiting_for_worker_requires_the_worker_start_line ... ok
vpn::elevate::tests::task_xml_decoder_handles_utf16le_with_bom ... ok
cargo test --lib vpn::  → 158 passed, 0 failed（管理员权限）
```

此前非管理员 shell 下 `interfaces_and_routes_carry_driver_description_on_this_machine` 与
`powershell_json_queries_work_on_this_machine` 会因 `Get-NetAdapter` 返回“拒绝访问”失败；
管理员权限下 156 个测试全部通过，与本次改动无关。

macOS 分支没有改动；`read_log` 的 UTF-16 识别只在 Windows 生成的文件上生效，macOS 仍走 UTF-8 回退。

---

## 14. 2026-11-06 11:02 现场日志判读

目标机器：Windows 11 家庭版 25H2（26200.9711），主机名 `DESKTOP-K92B3SR`，用户 `user`。

### 14.1 这次日志说明了什么

收集到的 `worker.log` **只有一行**：

```text
[心跳] 开始为 worker 续命
```

同时：

- 没有 `[worker] 启动`、没有 `status.json`；
- 没有 `script.log` / `powershell.log`（worker 没走到生成脚本、调用 PowerShell 的阶段）；
- `openconnect-process.txt` 为空，系统中没有 openconnect 进程；
- `Get-NetAdapter` 里没有 Wintun/OpenConnect 网卡；`routes-ipv4` 没有 `/1` 或 `/32`；`nrpt` 为空；
- `vpnc-script-win.js` 的时间是 9:58:30，是上一轮现场留下的旧文件，不是这次生成的。

结论：**这次失败发生在 worker 启动之前，不是 openconnect 协议、认证或数据面问题。** 主进程的心跳线程起来了（这行由 `commands.rs` 写），但 worker 进程没有写出它的第一行日志。

### 14.2 为什么之前的日志没抓到

旧版 `start_worker()` 的 Windows 路径有两个盲点：

1. `schtasks /Run` 返回 0 就当成“计划任务启动成功”并立即返回；任务动作即使因为旧路径、权限或杀软失败，`schtasks` 也可能返回 0。
2. 退回 UAC 时只等批处理返回，不确认 worker 是否真的写下启动日志；后来 11:28 的日志进一步证明，`Start-Process -Wait` 还会一直等到 worker 退出（见 14.5）。

现场请求里的路径含中文：

```text
D:\工作文件\1105\ws-workbench-1.0.0-portable-win64\ws-workbench.exe
```

当时怀疑 `run_elevated_batch()` 写出的 UTF-8 `.cmd` 被 `cmd.exe` 按 GBK 读，导致中文路径乱码。**11:28 的日志证明这个怀疑不是本次根因**：`run-worker.cmd` 内容正确，worker 也确实启动了；真正的问题是心跳线程启动太晚。中文路径仍然值得防范，但已不是主要矛盾。

### 14.3 已补的启动侧日志与验证

- 主进程在提权前写 `[启动] 正在请求启动 worker：管理员=...，程序=...，会话目录=...`；
- `start_worker()` 记录计划任务是否存在/是否匹配、`schtasks /Run` 的返回、任务定义前 1200 字；
- `schtasks /Run` 返回后**等待 worker 写下 `[worker] 启动`**（15s），等不到就记日志并回退 UAC；
- UAC 路径也等待 worker 启动，等不到直接返回错误，不再让心跳线程空跑；
- 批处理开头加 `chcp 65001 >nul`，避免中文路径被按 GBK 读；
- 等待期间主动 `touch_alive()`，避免 worker 因为主进程心跳线程还没启动而误判“主进程已死”；
- 收集脚本新增 `elevate.exit` / `create-task.cmd` / `run-worker.cmd`，以及 `vpn-task.txt` / `vpn-task-xml.txt`。

这样下一次如果 worker 还是没起来，`worker.log` 会直接出现 `[启动]`/`[elevate]` 行，收集包里也能看到计划任务实际指向的路径。

### 14.4 下一步

1. 用最新便携包替换目标机器上的整个目录（不能只替换 exe，`apps/openconnect` 也要一致）；
2. 在 VPN 页点“注册免 UAC 任务”（内部带 `/F`，会覆盖旧任务）；如果显示“移除任务”，也可以先移除再注册；
3. 重新连接一次；
4. 如果仍失败，直接跑最新的 `collect-vpn-logs.ps1`，把新 zip 发回。新包里的 `worker.log` 不再是只有一行心跳。

### 14.5 11:28 现场：真正的根因（心跳线程启动太晚）

11:28 的收集包把上一节的猜测推翻了，也找到了更直接的根因：

```text
11:28:02  [elevate] 改用 UAC 提权启动 worker
11:28:02  [worker] 启动，pid 30476
11:28:08  CSTP connected. DPD 90, Keepalive 32400
11:28:13  [worker] 主进程心跳已停止（11s 未刷新），自动收尾并退出
11:28:14  [启动] worker 启动请求已返回：UAC 提权
11:28:14  [心跳] 开始为 worker 续命
```

关键顺序：**worker 先启动、先连上 CSTP，主进程的心跳线程却是 worker 退出之后才启动的。**

原因在 `run_elevated_batch()`：

```powershell
Start-Process -FilePath 'cmd.exe' ... -Verb RunAs -Wait
```

`-Wait` 等的是整个进程树。批处理里的 `start` 拉起的 worker 是 `cmd.exe` 的子进程，所以 PowerShell 一直等到 worker 退出才返回；`start_worker()` 因此也一直不返回，`commands.rs` 里启动心跳线程的代码排在它后面，11 秒内根本没执行。worker 的 10 秒宽限期一过，连续两次检查都读不到心跳，就按“主进程已死”收尾。

本机做了最小复现：批处理用 `start` 拉起一个睡 5 秒的子进程，带 `-Wait` 的 `Start-Process` 用了 **6.09 秒** 才返回，不带 `-Wait` 只用 **0.01 秒**。这证明 `-Wait` 确实会等到 `start` 拉起的子进程退出。

这也解释了 9:58 那次的现象，以及为什么修了心跳阈值仍然会断：不是阈值不够大，是心跳线程根本没开始。

同时，11:28 的任务 XML 本身是**正确**的：

```xml
<Command>"D:\工作文件\1106\ws-workbench-1.0.0-portable-win64(1)\ws-workbench.exe"</Command>
```

但 `worker_task_definition()` 把 `schtasks /XML` 的 UTF-16LE 输出按 UTF-8 解码，中文路径变成 `D:\�����ļ�\...`，导致 `worker_task_matches()` 永远返回 false，于是每次连接都多走一次 UAC。任务本身没有问题，是读取/比对错了。

已修改：

- `run_elevated_batch()` 去掉 `Start-Process -Wait`，改为轮询 `elevate.exit`；批处理一写完退出码就返回，不再等 worker 退出；
- 任务 XML 先重定向到临时文件，再用 `script::read_log()` 按 BOM 解码 UTF-16LE，中文路径可以正常比对；
- 收集脚本默认输出到**当前目录**，不再写到桌面。

这两处修完后，UAC 路径会在 worker 启动后立即返回，`commands.rs` 马上启动心跳；任务路径也能被正确识别，正常情况下不再回退 UAC。

### 14.6 11:54 现场：任务匹配又踩了“读不到就当匹配”的坑

11:54 的主程序在：

```text
D:\工作文件\1026\ws-workbench-1.0.0-portable-win64(2)\ws-workbench.exe
```

但它通过计划任务拉起的 worker 在：

```text
D:\工作文件\1106\ws-workbench-1.0.0-portable-win64(1)\ws-workbench.exe
```

任务 XML 实际写的是 **1106**，而主程序是 **1026**；日志却出现了：

```text
[elevate] 计划任务存在，指向当前程序/目录: true
```

原因是 `worker_task_definition()` 用 `cmd /C ... > temp` 读取任务 XML 时返回了 `None`，`worker_task_matches()` 又退回「recorded 目录一致就 true」。结果 `/Run` 拉起 1106 的旧 worker。

任务错配是独立问题；11 秒收尾并不是旧 worker 独有——12:39 用同版本 worker 仍复现，根因见 14.9 的心跳文件 mtime。

已改成：

- 直接用 `schtasks.exe /Query /XML` 的原始 stdout 字节，按 UTF-16LE/BE BOM 解码，不再经过 cmd 重定向；
- 任务定义读不到时**直接返回 false**，不再拿 recorded 目录当匹配依据；宁可重新注册/UAC，也不拉起一个无法确认版本的 worker；
- 任务 XML 解出替换字符（`\uFFFD`）也视为读取失败。

另外，“点击移除任务没反应”的原因是 `schtasks /Delete` 在普通权限下可能被拒绝，而前端 `removeElevation()` 没有 `catch`，Promise reject 后界面既不刷新也不写日志。现在：

- 普通删除失败会自动走一次 UAC 提权删除（`delete-task.cmd`）；
- 前端开始/成功/失败都会写日志，不会再出现“点了完全没反应”。

Windows 托盘图标也从白色线条图改成应用自身的彩色 `32x32.png`，避免在浅色任务栏上看不清；macOS 仍保留模板图。

### 14.7 托盘退出后解压目录仍被占用

便携包直接从解压目录运行，worker 和 `apps/openconnect/openconnect.exe` 也在该目录下。旧托盘退出逻辑只写 `stop.flag` 就立刻 `app.exit(0)`；worker 还没来得及杀掉 openconnect、还原路由，用户马上删目录就会看到：

```text
操作无法完成，因为其中文件或文件夹已在另一程序打开
```

此时任务管理器的“应用”页通常看不到它，因为占用者是隐藏的 worker 或 openconnect，要到“详细信息”里看 `ws-workbench.exe` / `openconnect.exe` / `cscript.exe`。

已改：

- Windows 托盘“退出”改为后台等待 worker 最多 8 秒，确认 worker 退出后再退出主程序；
- 即使状态已经写成 `Disconnected`/`Failed`，只要 worker pid 还活着，也会写停止请求，让它先杀 openconnect；
- worker 超时仍未退出时，在 `worker.log` 写入明确提示，便于下一次定位。

### 14.8 12:21 现场：同时跑了三个 ws-workbench

用户机器上实际同时存在：

```text
3080  D:\工作文件\1026\ws-workbench-1.0.0-portable-win64(2)\ws-workbench.exe
9188  D:\工作文件\1026\ws-workbench-1.0.0-portable-win64(2)\ws-workbench.exe
34212 D:\软件\VPN\ws-workbench\ws-workbench.exe
```

前两个在同一个解压目录，第三个是另一份拷贝。它们可能是“主程序 + worker”的组合，也可能是重复双击出来的主程序；旧版本没有单实例保护，每个主程序都会建托盘图标和 worker。用户只点了其中一个托盘的“退出”，其他实例仍然握着 1026 目录或 `D:\软件\VPN` 目录里的 exe/DLL，所以删除目录会报“文件已在另一程序打开”，而任务管理器“应用”页又看不到无窗口的 worker。

已加 Windows 命名 mutex 单实例：

- 第二个 GUI 进程启动时会尝试把已有窗口叫到前台，然后直接退出；
- `--vpn-worker` 在 `main.rs` 里已经提前分流，不会经过这个 mutex，不会被误杀；
- macOS 不受影响。

### 14.9 12:39 现场：心跳文件写空字节，Windows 不更新 mtime

12:39 的现场已经排除了任务错配：

```text
12:39:20 [elevate] 计划任务存在，指向当前程序/目录: false
12:39:20 [elevate] 任务已过期，改用 UAC 提权拉起当前版本 worker
12:39:21 [worker] 提权状态：管理员，程序 ...1026...\ws-workbench.exe，主程序 ...1026...\ws-workbench.exe
12:39:21 [心跳] 开始为 worker 续命
12:39:31 [worker] 主进程心跳已停止（11s 未刷新），自动收尾并退出
```

主程序和 worker 已经是同一个文件，心跳线程也在 worker 启动时就开始，仍然 11 秒收尾。

根因在 `VpnSession::touch_alive()`：

```rust
fs::write(self.alive_path(), b"")
```

Windows 上对**已经存在的空文件**再次写空内容，只截断、不更新 `LastWriteTime`。worker 用文件 mtime 算心跳年龄，于是永远看到第一次 `touch_alive` 创建文件的时间；10 秒阈值一到，第 11 秒就判定主进程死亡并拆隧道。

本地用 Rust 复现：

```text
empty write twice: empty_changed=false
non-empty write:   nonempty_changed=true
```

已改成每次写入当前毫秒时间戳，保证 mtime 一定刷新；并加了回归测试
`vpn::session::tests::touch_alive_refreshes_mtime_even_on_windows`，把 mtime 拨回一分钟前再 `touch_alive`，必须重新变新。

`cargo test --lib vpn::` → **158 passed, 0 failed**。

### 14.10 托盘退出再加 12 秒看门狗

12:39 之后又出现“点了托盘退出，进程还在”的反馈。为避免 Tauri 的退出事件或 `tasklist` 检查卡住，Windows 托盘退出现在有两层：

1. 正常路径：`shutdown_and_wait` 等 worker 最多 8 秒，然后 `app.exit(0)`；
2. 看门狗：同一个退出点击同时启动 12 秒定时器，时间到仍没退出就 `std::process::exit(0)`，强制结束主进程。

`is_process_alive()` 的 `tasklist` 超时也从 15 秒降到 3 秒，避免退出流程看起来卡死。`worker.log` 里现在一定会出现 `[退出]` 行，便于区分“没点到退出”和“退出了但进程残留”。

### 14.11 12:52 现场：开发默认规则进了生产，且 DOMAIN-SUFFIX 不会自动注入子域路由

12:52 的连接本身已经成功，问题在分流：

```text
[分流] 路由注入模式：域名 ["risencn.com"]，网段 []
[分流] 首批没有拿到可用的内网地址
[自检] risencn.com 解析失败：不知道这样的主机
```

两件事叠加：

1. `src/composables/useVpn.ts` 的 `addProfile()` 里硬编码了开发环境的默认规则：
   ```ts
   const risen = newRule('DOMAIN-SUFFIX', 'risencn.com', 'VPN')
   ```
   新建任何配置都会自动带上它。这是开发数据进生产包，已删除；新配置现在从空规则开始。
2. 规则模式的实现是**路由注入**：后台线程只解析规则里的**字面域名**。
   `risencn.com` 本身只有 SOA、没有 A 记录；`oa.risencn.com` / `cas.risencn.com`
   才有 A 记录（公网 DNS 返回 `192.168.18.41`）。因此：
   - `DOMAIN-SUFFIX risencn.com` 只会去解析 `risencn.com`，拿到 0 个地址；
   - 不会自动解析 `oa` / `cas`，也就不会给 `192.168.18.41` 装 `/32` 路由；
   - 浏览器访问 `cas` / `oa` 时走物理默认路由，看起来就是“没走 VPN”。

已做：

- 删除生产代码里的 `risencn.com` 默认规则；
- 新配置默认规则类型改为 `DOMAIN`，提示用户写完整主机名；
- `DOMAIN-SUFFIX` 的 placeholder 当时写明“只解析该域名本身，子域请用完整域名或全局模式”；14.14 已改为“连同所有子域”；
- 中文 Windows 的“不知道这样的主机”现在按“没有 A 记录”提示，不再误报成 DNS 链路故障。

当前配置的规则不会自动删除；要访问 `oa` / `cas`，选一种：

- 加两条 `DOMAIN`：`oa.risencn.com`、`cas.risencn.com`，走 VPN；
- 或加 `IP-CIDR 192.168.18.0/24` 走 VPN（如果整个内网都在这个网段）；
- 或直接用**全局模式**。

> 14.14 已把 Windows 首选路径改为「NRPT → 127.0.0.1:53 本机转发器 → 回包前注入」；
> 14.11 里“只能逐条写完整域名”的描述只适用于转发器不可用时的回退路径。

### 14.12 12:51 UI：注册按钮“日志成功、状态不变”

现场：

```text
12:51:43 INFO 正在注册免 UAC 计划任务…
12:51:44 OK 计划任务已注册，之后连接不再弹 UAC
```

但左侧仍是“尚未注册免 UAC 计划任务”，按钮也还是“注册免 UAC 任务”。

原因有三层：

1. `vpn_setup_elevation()` 只检查 `worker_task_exists()`，任务存在就返回成功；
2. `vpn_elevation_info()` 用的是更严格的 `worker_task_matches()`，任务存在但没指向当前程序/目录时 `task_ready=false`；
3. 前端没有 loading 态，点完按钮既不禁用也不显示“注册中”，看起来像没点。

已改：

- `ElevationInfo` 增加 `task_exists`，区分“任务存在”和“任务可用”；
- 后端 `ensure_worker_task()` 创建后重试校验 `worker_task_matches()`，不匹配返回 `Ok(false)`；
- `vpn_setup_elevation()` 对 `Ok(false)` 返回明确错误，不再假报成功；
- 前端增加 `elevationBusy`：点击立即禁用按钮并显示“注册中…/移除中…”，`finally` 复位；
- 任务存在但不可用时也显示“移除任务”，用户不会卡在“只能注册、不能删旧任务”的死路；
- `decode_task_xml()` 增加无 BOM 的 UTF-16LE/BE 识别，避免任务定义读成乱码导致 `task_ready` 永远 false。

`cargo test --lib vpn::` → **158 passed, 0 failed**。

### 14.13 13:23 模式切换：为什么标签会“跳回去再跳回来”

后端 `vpn_set_proxy_mode()` 只写 `mode` 文件就返回；worker 要撤/装规则分流、半默认路由和 DNS，现场约 4 秒。前端 `settleOptimistic()` 原来最多等 3 秒，3 秒到了就把乐观高亮交回 `applied_mode`；但此时 worker 还没把新模式写进状态，`applied_mode` 仍是旧值，于是标签先跳回旧模式，下一轮状态到了再跳到新模式。

已改成：

- 最多等 15 秒；
- 只有 `applied_mode === mode` 才撤掉乐观值；
- 15 秒仍没确认才告警并交回真实状态。

切换模式本身走的是 `vpn_set_proxy_mode`，日志是 `[模式] 运行期切换`，**不会重启 openconnect，也不会重建隧道**；13:23 和 13:32 的日志里隧道仍然是同一个 `vpn.risencn.com1`，没有新的 `openconnect 已启动`。用户看到的“重新构建”主要是标签回跳和 4 秒切换窗口造成的。

全局模式的 `0.0.0.0/1 + 128.0.0.0/1` 也不会覆盖更具体的本地连接路由，所以本地网段仍然直连；这是设计行为（`fulltunnel.rs` 顶部注释明确写了）。要判断某个地址走哪边，用：

```powershell
Find-NetRoute -RemoteIPAddress 192.168.18.41
Find-NetRoute -RemoteIPAddress 8.8.8.8
```

### 14.14 13:40 Windows 对齐 macOS：DOMAIN-SUFFIX 覆盖任意子域

目标是让 Windows 上的：

```text
DOMAIN-SUFFIX risencn.com
```

和 macOS 一样，`oa.risencn.com`、`cas.risencn.com` 以及以后新增的子域都自动走 VPN，不需要逐个写完整域名。

做法：

```text
应用 -> 系统解析器 -> NRPT -> 127.0.0.1:53 本机转发器
     -> VPN DNS -> 解析 A 记录 -> 先装 /32 隧道路由 -> 回包给应用
```

- 本机转发器由 worker 以管理员身份绑定；Windows 上 53 不要求特权端口，但可能被别的 DNS 服务占用。
- NRPT 的 `NameServers` 从 VPN DNS 改为 `127.0.0.1`，只对命中域名的查询生效，不改系统 DNS。
- `forward_loop()` 从 macOS 专用改为 macOS/Windows 共用；Windows 注入路由时传入真实的 `tun_index`。
- 用一条随机子域探针确认 NRPT 真的把查询送进 loopback；1.5 秒没有查询就回退到旧路径，不假装成功。
- 回退路径仍是「NRPT 直连 VPN DNS + 每 15 秒主动解析字面域名」；worker.log 会写明是 53 被占用、NRPT 失败还是 loopback 没有查询。

已知边界：

- 仍只覆盖系统解析器（`GetAddrInfo`）；浏览器自带 DoH/DoT 时不受 NRPT 控制。
- 本机转发器先装 `/32` 再回包，因此 `DOMAIN-SUFFIX` 不再有「15 秒窗口」；回退路径仍有。
- 如果现场 53 被占用或系统不接受 loopback DNS，日志会明确写出回退，不会静默变成旧行为。

`cargo test --lib vpn::` → **158 passed, 0 failed**。

### 14.15 16:30 现场：转发器没收到子域查询 —— NRPT 命名空间少了前导点

现场（便携包 `20261106-1422`）只配了一条 `DOMAIN-SUFFIX risencn.com`，访问
`https://oa.risencn.com:9508/...` 仍然不走 VPN；把规则改成 `oa.risencn.com` 就正常。
日志关键几行：

```text
16:31:18 INFO [分流] 本机 DNS 转发器已监听 127.0.0.1:53，上游 VPN DNS [223.5.5.5]
16:31:19 INFO [分流] NRPT 已指向 127.0.0.1:53，但 1.5 秒内没有查询进入本机转发器；
          可能系统不接受 loopback DNS，回退到「NRPT 直连 VPN DNS + 后台解析」
16:31:21 INFO [分流] 按域名解析：1 个域名，首批注入 0 个内网地址
```

转发器绑定成功、NRPT 也写成功，但**探针（随机子域）一次都没进来**。这排除了「53 被占用」，
也说明问题不在 loopback：14.14 写的规则是 `Add-DnsClientNrptRule -Namespace 'risencn.com'`，
而 NRPT 里**不带前导点**的命名空间是精确匹配，只有 `risencn.com` 本身命中；
`ws-probe-*.risencn.com`、`oa.risencn.com` 都不在里面。日志里 14.14 把「命名空间按后缀匹配」
当成了事实，这一步错了。

对照微软 VPN 配置样例（<https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/vpn/vpn-profile-options>）：

```xml
<DomainNameInformation>
  <DomainName>hrsite.corporate.contoso.com</DomainName>   <!-- 精确：单台主机 -->
  ...
  <DomainName>.corp.contoso.com</DomainName>              <!-- 后缀：整族子域 -->
```

同一个域名族里，精确规则不带点，后缀规则带点 —— 带点才是后缀语义。

另外这份现场日志还确认了 `/32` 是必需的：`oa.risencn.com` 的公网 A 记录是
`192.168.18.41`（`Resolve-DnsName ... -Server 223.5.5.5` 实测），不在服务端下发的
`192.168.136.0/22` 里；没有 `/32` 就会走物理默认路由。

改法（`win_dns::build_apply_script`）：

- 每个域名写两条 NRPT 规则：`.risencn.com`（后缀，覆盖任意子域）+ `risencn.com`（精确，保留旧行为）；
- 带点的那条包在 `try/catch` 里，个别系统拒绝带点命名空间时不拖垮精确规则；
- 探针超时从 1.5 秒放宽到 2.5 秒（刚写完规则时 DNS Client 还要刷新策略表）；
- 探针失败时把 `Get-DnsClientNrptRule` 里**实际存下来的命名空间**打进日志，
  下次现场一眼能区分「规则没点」和「loopback 不通」；
- 回退路径不再只解析字面域名：每 3 秒扫一次 `Get-DnsClientCache`，把属于规则域名
  （含子域）的 A 记录补成 `/32`，所以即使 NRPT 后缀规则被拒绝，子域也能在 3 秒内走 VPN
  （首次连接可能失败一次，刷新后正常）。

现场验证看三行：

1. `Get-DnsClientNrptRule | Where-Object DisplayName -like 'ws-workbench:*' | Format-List Namespace,NameServers`
   每个域名应有两条（带点 + 不带点）；
2. worker.log 应出现「NRPT 已写好 N 条规则（域名 + 前导点子域后缀）」而不是「没有查询进入本机转发器」；
3. 浏览器开 `oa.risencn.com` 后，`Find-NetRoute -RemoteIPAddress 192.168.18.41` 的第一跳应是隧道。

`cargo test --lib vpn::` → **166 passed, 0 failed**（新增 8 条：后缀命名空间、命名空间归一化、
规则计数解析、DNS 缓存后缀边界/去重/容错、规则计数脚本）。

### 14.16 17:40 换新包就得手点「注册免 UAC 任务」：合并成一次提权

现场反馈：每换一个新包（解压到新的日期目录），连接时又要弹 UAC；为了之后不弹，
得先点一次「注册免 UAC 任务」、再过一次 UAC —— 等于每次升级都要手动两步。

原因不在一处，而是两条路径叠加：

1. 任务定义里写死了「exe 路径 + 会话目录」，新包换了目录 → `worker_task_matches` 为 false；
2. `vpn_connect` 在任务不可用时**先**调一次 `ensure_worker_task`（注册要过一次 UAC），
   再进 `start_worker` 的「刷新任务 + 拉起 worker」回退路径（又一次 UAC）。
   注册若在第二步校验没通过（重试 5×200ms 不够、或任务被策略改写），就退化成
   「每次连接弹一次 UAC」，用户只好回头去点那个按钮。

改法：

- `elevate::task_create_and_launch_command()`：一条批处理里先 `schtasks /Create ... /RL HIGHEST /F`
  再 `start "" <exe> --vpn-worker <dir>`，两条命令用 `&` 而不是 `&&` —— 注册被组策略拦了
  也要把本次 worker 拉起来，schtasks 的原始输出写进会话目录 `create-task.log`；
- `start_worker()` 的任务不可用分支改用这条合并命令，并在拉起后复查
  `worker_task_matches()`：成功就在日志里写「已自动注册免 UAC 任务」，失败则明确写
  「下次连接仍会弹一次 UAC」；
- `vpn_connect` **删掉**单独的预注册调用（那正是双弹的来源），改为 `start_worker()` 成功
  且任务确实匹配后才 `remember_task_dir()`；
- 面板提示改成三种状态各说清「会不会弹、弹几次」：可用（不再弹，覆盖解压可永久免）、
  存在但过期（下次连接弹一次并自动刷新）、不存在（首次连接弹一次并自动注册）。

结果：

| 场景 | 改前 | 改后 |
| --- | --- | --- |
| 同一目录内升级 | 每次连接弹 UAC（任务对不上） | 0 次（`/TR` 路径没变，直接 `/Run`） |
| 解压到新目录 | 手动点注册 + UAC，或每次连接弹 | 首次连接 1 次 UAC（顺带修好任务），之后 0 次 |
| 组策略禁止建任务 | 每次连接弹 1 次 | 每次连接弹 1 次（日志写明原因） |

> 安全性说明：这里没有把提权入口改成「用户可写的固定脚本」。计划任务永远指向
> 具体 exe 路径，且每次注册都要过一次 UAC；否则任何以当前用户身份运行的进程
> 都能替换那个脚本，从而静默拿到管理员权限 —— 那是本机提权漏洞，不能为了少点
> 一次 UAC 去换。

`cargo test --lib vpn::` → **167 passed, 0 failed**（新增 `create-task-and-launch` 命令拼装 1 条）。

> 17:50 追加：同一次改动里曾加过「解析地址」面板（域名 → IP → 建议网段 + 一键加 IP-CIDR），
> 现场反馈一个域名会解析出多个地址、列表太长，已整块回退（含 `VpnStatus.resolved`、
> `split` 的解析结果记录、`dns_client::question_name`）。要查某个域名解析到什么，
> 用 `nslookup <域名> <VPN DNS>` 即可，不必塞进面板。

### 14.17 18:05 三个「生效」时刻加系统通知

需求：连接生效、新规则生效、代理模式切换生效时各发一条系统通知（窗口最小化到托盘时也要知道结果）。

**没引 `tauri-plugin-notification`**，直接用系统自带能力，理由是便携包：插件在 Windows 上依赖
应用注册的开始菜单快捷方式（AppUserModelId），而本项目的发布物就是解压即用的 zip。

| | 触发点 | 判据 |
| --- | --- | --- |
| 连接生效 | `useVpn` 里 `watch(() => status.phase)` | `phase` 从非 `connected` → `connected`（状态事件会重复推送，必须比上一拍） |
| 规则生效 | `pushRulesIfConnected()` 成功之后 | 推送完成 + 300ms 后状态已刷新回来 |
| 模式生效 | `setProxyMode()` 等到 `waitForAppliedMode` | worker 回写了目标 `applied_mode`；10 秒没等到只写 warn 日志，不发通知 |

**踩到的坑：`-Command` 会吃掉 toast XML 里的双引号。** 实测：

```powershell
powershell.exe -Command "...$xml.LoadXml('<toast><visual><binding template=\"ToastGeneric\">...')"
# → Exception from HRESULT: 0xC00CE502（XML 解析失败），Show 再报「参数错误」
```

错误回显里 `template=` 后面的引号已经没了 —— 脚本要穿过「Rust 拼命令行 → Win32 解析 →
PowerShell 再解析」三层，`\"` 在某层被当成普通的 `"` 处理。改成 `-EncodedCommand`
（脚本按 UTF-16LE 编码再 base64）后零转义，实测退出码 0；顺带解决中文在此代码页下
变问号的问题。同一条命令写进文件、用 `-File` 跑也是 0，说明 toast 本身在便携 exe 上
可用（AUMID 用固定的 `WsWorkbench`，不注册快捷方式也能弹）。

回归测试 4 条（全部纯逻辑，不真的弹窗）：`clip` 压平换行并截断、toast 脚本做 XML 转义
（并断言单引号字符串里不出现裸 `'`）、AppleScript 转义、`encoded_command` 能 UTF-16 回读。

`cargo test --lib vpn::` → **171 passed, 0 failed**。

### 14.18 18:20 提权区块从界面撤掉，规则弹窗减到三项

两个界面上的「多余东西」，都是同一条原则：能自动做的不摆按钮，能靠开关表达的不加标签。

**提权**：面板左下角那块「提权方式 + 注册免 UAC 任务 / 移除任务」整块删掉。原因是注册这件事
已经在连接那次提权里做完了（`/Create /F` 顺带覆盖过期任务，等于「自动移除 + 自动注册」），
按钮除了让人以为「必须手动点一次」之外没有作用。现在提权状态只出现在日志的 `[启动]` /
`[elevate]` 行里，用户不需要先点按钮再连接。

**规则弹窗**：表单从「类型 / 内容 / 策略 / 启用 / 备注」减到前三项，去掉的几处：

| 删掉的东西 | 原因 |
| --- | --- |
| 类型下方的解释长句 | 三类规则名字已经说明白了 |
| 按类型变的长 placeholder | 太长、看着像已经填好，直接固定「请输入」 |
| 表单里的启用开关 | 状态在列表行的开关上，两处开关会互相矛盾 |
| 备注输入框 | 没人用，占一屏 |
| 添加/编辑按钮下方的提示文字 | 「追加到末尾、从上往下匹配」是内部实现细节 |

列表行同步去掉「已禁用」标签和备注副标题（开关本身就是状态），编辑/删除图标换成项目分组
模块那套铅笔与叉号 SVG。**编辑一条被禁用的规则并保存，它会被重新启用** —— 表单里没有开关，
改内容就等于重新启用；只想停用就直接点行上的开关。

### 14.19 18:35 「连上了没提示」的真凶：Win11 便携 exe 没有程序包标识符

现场反馈：连接成功后没有任何系统提示，而且点连接时按钮会「转一下 → 停 → 又开始转」。

**先说按钮闪烁。** `vpn_connect` 只是把请求文件写进会话目录就返回了，此时状态机还停在
`disconnected`，前端 `finally` 里把 `connecting` 清掉的瞬间就退回了「连接」文案；几百毫秒后
worker 才把 phase 推进到 `elevating/connecting`，界面又亮起来。加了 `waitForConnectOutcome()`：
先等 `phase` 离开静止态（见到 `elevating/connecting/configuring_routes` 之类），再等它落到
`connected|failed|disconnected`；一直没起步（UAC 框上还没点）最多等 60 秒就把按钮放回去。

**再说通知。** 用 `cargo run --example notify_probe` 走真实调用链，拿到真错误：

```
Exception calling "CreateToastNotifier" with "1" argument(s):
"The process has no package identity. (Exception from HRESULT: 0x80073D54)"
```

排查过程（每一步都有实测，不是猜）：

| 试法 | 结果 |
| --- | --- |
| 从 `pwsh` 里调同一段脚本、同一个 `WsWorkbench` AUMID | **成功**，退出码 0，通知历史里有记录 |
| 从 Rust 子进程调同一段脚本（stdio 分别 piped / null / 继承、加不加 `CREATE_NO_WINDOW`） | 全部 `0x80073D54` |
| 换系统已注册的 PowerShell AUMID（`{1AC14E77-…}\WindowsPowerShell\v1.0\powershell.exe`） | 从 pwsh 成功，从 Rust 子进程仍 `0x80073D54` |
| `SetCurrentProcessExplicitAppUserModelID('WsWorkbench')` + `HKCU\Software\Classes\AppUserModelId` 注册 | 仍 `0x80073D54` |
| `notify-rust` 4（`tauri-plugin-notification` 的 Windows 后端） | 同一个错，`Err(Os(HRESULT(0x80073D54)))` |
| `.NET NotifyIcon.ShowBalloonTip` 托盘气泡兜底 | 退出码 0，但**通知历史里查不到任何记录**（`wpndatabase.db`），等于没弹 |

结论：这不是脚本写法问题，是 Win11 24H2/25H2 对 `CreateToastNotifier` 的硬要求 —— 调用进程
要么有 MSIX 包身份，要么有同 AUMID 的开始菜单快捷方式。便携 zip 两者都没有。从 `pwsh` 能成，
是因为那台机器上的 pwsh 是打包版本，子进程带上了身份，属于「在我的终端里能跑」。

**改法**：系统通知只当「尽力而为」，发不出去必须**如实返回错误**（脚本首行
`$ErrorActionPreference = 'Stop'`，否则 `Show` 失败只进 stderr、进程照样 0 退出，调用方会
误判「已经弹出去了」）。失败时：

1. 写一行 `[通知] … 发送失败：<首行错误>` 进 VPN 日志，现场有据可查；
2. 后端补发 `vpn-notice` 事件，前端用 ant-design-vue 的 `notification` 在右上角弹一条应用内
   提示 —— 窗口在的时候用户照样知道「连上了 / 模式换了 / 规则生效了」。

同时把通知的**触发点从 webview 挪到 Rust 状态轮询线程**：窗口收进托盘、webview 被销毁时也要
能发；判定改成「和上一次状态快照比」，状态文件每 600ms 重写同样的内容不会连环弹；通知本身在
独立线程里发，绝不占轮询线程和命令线程（PowerShell 一次几百毫秒）。

新增/调整的回归测试：`toast_script_reports_failure_instead_of_exiting_zero`（断言首行是
`Stop`、且没有自己 `catch` 掉）、`only_the_transition_into_connected_notifies`、
`mode_switch_only_counts_while_already_connected`（连上那一拍 `applied_mode` 一起写入，不能
算成用户切模式）。`cargo test --lib vpn::` → **174 passed, 0 failed**。

### 14.20 18:52 现场日志判定：退出码 0 不等于「弹出来了」

装 18:36 那个包在真机上连了一次、又切了两次代理模式，日志里**一行 `[通知]` 都没有**（连接 18:43:56
生效，模式在 18:45:12 与 18:45:24 各生效一次）。失败分支才会写 `[通知] … 发送失败`，所以只能
说明 PowerShell 返回了 0 —— 也就是 `ToastNotificationManager.Show()` 被系统接受了，但屏幕上什么
都没出现。

结论：**退出码只能证明「系统收下了这条通知」，不能证明「用户看见了」**。便携 exe 只要没在系统里
注册成已知应用（没有 MSIX 包身份、没有同 AUMID 的开始菜单快捷方式），`Show` 就可能被静默丢弃。
所以原来那套「成功就不提示、失败才退回界面内提示」的设计是可证的错误。

**改法：两条通道都发，界面内提示是可靠的那条。**

| 通道 | 条件 | 说明 |
| --- | --- | --- |
| 界面内提示（右上角卡片） | 每次触发都发（`vpn-notice` 事件） | 唯一可靠；`success=true` 的「VPN 已连接」用成功样式 |
| Windows 系统通知 | 每次触发都尝试 | 窗口在托盘里时只能靠它；窗口在眼前时多一条也不算打扰 |

同时给 toast 脚本加了「按当前用户注册 AUMID」这一步（`HKCU\Software\Classes\AppUserModelId\WsWorkbench`
写 `DisplayName` / `IconUri` / `ShowInSettings`，免管理员、可反复写、失败不影响发通知）：注册过之后
系统设置的通知列表里才算有这么一个应用，被静默丢弃的概率最低。日志两种结局都要留痕：
`[通知] 已向系统提交「…」（是否显示取决于 Windows 通知设置）` 或
`[通知] 系统通知「…」发送失败：…`。

回归测试相应调整：`toast_script_reports_failure_instead_of_exiting_zero` 现在断言
「`CreateToastNotifier` 之后不能再有 `catch`」（注册段可以 catch，`Show` 不行），
`toast_script_escapes_xml_and_registers_aumid` 断言注册段与 XML 转义同时存在。

### 14.21 18:53 样式里的 px 一律用偶数

`11px` 写 `12px`、`13px` 写 `14px`。缩放不是 100% 时奇数尺寸会被摆到半个像素上，图标、边框、
文字都发虚。`1px` 的描边与分隔线是刻意留的发丝线，不受这条约束。全仓 79 处奇数 px 已按 +1 规则
改成偶数（字体 11→12、13→14，间距 3→4、5→6、7→8，圆角 3→4、9→10，点状指示 11→12）。

### 14.22 18:58 真机确认系统通知能弹 → 撤掉界面内卡片

装上 18:55 那个包再连一次，**系统通知真的弹出来了**（注册 AUMID 那一步起了作用：之前
`Show` 被静默丢弃，正是因为没有 `HKCU\Software\Classes\AppUserModelId\WsWorkbench`）。
于是：

1. **界面内提示撤掉**：`EVENT_NOTICE` / `VpnNoticePayload` / `vpn_notify` 命令 / 前端的
   `notification.info` 全部删除。一事件一条通知，不重复。验收只看系统通知。
2. **补上「断开」通知**：`notify::Trigger::Disconnected`，规则是「本来连着」→
   落到 `disconnecting` 或 `disconnected`。`disconnecting → disconnected` 是同一次断开，
   只报一条（否则会连弹两条）；`failed` / 未连接 → `disconnected` 不报。
   文案：`VPN 已断开 / {配置} 已断开，分流与路由已还原`。
3. **macOS 也发系统通知**：以前只有 Windows 分支被验证过，macOS 走
   `osascript -e 'display notification … with title … sound name "default"'`，现在明确
   写进 README 并补了 `sound name "default"`（通知中心默认只闪一下，容易漏）。两平台的
   触发判定完全共用同一段 Rust 代码，不存在「只在 Windows 生效」的分叉。
4. **代理模式切换的加载态加强**：原来只有选中的那颗按钮里有个 12px 小圈，等于没有。
   现在切换期间卡片标题左侧的图标位换成 18px 转圈、副标题改成「正在切换到「全局」，
   等待 worker 确认…」、卡片描边变主题色；下方那行锁提示同场景不再重复。

`cargo test --lib vpn::` → **175 passed, 0 failed**（新增
`disconnect_notifies_once_and_only_after_being_connected`）。

---

## 15. 参考资料

- OpenConnect 官方仓库：<https://gitlab.com/openconnect/openconnect>
- OpenConnect v9.21 源码 tarball（本次实际解包核对）：<https://www.infradead.org/openconnect/download/openconnect-9.21.tar.gz>
- OpenConnect v9.21 源码树：<https://gitlab.com/openconnect/openconnect/-/tree/v9.21>
- openconnect-gui：<https://gitlab.com/openconnect/openconnect-gui>
- vpnc-scripts：<https://gitlab.com/openconnect/vpnc-scripts>
- 官方 `vpnc-script-win.js`（自带副本头部引用的 commit）：<https://gitlab.com/openconnect/vpnc-scripts/-/blob/6cdb2fe2d75be324564fe27ff85a5e1bb17a66b0/vpnc-script-win.js>
- 微软 `New-NetRoute`：<https://learn.microsoft.com/en-us/powershell/module/nettcpip/new-netroute>
- 微软 `MIB_IPFORWARD_ROW2`（NextHop 全零 = on-link）：<https://learn.microsoft.com/en-us/windows/win32/api/netioapi/ns-netioapi-mib_ipforward_row2>
- Wintun（WireGuard 镜像）：<https://github.com/WireGuard/wintun>
- Wintun 官方：<https://www.wintun.net/>

---

## 附录 A：本机实测摘要

### A.1 二进制版本

```text
OpenConnect version v9.21
Using GnuTLS 3.8.11. Features present: RSA software token, HOTP software token,
TOTP software token, Yubikey OATH, System keys, DTLS, ESP
Supported protocols: anyconnect (default), nc, gp, pulse, f5, fortinet, array
Default vpnc-script (override with --script): ...\vpnc-script-win.js
```

### A.2 Wintun/cscript 字符串

```text
wintun.dll
Could not load wintun.dll
WintunCreateAdapter ... WintunStartSession ... WintunSendPacket
cscript.exe /e:JScript
Script did not complete within 10 seconds.
Script '%s' returned error %ld
```

### A.3 同机 Cisco AnyConnect 路由表（`route print -4` 摘要）

```text
0.0.0.0          0.0.0.0      192.168.136.1   192.168.136.98     2
192.168.136.0    255.255.252.0    On-link      192.168.136.98   257
```

### A.4 本机通过的测试

```text
vpn::script::tests::windows_script_reports_split_scope_through_cscript ... ok
vpn::win_dns::tests::generated_scripts_parse_as_powershell ... ok
vpn::worker::tests::single_stale_heartbeat_does_not_kill_the_tunnel ... ok
vpn::commands::tests::heartbeat_ignores_the_startup_disconnected_status ... ok
```

### A.5 本机复现的编码问题

```text
TUNDEV=测试网卡
script.log 第 97 字节 = 0xB2
UTF-8 解码失败：invalid start byte
```

### A.6 本机限制

- 当前 shell 非管理员：`Get-NetAdapter` / `Get-NetRoute` / `Get-DnsClientNrptRule` 均“拒绝访问”，无法创建 Wintun 或读 NRPT。
- 本机 Cisco AnyConnect 正在使用默认路由，直接跑 openconnect 会冲突。
- 11-05 保存的测试账号 `fengwenshan` 认证失败（`Login failed`），没有可用于验收的账号。
- WSL 不可用（`Wsl/E_ACCESSDENIED`），无法在本机起 ocserv 做 mock 服务端。

因此，本报告把“能证明的”和“还不能证明的”分开写；最后一块需要目标机器上的 10 分钟实测来补。
