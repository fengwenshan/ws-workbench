# VPN 严格规则模式（服务端下发范围只展示、不生效）

- 日期：2026-11-10
- 范围：`src-tauri/src/vpn`（worker / split / models）与 VPN 面板展示；不涉及服务端与域名配置
- 现场：Windows 客户端规则列表 0 条，仍能访问 `https://oa.example.com:9508/...`；面板「当前生效的分流」只有 `223.5.5.5/32 走 VPN`

## 1. 结论

规则模式的**唯一**分流依据是用户规则。服务端通过 AnyConnect 下发的
`CISCO_DEF_DOMAIN`（现场为 `example.com`）与 `CISCO_SPLIT_INC_*` 网段
**不再自动并入分流计划**，只作为「服务端声明的内网范围」记录进
`status.server_scope`，在面板与日志里展示，供排查时对照。

规则为空时：隧道照常建立，但一条流量都不接管（`SplitTunnel::apply`
早退 → `degraded_reason` → 状态为「已连接（未接管系统流量：……）」）。

## 2. 背景与根因

现场「规则空着也能进内网」不是残留路由、不是全局模式、也不是规则文件未清：

- 连接时 `session.prepare()` 会删掉 `rules.json`，worker 的规则来自 `VpnRequest`；
- 关闭/崩溃残留由 worker 启动时的 `cleanup_stale` 与 `teardown` 覆盖；
- 真正原因在 `worker.rs`：解析脚本回报的 `defdomain=` / `split=` 后，把这些值
  无条件 `plan.add_domain()` / `plan.add_cidr()` 进了分流计划 —— 这是当初刻意
  做的「服务端范围自动并入」，但界面上完全看不出来，于是表现为隐式行为。

另外 `oa.example.com` 在公网 DNS 上就有一条 RFC1918 的 A 记录
（`192.0.2.41`），DNS 解析不需要 VPN；能不能访问只取决于是否注入了
`/32` 隧道路由。所以「规则空却通内网」只能来自客户端自己装的路由。

## 3. 方案选择

| 方案 | 行为 | 结论 |
| --- | --- | --- |
| A 严格 | 服务端范围完全不并入，规则是唯一依据 | **采用** |
| B 展示后可点「一键采纳」 | 需要额外交互与规则生成 | 否（用户明确不要一键生成） |
| C 保留并入但面板明示 | 隐式行为仍在，只是可解释 | 否 |
| D 并入时弹窗询问 | 连接过程被打断 | 否（用户明确不要弹窗） |

配套决定：不弹窗、不做迁移引导、不加「一键生成规则」；服务端范围**可见**
（面板 + 日志），但恒不生效。

## 4. 实现位置

- `src-tauri/src/vpn/models.rs`：新增 `ServerScope { domains, cidrs }`；
  `VpnStatus.server_scope`（`serde(default)`，老 `status.json` 照常读）。
- `src-tauri/src/vpn/worker.rs`：`defdomain` / `split` 标记只归一化后写入
  `status.server_scope`，并各打一行
  `[分流] 服务端下发……：仅记录展示，不自动并入分流`；
  新增 `rule_plan()`，`start_split_rules` 只从用户规则生成计划；
  删除 `server_def_domain` / `server_split_routes` 字段与并入循环。
- `src-tauri/src/vpn/split.rs`：删除已无调用方的 `SplitPlan::add_domain` /
  `add_cidr` / `add_unique` 及对应测试；模块文档明确服务端范围不注入。
- 前端：`src/types/vpn.ts` 增加 `ServerScope`；`VpnRoutePreview.vue` 在
  「当前生效的分流」里展示「服务端下发域 / 服务端下发网段」并注明仅展示；
  规则弹窗空态、模式说明、`README.md` 同步改为严格语义。

## 5. 与 Windows `DOMAIN-SUFFIX` 子域的历史关系

现场曾出现过：Windows 上只写 `DOMAIN-SUFFIX example.com`，访问
`oa.example.com` / `cas.example.com` / `api.example.com` 不通，macOS 正常。
根因（见 `2026-11-06-vpn-windows-feasibility-evidence.md` 第 14.15 节）是 NRPT
命名空间没有前导点，只做精确匹配，子域查询根本没进 loopback 转发器。
修复（`a416fbf`，2026-11-06 19:13，已进 v1.0.16+）：

1. `win_dns::build_apply_script` 同时写 `example.com` 与 `.example.com` 两个命名空间；
2. NRPT 指向 `127.0.0.1:53` 本机转发器，转发器在回包**之前**注入 `/32`，
   子域与精确域名走同一条路；
3. 写完后用随机子域探测「查询是否真的进了转发器」；探测失败则回退为
   「NRPT 直连 VPN DNS + 每 3 秒扫系统 DNS 缓存补 `/32`」，日志会写明回退原因，
   回退路径下子域的首个请求可能失败一次，路由补齐后重试即通。

因此严格规则模式不会重新引入该问题：它只改「计划来源」，不碰 Windows 的
NRPT / 转发器 / 路由注入路径。现场验收仍需在 Windows 上按第 6 节跑一遍。

## 6. 验收矩阵（Windows + macOS 各跑一遍）

