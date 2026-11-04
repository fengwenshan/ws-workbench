# Clash 式规则分流（TUN 引擎）设计

- 日期：2026-09-30
- 状态：待评审
- 关联：`2026-09-29-local-storage-encryption-design.md`（落盘加密信封）、现有 `vpn/` 模块（openconnect 隧道管理）

## 1. 背景与目标

现状分流模型是「两个模式（`customer_first` / `company_first`）+ 一张 include/exclude 的 CIDR 网段表」，实现手段是**系统路由表**：谁抢默认路由，谁就是主出口。这个模型有两个根本限制：

1. 路由表**只看目标 IP 网段**，看不到域名、进程、端口；
2. 「主出口」是一个全局开关，无法表达「A 走 VPN、B 走本地、C 走 VPN」这种逐条规则。

目标：把分流从「路由表 + 模式」升级为 **Clash 式规则引擎**——有序规则、自上而下首个命中生效、末尾 `MATCH` 兜底，支持按 **域名 / 进程 / 端口 / IP 网段** 匹配，并支持导入外部规则集文件（`RULE-SET`）。默认出口由用户自己写的 `MATCH` 决定。

### 范围

**纳入**

- 新的流量接管层（TUN + 用户态 TCP/IP 栈）与规则引擎，**macOS 先行**，Windows 后续。
- 匹配类型：`DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`PROCESS-NAME`、`DST-PORT`、`IP-CIDR`、`RULE-SET`、`MATCH`。
- 出口：`DIRECT`、`VPN`。
- fake-ip DNS。
- 全量 UDP 转发。
- 配置 / 存储 / UI 改造，移除 `mode` 概念。

**不纳入**

- `REJECT` 出口、代理组、`PROCESS-PATH`、`GEOIP`、`SRC-IP-CIDR`。
- 接管 IPv6。
- Windows 平台实现（本期仅设计接口，不实现）。

## 2. 决策摘要

以下决策已经确认，是本设计的约束：

| # | 决策 | 取舍说明 |
| --- | --- | --- |
| D1 | 要做**真正的**域名/进程/端口匹配，不只是规则形式像 Clash | 必须有流量接管层，路由表不够 |
| D2 | 接管方式：**TUN 透明接管**（类 Clash TUN 模式） | 全覆盖、最接近 Clash；需自研用户态网络栈 |
| D3 | 规则模型：**纯 Clash 语义**，去掉 `customer_first`/`company_first` | 默认出口完全由末尾 `MATCH` 决定 |
| D4 | 额外能力：仅增加 **外部规则集文件（RULE-SET）** | 不加 `REJECT`、不加进程路径匹配 |
| D5 | DNS 策略：**fake-ip** | 域名匹配最准，与 CDN/泛域名友好；DoH/DoT 拿不到域名 |
| D6 | 范围节奏：**先 macOS，再 Windows** | 共享核心一套，平台层逐个接 |
| D7 | 实现路线：**全自研**（纯 Rust，单进程） | 许可证干净、完全可控；工作量最大 |
| D8 | UDP：**全量转发** | 规则对全部流量生效；需实现 UDP 映射与回收 |

## 3. 总体架构

一句话：**openconnect 只负责把隧道拉起来（提供 utun 与 VPN IP），路由决策权全部交给新的 TUN 引擎**；引擎按规则决定每条连接走 `DIRECT` 还是 `VPN`。

```
应用
 │  (半默认路由被指向 TUN)
 ▼
[捕获层] utun  →  用户态 TCP/IP 栈 (smoltcp / netstack 封装)
 │
 ├─ UDP:53 ─────────► [fake-ip DNS 解析器]  分配 198.18.x.x、记录 假IP↔域名
 │
 ▼
[规则引擎]  按「假IP→域名」/「真 IP」/「源进程」自上而下匹配
 │
 ├── DIRECT ─► 出站在物理网卡（IP_BOUND_IF 钉接口）
 └── VPN ────► 出站在 utun（IP_BOUND_IF 钉接口）
