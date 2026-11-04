# Clash 式规则分流 · 竖切（方案 A）设计

- 上级设计：`docs/superpowers/specs/2026-09-30-clash-like-routing-design.md`（下称"总设计"）
- 范围：总设计第 11 期的 **2 + 4**，外加 **UDP 按规则转发**（总设计第 3 期的一部分）一次做全
- 平台：仅 macOS；Windows 留下一期
- 状态：待实施

## 1. 目标与范围

一期纯逻辑层（`rule_engine` + `fakedns`）已完成并通过审查。本期把它接上运行期与配置/UI，做成**端到端可用**的竖切：用户在界面上编辑 Clash 式规则，连接后流量按规则分流。

**做：**

- macOS TUN 透明接管（自建 `utun`，smoltcp 用户态栈终结连接）。
- 七类匹配：`DOMAIN` / `DOMAIN-SUFFIX` / `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `IP-CIDR` / `MATCH`。
- fake-ip DNS（拦截 TUN 上的 `UDP:53` / `TCP:53`，分配 `198.18.0.0/16` 假 IP，双向映射，TTL+LRU 回收，`fake_ip_filter`）。
- 数据面：TCP 与（非 DNS 的）UDP 均按规则分流并转发。
- 配置与存储改造、Clash 式有序规则编辑 UI、彻底移除 `mode`。

**不做（留下一期）：**

- `RULE-SET` 外部规则集文件导入（`rule_engine` 的内存展开逻辑一期已就绪，本期 UI 不暴露、配置不写该项）。
- Windows 平台层。
- IPv6 接管（本期对 AAAA 返回空，逼应用走 IPv4）。
- `REJECT`（总设计 D4 明确不做）。

## 2. 关键决策（本期新增/细化）

| # | 决策 | 取舍 |
| --- | --- | --- |
| S1 | 用户态栈用 **`smoltcp`**（0BSD，无 GPL 污染） | 取代总设计 D7 的"全自研"；用成熟状态机换掉自写 TCP 的巨大风险，与"纯 Rust、单进程"不冲突 |
| S2 | VPN 出口采用 **双 TUN + 按流动态路由注入** | 见第 3 节；对现有 openconnect 接缝最小 |
| S3 | 新建配置的 `MATCH` 初值为 **直连（DIRECT）** | 首次连接即使隧道异常也不会断网；用户可在 UI 随时改 |
| S4 | 非 DNS 的 UDP **按规则转发** | 捕获层路由是 IP 级的、UDP 必然被收上来；丢弃即黑洞，故顺带做 UDP 映射转发 |
| S5 | 依赖新增 `smoltcp`、`nix`（`libc` 随 `nix` 转出） | 现有 `Cargo.toml` 均无 |

## 3. 架构

### 3.1 一句话

openconnect 只负责把隧道拉起来（提供 `utunZ` 与 VPN IP）并**不装任何路由**；路由决策权全部交给新的 TUN 引擎，引擎按规则决定每条连接走 `DIRECT` 还是 `VPN`。

### 3.2 三个关键机制

1. **自环绕过**：引擎自己发起的出站 socket 一律用 `IP_BOUND_IF` 钉死出口接口——`DIRECT` 钉物理网卡，`VPN` 钉 openconnect 的 `utunZ`。钉了接口即绕过路由表，不会被自己的捕获 TUN 二次捕获。
2. **路由接管**：捕获 TUN `utunC` 装 `0.0.0.0/1` + `128.0.0.0/1` 两条半默认路由（不动系统默认路由，回收时干净）；**VPN 服务器 IP 加一条走物理网关的 host 路由**，否则 openconnect 自身流量会绕回 TUN 形成死循环；VPN 出口的每条流，给其**真实目标 IP** 临时加 `/32 → utunZ` 的路由（比 `/1` 更具体，故优先命中），流结束撤销。
3. **进程反查**：macOS 用 `sysctl net.inet.tcp.pcblist_n` / `net.inet.udp.pcblist_n` 把"连接的本地端点"反查成 PID，再取进程名（需 root，worker 本就提权）。

### 3.3 拓扑

```
应用
 │  (utunC 的半默认路由把所有流量收上来)
 ▼
[捕获层] utunC  →  用户态栈 smoltcp（终结应用侧连接）
 │
 ├─ UDP/TCP:53 ─► [fake-ip DNS 服务]  分配 198.18.x.x、记录 假IP↔域名
 │
 ▼
[规则引擎]  按「假IP→域名」/「真 IP」/「源进程」/「目标端口」自上而下匹配
 │
 ├── DIRECT ─► 真解析 → 出站 socket 钉 en0
 └── VPN ────► 真解析(可走隧道 DNS) → 加 /32 路由 → 出站 socket 钉 utunZ