| 场景 | 期望 |
| --- | --- |
| 规则为空 | 无 NRPT / resolver / `/32`；`Find-NetRoute 192.0.2.41` 走本地 WLAN；OA 超时 |
| 空规则连上后**再加**规则（不重连） | 200ms 内热更新：日志「正在重新下发路由注入分流」→「分流已生效」；`oa/cas` 立即可访问 |
| `DOMAIN-SUFFIX example.com` | `oa/cas/api.example.com` 均通；日志有「NRPT 已写好 N 条（域名 + 前导点子域后缀）」 |
| `DOMAIN-SUFFIX example.com → 走 VPN` + `DOMAIN hlwapi.example.com → 直连` | hlwapi 不进隧道（无 `/32`、`Find-NetRoute` 走本地 WLAN），`oa/cas` 仍走 VPN；日志有「直连例外：hlwapi.example.com」；预览 hlwapi 显示 DIRECT |
| 老配置里留着 `IP-CIDR` | 不生效：该类型已下线，列表标「不生效」，连接时跳过 |
| 已连接时删掉规则 | 规则热更新后 `/32` 与 NRPT 被回收，内网随之中断 |
| 强杀界面 / 重启 | 无残留 NRPT、路由、resolver 文件 |
| 服务端下发 `0.0.0.0/0` | 只记录展示，不接管全隧道；日志有「仅展示，不自动并入」 |

> 2026-11-10 macOS 现场回归：空规则连上后新增 `DOMAIN-SUFFIX example.com`，内网打不开、
> 断开重连才好。根因是 `poll_rules_change` 用 `self.split.is_some()` 当判据：空规则时
> `split` 恒为 `None`，新增规则被误判成「切回规则模式后生效」直接跳过。已改为按
> 「是否规则模式 + 连接是否收尾（phase=Connected）」决定，并补两条回归用例
> （`adding_rules_after_an_empty_rule_connect_reapplies_split`、
> `rules_change_during_connect_waits_for_wiring`）。

## 7. 直连例外（后缀里摘出某个子域）

现场诉求：`DOMAIN-SUFFIX example.com → 走 VPN` 之后，不想让
`https://hlwapi.example.com:1443/...` 走 VPN（它解析到公网 `203.0.113.11`，进隧道反而不可达）。

语义：同一域名下取**最具体**的命中规则——`pattern` 越长越具体，长度相同时
`DOMAIN` 比 `DOMAIN-SUFFIX` 具体，完全相同则先出现的规则胜出（`split::match_score` /
`split::tunnels_domain`）。所以：

```
DOMAIN-SUFFIX example.com        → 走 VPN
DOMAIN        hlwapi.example.com → 直连     # 例外
```

覆盖范围：`DOMAIN` 例外只作用于该名字本身；`DOMAIN-SUFFIX` 例外作用于整棵子树。

数据面同一条判定（`SplitPlan::tunnels`）：

1. DNS 分流照旧：例外在父级后缀的 resolver / NRPT 范围里，查询仍由本机转发器代答
   （答案来自 VPN DNS）；
2. 转发器回包前（Windows 还有「系统 DNS 缓存扫描」那条腿）先判 `tunnels(qname)`，
   直连的名字**不注入 `/32`**，流量因此走默认路由直连；
3. 主动解析与自检只覆盖 `tunnels(pattern) == true` 的域名；
4. `vpn_preview_rules` 用同一套判定，预览会如实显示 DIRECT。

### 7.1 热更新为什么还要一份「DNS 答案账本」

`/32` 只在转发器**看到 DNS 应答**时才注入。浏览器和系统解析器都会缓存答案，
所以「把某个名字从直连例外改回走 VPN」时，应用很可能不再发查询——等不到注入
时机，表现就是「规则改了没生效，断开重连才好」（现场回归：删掉
`hlwapi.example.com 直连` 后仍然直连，重连后才打不开）。

修法：转发器把每条应答的 `查询名 -> A 记录` 记进 `DnsMemo`（本会话内存，含例外
名字，上限 1024 条，不落盘），`SplitTunnel::apply` 在重新布线时用
`remembered_injections()` 把「按新规则该走隧道」的地址补注入 `/32`。于是
两个方向都即时生效：

- 改成直连：teardown 回收旧 `/32`，不需要新查询；
- 改成走 VPN：按账本补注入，不需要新查询。

补注入同样过 `injectable()`（服务器自身 IP、不可路由地址一律跳过）。

Windows 回退路径（NRPT 直连 VPN DNS + 后台解析）没有转发器，账本只能由
`scan_system_dns_cache()` 在扫系统 DNS Client 缓存时补记（`parse_dns_cache()` 只做解析、
不做规则过滤，过滤统一在调用点走 `tunnels_domain()`，避免两条路径判定不一致）。
这一步是必需的：热更新的 teardown 会 `Clear-DnsClientCache`，缓存被清空后如果只靠
「等应用重新查询」，删掉直连例外就退化成原来的「要重连」。记账时用
`covered_by_rules()` 筛掉与规则无关的缓存条目，避免公网域名的记录挤占 1024 条上限。

已知限制（写进 README 与规则弹窗提示）：

- 端口不参与判定，`1443` 这类没法单独绕开；例外是「域名 / 网段」粒度；
- 路由是 IP 粒度：若例外域名和某个必须走 VPN 的服务解析到同一 IP，或该 IP 被
  `IP-CIDR` 圈进去——但该类型已下线，现在只能用域名例外；
- 网段规则（`IP-CIDR`）已下线：整段注入做不到按网段排除，半生效不如不做；
- 账本只覆盖「本会话经本机转发器解析过」的名字：新加的后缀规则要命中一个
  从没解析过的子域，仍然要等应用下一次解析（清 DNS 缓存/重开页面即可）。