```

### 三个关键机制

1. **自环绕过**：引擎自己发起的 socket 用 `IP_BOUND_IF` 钉死出口接口。钉了接口即绕过路由表，不会被自己的 TUN 再次捕获。`DIRECT` 与 `VPN` 两个出口**统一**用此方式，不需要绑源 IP。
2. **路由接管**：用 `0.0.0.0/1` + `128.0.0.0/1` 两条半默认路由指向 utun（不动系统默认路由，回收时干净）；同时为 **VPN 服务器 IP 加一条走物理网关的 host 路由**，否则 openconnect 自身流量会绕回 TUN 形成死循环。
3. **进程反查**：macOS 上用 `sysctl` 的 `net.inet.tcp/udp.pcblist_n` 把「连接的本地端点」反查成 PID，再取进程名（需要 root，worker 本来就提权）。

### 组件边界

每个组件都是可独立理解的单元，通过明确接口通信：

| 组件 | 职责 | 依赖 |
| --- | --- | --- |
| `tun_device` | 建/销毁 utun，读写 IP 包 | macOS utun 控制 socket |
| `netstack` | 用户态 TCP/IP 状态机；吐「连接事件 + 数据流」，接「回写数据」 | smoltcp 之类 crate |
| `fakedns` | 拦截 DNS，分配/回收假 IP，维护双向映射与 filter | netstack |
| `rule_engine` | 解析 + 匹配规则（输入：域名/IP/端口/进程；输出：`DIRECT`/`VPN`） | 无（纯逻辑，最易测） |
| `router` | 按匹配结果建出站连接、钉接口、双向搬运（TCP 与 UDP 各一套） | netstack + 平台层 |
| `proc_lookup` | 连接 → PID → 进程名 | sysctl（平台层） |
| `session` | 编排：提权 → 起隧道 → 建 TUN/路由 → 起引擎；停止反序清理 | 以上全部 |

### 与现有代码的关系

- `script.rs` 中「按 mode 抢/不抢默认路由 + include/exclude `add_route`」整体**废弃**；openconnect 的 vpnc-script 退化为「**不装任何路由**」，仅保留隧道接口 IP/MTU 相关处理。
- `worker.rs` 的 `include_rules` / `exclude_rules` / `local_subnets` 路由逻辑被 TUN 引擎取代。
- `VpnMode`（`customer_first` / `company_first`）从 Rust 类型、前端类型、配置、UI 中**全部移除**。

## 4. 规则模型与匹配语义

规则是**有序列表**，**自上而下首个命中生效**，末尾必须有兜底默认出口。等价于 Clash 的：

```
DOMAIN-SUFFIX, client.com, VPN
DOMAIN-KEYWORD, gitlab,    VPN
PROCESS-NAME,  Idea,       VPN
DST-PORT,      22,         VPN
IP-CIDR,       10.0.0.0/8, VPN
RULE-SET,      ./rules/xxx.yaml, VPN
MATCH,                     DIRECT
```

### 语义定义

- **顺序即优先级**：逐条比较「这条规则的条件是否成立」，成立即采用其出口并停止。**不是**最长前缀匹配。
- **`MATCH`**：不在规则数组里，用独立字段 `default_policy: DIRECT | VPN` 表示；必须存在。UI 上钉死在最后一行、不可删除、不可排序。
- **每类规则的匹配含义**：
  - `DOMAIN`：目标域名**完全相等**。
  - `DOMAIN-SUFFIX`：目标域名以该串结尾（`client.com` 命中 `a.client.com`，不命中 `notclient.com`）。
  - `DOMAIN-KEYWORD`：目标域名**包含**该串。
  - `PROCESS-NAME`：源进程名相等（大小写不敏感）。
  - `DST-PORT`：目标端口相等；值为单端口或 `start-end` 区间。
  - `IP-CIDR`：目标 IP 落在该 CIDR 内（IPv4）。
  - `RULE-SET`：引用一个已导入的规则集文件，等价于把该文件的条目**就地展开**到这条规则的位置逐条比对。
- **`no-resolve`**：仅对 `IP-CIDR` 有意义，字段默认值 `true`，即默认**不触发 DNS 解析**（避免无谓解析与泄漏）；需要「先按域名解析、再匹配 IP」的极端场景时，用户显式把该规则置为 `false`（对应 Clash 的 `no-resolve` 参数）。其余规则类型该字段恒为 `true` 且不生效；域名类规则的匹配依赖第 5 节的「假 IP → 域名」映射，不额外解析。
- **`RULE-SET` 文件格式**：
  - `.yaml`：Clash 风格，`payload:` 为字符串数组，每项形如 `DOMAIN-SUFFIX,xxx` / `IP-CIDR,10.0.0.0/8`。
  - `.list`：纯文本，一行一条，格式同上。
  - 文件在**启动时**加载并展开为内存规则；`kind: domain` 与 `kind: ipcidr` 两类分别处理。解析失败即整体拒绝启动（见第 9 节）。

## 5. fake-ip DNS 子系统

因为默认路由指向 TUN，应用的 DNS 查询天然落到我们手里，**不需要**额外的 PF/防火墙劫持。

- **拦截范围**：TUN 上收到的 `UDP:53` 与 `TCP:53`。
- **假 IP 池**：默认 `198.18.0.0/16`，可配置 `fake_ip_range`。DNS 应答时分配假 IP，建立 `假IP → 域名`；连接到达 TUN 时反查域名送入规则引擎。
- **生命周期**：按 DNS TTL + LRU 回收，池满淘汰最旧条目；**断开重连时清空整池**。
- **`fake_ip_filter`**：一组「不伪造、要真解析」的域名（支持后缀/关键词，如 `*.lan`、`*.local`）。命中的域名走真解析，其连接按真 IP 交给 `IP-CIDR` 或默认出口。这是本地服务/特殊应用的兼容性关键。
- **真解析走哪个 DNS**：`DIRECT` 出口用物理网的 DNS；`VPN` 出口在 `use_vpn_dns` 为真时用隧道 DNS。上游 DNS 服务器可配，默认用系统。
- **IPv6**：本期**不接管 IPv6**。TUN 只处理 IPv4，对 AAAA 查询返回空，逼应用走 IPv4。理由：双栈 + fake-ip v6 会让一期复杂度翻倍，先把 IPv4 打穿。
- **已知边界**：DoH/DoT（走 443/853）不被拦截，这类应用的域名拿不到，只能退回按 IP/进程匹配。此边界写入文档与 UI 提示。

## 6. 数据面

### TCP

netstack 终结应用侧连接 → 判定目标（假 IP → 域名；真 IP → 用真 IP，并用 `proc_lookup` 拿进程）→ 规则匹配 → 在出口钉接口建连 → 双向搬运。要点：半关闭、连接失败/超时、MSS/MTU 处理。

### DNS（UDP:53）

由 `fakedns` 接管，不出 TUN。

### 其余 UDP（QUIC 443、游戏、自定义 UDP）

- **全量转发**：为每个 UDP 流建出站 socket，按五元组做映射，**钉出口接口**，双向转发，带空闲超时回收。
- QUIC 的场景：客户端连的是**假 IP**，域名已知，规则照常命中；出站把 UDP 报文发到真实目标地址。
- ICMP 直通物理网卡。
- loopback 不接管（走 `lo`，天然不被抓）。

## 7. 平台层（macOS 一期）

- **TUN**：经 `PF_SYSTEM` / `SYSPROTO_CONTROL` / `UTUN_CONTROL_NAME` 建 utun，设点对点地址与 MTU。
- **路由**：`0.0.0.0/1` + `128.0.0.0/1` 半默认路由指向 utun；VPN 服务器 IP 加 host 路由走物理网关；退出时逐条回收。
- **出口钉接口**：`IP_BOUND_IF`（level 25）——`DIRECT` → 物理网卡，`VPN` → utun；引擎自身 socket 同样钉死。
- **进程反查**：`sysctl net.inet.tcp.pcblist_n` / `net.inet.udp.pcblist_n` 反查本地端点 → PID → 进程名；需 root。
- **提权与生命周期**：复用现有提权流程（`ElevationInfo`）。TUN 引擎跑在**已提权的 worker 进程**内，`session` 编排：提权 → 起 openconnect（不装路由）→ 等隧道就绪 → 建 TUN + 路由 → 起引擎；停止时反序清理。
- **平台抽象**：TUN 设备、路由操作、接口钉定、进程反查四件事以 trait 抽象，macOS 先实现，Windows 后续补实现。

## 8. 配置、存储与 UI 改造

### 类型（`src/types/vpn.ts` 及 Rust 侧镜像）

- 删除 `VpnMode`、`MODE_LABELS`、`MODE_DESCRIPTIONS`。
- `VpnRule`：`{id, cidr, remark, enabled}` → `{id, kind, value, policy, no_resolve, remark, enabled}`；`kind ∈ DOMAIN | DOMAIN-SUFFIX | DOMAIN-KEYWORD | PROCESS-NAME | DST-PORT | IP-CIDR | RULE-SET`，`policy ∈ DIRECT | VPN`。
- `VpnRuleSet`：`rules` 变为**有序**数组；新增 `default_policy: DIRECT | VPN`（对应 `MATCH`）；新增 `rule_sets: [{id, name, path, kind: domain | ipcidr}]`。
- `VpnProfile`：去掉 `mode`；新增 `fake_ip_range`、`fake_ip_filter: string[]`、`enable_ipv6: boolean`（固定 `false`，仅占位）；保留 `use_vpn_dns`。

### 存储

按项目规范「本地存储一律加密」：规则内容（域名等）与导入的规则集文件副本**全部密文落盘**——Rust 侧经 `crypto::encrypt_str` / `decrypt_str` 信封，前端经 `safeStorage`。导入的规则集文件存到 app 数据目录的密文副本，加载时在内存解密。

### UI

- `VpnRuleTable.vue` 重写为 Clash 式规则编辑器：有序行、**拖拽排序**、类型下拉 + 值输入 + 出口下拉 + `no-resolve` 开关 + 启停 + 删除；底部钉一行 `MATCH` 选择默认出口；`RULE-SET` 文件导入/管理与条目计数。
- 移除模式选择器；新增 DNS 设置区（fake-ip 段、filter、IPv6 开关）。
- 展示 DoH/DoT 域名匹配不到的提示。

## 9. 错误处理与降级

- **规则解析失败 → 拒绝启动**（fail-closed，绝不「猜一个出口」）。
- **TUN / 路由建立失败** → 中断连接、回收已加路由、明确报错。
- **引擎崩溃 = 网络黑洞（最危险）**：路由回收必须与引擎存活绑定——worker 看门狗发现引擎退出，立即撤路由、清 DNS 映射，恢复原网络。
- 假 IP 池耗尽 → 淘汰 LRU 最旧条目。
- `DIRECT` 域名解析失败 → 把失败返回给应用，不静默改写出口。
- 停止 / 异常 / 用户取消，一律保证路由与 DNS 恢复。

## 10. 测试策略

- **纯逻辑层（主力，`cargo test` 全覆盖）**：
  - `rule_engine`：顺序与首命中、`MATCH` 兜底、各匹配类型（域名相等/后缀/关键词、进程名、端口含区间、`IP-CIDR`）、`RULE-SET` 展开、`no-resolve` 行为。
  - `fakedns`：分配、回收/淘汰、`fake_ip_filter` 命中。
- **运行时层（macOS 手工 e2e 清单）**：按域名 / 按 IP / 按进程 / QUIC 分别验证出口；断开后路由与 DNS 完全恢复；引擎被杀死后网络不黑洞。

## 11. 分期与实施顺序

| 期 | 内容 | 可验证产出 |
| --- | --- | --- |
| 1 | 纯逻辑层：`rule_engine` + `fakedns` + 单测 | `cargo test` 绿 |
| 2 | macOS 运行时：`tun_device` / `netstack` / `router` / `proc_lookup` + `session`，**TCP 打穿** | 按域名 / IP / 进程能正确分流 |
| 3 | UDP 全量转发 + `RULE-SET` 文件 | QUIC / UDP 也受规则约束 |
| 4 | 配置 / 存储 / UI 重写 + 移除 `mode` | 用户可图形化编辑规则、端到端可用 |
| 5 | Windows 平台层 | 两端一致 |

## 12. 验收标准

- [ ] 规则按书写顺序、首个命中生效；末尾 `MATCH` 决定默认出口。
- [ ] `DOMAIN` / `DOMAIN-SUFFIX` / `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `IP-CIDR` / `RULE-SET` 均能正确分流。
- [ ] `DOMAIN-SUFFIX` 不误命中 `notclient.com` 这类边界。
- [ ] `RULE-SET` 支持 `.yaml`（`payload:` 数组）与 `.list`，条目计数正确，解析失败拒绝启动。
- [ ] fake-ip 正常分配与回收；`fake_ip_filter` 内的域名走真解析。
- [ ] QUIC / 任意 UDP 也能按规则分流（全量转发）。
- [ ] 断开 / 异常退出后，系统路由与 DNS 完全恢复，无残留。
- [ ] 引擎被强制杀死时，worker 看门狗撤路由，网络不黑洞。
- [ ] 规则与规则集文件在磁盘上为密文；界面展示明文。
- [ ] `mode` 相关类型、配置、UI 入口全部移除，无编译残留。
- [ ] `cargo test` 全绿（含 `rule_engine`、`fakedns` 新增单测）。
- [ ] `pnpm build`（`vue-tsc --noEmit && vite build`）通过。

## 13. 风险与已知问题

- **自研用户态网络栈**是最大风险点：TCP 状态机对接、UDP 映射与回收、MTU/MSS、并发与资源回收都容易翻车。故第 1、2 期先把纯逻辑与 TCP 打穿，再上 UDP。
- **两个 TUN 并存**：openconnect 隧道本身是一个 utun，捕获层是另一个。必须确保 openconnect 侧不装路由，且引擎自身流量被钉接口绕过捕获。
- **fake-ip 固有边界**：DoH/DoT 应用的域名拿不到；少数自行按 IP 直连的应用无法按域名匹配。
- **无 `REJECT`**：本设计只有 `DIRECT` / `VPN` 两个出口，不做拦截。
- **`PROCESS-NAME` 依赖平台接口与 root**：进程反查在 macOS 走 `sysctl`，权限或系统版本变化时需专门验证。
- **许可证路线**：选择全自研（D7）以避免引入 GPL 内核，代价是工作量最大。