```

## 4. 组件

### 4.1 平台抽象

新增 `src-tauri/src/vpn/platform.rs`，以 trait 抽象四件事，macOS 先实现：

- `TunDevice`：建/销毁 `utun`，读写 IP 包。
- `RouteTable`：加/删路由（半默认、host、`/32`），查询存在性。
- `IfBinder`：把 socket 钉到指定接口（`IP_BOUND_IF`）。
- `ProcLookup`：本地端点 → PID → 进程名。

### 4.2 新增模块（`src-tauri/src/vpn/`）

| 模块 | 职责 | 依赖 |
| --- | --- | --- |
| `platform.rs` | 上述四件事的 trait + macOS 实现 | `nix` / `libc` |
| `tun_device.rs` | macOS `utun` 控制 socket（`PF_SYSTEM`/`SYSPROTO_CONTROL`/`UTUN_CONTROL_NAME`），设点对点地址与 MTU | `nix` |
| `netstack.rs` | smoltcp 驱动的用户态 TCP/IP 状态机：吐「连接事件 + 数据流」，接「回写数据」 | `smoltcp` |
| `router.rs` | 按匹配结果建出站连接、钉接口、双向搬运；TCP 与 UDP 各一套；半关闭/超时/回收 | netstack + platform |
| `proc_lookup.rs` | 连接 → PID → 进程名 | platform |
| `dns_srv.rs` | DNS 服务端：应答 `UDP/TCP:53`，用 `FakeIpPool` 分配假 IP；`fake_ip_filter` 命中走真解析 | `fakedns` |

### 4.3 复用（不重写）

- `rule_engine`（一期已完成）：编译与 `evaluate`。
- `fakedns`（一期已完成）：`FakeIpPool` 池与过滤；本期只在其上加 DNS 服务端。
- `route.rs`：`route_exists` / `add_route` 等助手。
- `config.rs`：加密信封边界。

### 4.4 改动

- **`script.rs`**：macOS vpnc-script **退化**为"只设接口 IP/MTU，不装任何路由"；删除按 `mode` 抢/不抢默认路由与 include/exclude `add_route` 的逻辑。
- **`worker.rs`**：`run_openconnect` 在 `tunnel_ready` 后装配引擎（建 `utunC`、路由、启动 netstack/router）；看门狗从"守护路由"改为"**引擎退出即撤 `utunC` 路由 + 清假 IP 映射**"；删 `include_rules`/`exclude_rules`/`local_subnets` 相关路由逻辑。
- **`commands.rs`**：`VpnRequest` 填新字段（规则、`default_policy`、`fake_ip_range`、`fake_ip_filter`、`use_vpn_dns`）；连接前的 CIDR 校验/重叠检测替换为按规则 kind 的校验。
- **`models.rs`**：删 `VpnMode` 与 `VpnStatus.mode`；`VpnRule` 扩为 `{id, kind, value, policy, no_resolve, remark, enabled}`；新增 `VpnRuleSet {rules, default_policy}`；`VpnProfile` 去 `mode`、加 `fake_ip_range`/`fake_ip_filter`/`use_vpn_dns`。复用 `VpnStatus` 既有 `tun_dev`/`vpn_ip`/`vpn_gateway`/`dns`/`mtu`/`routes`。
- **`session.rs`**：编排「提权 → 起 openconnect（不装路由）→ 等隧道就绪 → 建 `utunC` + 路由 → 起引擎」，停止时反序清理。

### 4.5 前端

- **`src/types/vpn.ts`**：删 `VpnMode`/`MODE_LABELS`/`MODE_DESCRIPTIONS`；`VpnRule` 改新形状；`VpnRuleSet` 加 `default_policy`；`VpnProfile` 去 `mode`、加假 IP 相关字段；`VpnStatus` 去 `mode`。
- **`src/composables/useVpn.ts`**：删 `setMode`；新增规则增/删/改/**拖拽排序**、按 kind 校验（域名格式、端口/区间、CIDR）；`default_policy` 读写；读写一律经 `safeStorage`。
- **`VpnRuleTable.vue`**：重写为 Clash 式**有序**编辑器——每行含 kind 下拉、按 kind 变化的取值输入、出口下拉、`no_resolve` 开关（仅 `IP-CIDR` 可见）、备注、启用；**底部钉死 `MATCH` 行**对应 `default_policy`（不可删、不可排序）。
- **`VpnProfileForm.vue`**：删 mode 选择；加 DNS 段（`fake_ip_range`、`fake_ip_filter`、`use_vpn_dns`）。
- **`App.vue`**：删托盘 `vpn-mode` 开关；状态 chip 沿用。
- **`VpnRoutePreview.vue` / `VpnPanel.vue` / `VpnLearnCard.vue`**：适配新规则形状（学习模式采纳的 CIDR 产出为 `IP-CIDR` 规则）。

## 5. 数据流

- **域名流**：应用查 DNS → 被 `utunC` 收上来 → `dns_srv` 回假 IP（`198.18.x.y`）并记 `假IP↔域名` → 应用连「假 IP:端口」→ smoltcp 终结 → 由假 IP 反查域名 → 规则匹配（域名 + 目标端口 + 源进程）→ 出口。
- **真 IP 直连流**：应用连真 IP → smoltcp 终结 → `proc_lookup` 取进程名 → 规则匹配（IP / 端口 / 进程）→ 出口。
- **出口**：
  - `DIRECT`：用物理网 DNS 真解析（域名流）→ 建 socket 钉 `en0`。
  - `VPN`：真解析（`use_vpn_dns` 为真时走隧道 DNS）→ 给目标 `/32 → utunZ` → 建 socket 钉 `utunZ`。
- **兜底**：无任何规则命中 → `default_policy`。
- **local/特殊流量**：本机所在子网走内核更具体的直连路由、天然不被 `utunC` 接管；组播/链路本地同理；ICMP 直通物理网卡；loopback 走 `lo` 不被捕获。

## 6. 错误处理与降级

沿用总设计第 9 节，本期具体化：

- 规则解析/编译失败 → **拒绝启动**（fail-closed，绝不猜出口）。
- `utunC`/路由建立失败 → 中断连接、回收已加路由、明确报错。
- **引擎崩溃 = 最危险**：看门狗发现引擎退出，立即撤 `utunC` 半默认路由、清假 IP 映射、恢复网络。
- 假 IP 池耗尽 → 淘汰 LRU 最旧条目。
- `DIRECT` 域名解析失败 → 把失败返回应用，不静默改写出口。
- 停止 / 取消 / 异常 → 一律保证路由与 DNS 恢复，无残留。

## 7. 存储

- 规则与配置**一律密文落盘**：Rust 侧经 `crypto::encrypt_str`（`config.rs` 的加密信封边界），前端经 `safeStorage`；展示时解密。不新增明文落盘路径。

## 8. 测试与验收

### 8.1 自动化

- 纯逻辑单测（主力）：`rule_engine`（既有 23 条）、`fakedns`（既有 8 条）；本期对 `dns_srv` 解析/分配、规则校验、路由计划等可纯测的部分补单测。
- `cargo test` 绿；`pnpm build`（`vue-tsc --noEmit && vite build`）通过。

> 注：当前 `cargo test` 存在 4 个**与本次改动无关**的历史/环境失败（`apps::registry` 3 个 Windows 路径断言、`vpn::detect` 1 个），验收时会单独标注为已知项。

### 8.2 macOS 手工 e2e 清单

- 按**域名**、**真 IP**、**进程**、**目标端口**分别验证出口正确。
- `DOMAIN-SUFFIX` 不误命中 `notclient.com`。
- fake-ip 正常分配与回收；`fake_ip_filter` 内域名走真解析。
- 非 DNS 的 UDP（如 QUIC）按规则分流。
- 断开 / 异常退出后，系统路由与 DNS **完全恢复**。
- **强杀引擎**后看门狗撤路由，网络不黑洞。
- UI：规则可拖拽排序、`MATCH` 行不可动、按 kind 校验生效。

### 8.3 本期验收对应总设计第 12 节的子集

`规则顺序/首命中`、`七类匹配`、`后缀边界`、`fake-ip 分配回收与 filter`、`UDP 按规则`、`恢复无残留`、`杀引擎不黑洞`、`密文落盘`、**`mode` 全部移除无编译残留**、`cargo test` 无新增失败、`pnpm build` 通过。**`RULE-SET` 与 Windows 两项本期不做**。

## 9. 风险与已知问题

- **自研用户态栈仍是最大风险**（总设计 §13）：本期靠 `smoltcp`（S1）显著降低，但 TCP 半关闭、UDP 映射回收、MTU/MSS、并发仍是重点验证对象。
- **两个 TUN 并存**（总设计 §13）：靠"openconnect 不装路由 + 引擎自身 socket 钉接口 + VPN 服务器 host 路由"三保证解决；e2e 必须专门验证不死循环、不断网。
- **fake-ip 固有限制**：DoH/DoT 应用的域名拿不到，只能退回按 IP/进程匹配（写入 UI 提示）。
- **`PROCESS-NAME` 依赖 `sysctl` 与 root**：权限/系统版本变化需专门验证。
- **按流 `/32` 路由增删**：为降开销用引用计数缓存；看门狗必须保证异常时撤干净。

## 10. 实施顺序（供 writing-plans）

1. `platform.rs` trait + `tun_device.rs`（macOS）。
2. `netstack.rs`（smoltcp，TCP 终结）。
3. `router.rs`（`DIRECT` 出口 + TCP 双向搬运）。
4. `script.rs` vpnc-script 退化（不装路由）。
5. `worker.rs`/`session.rs` 引擎装配 + 看门狗 + 路由回收。
6. `dns_srv.rs` 服务化（`UDP/TCP:53`）。
7. `proc_lookup.rs`（`sysctl`）。
8. 规则编译接入 + `MatchInput` 组装 + `default_policy`。
9. UDP 五元组映射转发并套用规则。
10. 前端类型 / `useVpn` / `VpnRuleTable` 重写 / `VpnProfileForm` / `App.vue` / 组件适配。
11. 移除 `mode`（Rust 类型/配置 + 前端 + 托盘）。
12. e2e 手工验收 + 回归。
