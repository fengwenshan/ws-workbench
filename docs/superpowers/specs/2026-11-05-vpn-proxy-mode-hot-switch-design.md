# VPN 代理模式热切换 + 退出清理 设计

- 日期：2026-11-05
- 状态：待评审
- 范围：`src-tauri/src/vpn/*`、`src/components/vpn/VpnPanel.vue`、`src/composables/useVpn.ts`、`src-tauri/src/lib.rs`

## 1. 背景

当前代理模式（规则 / 全局 / 直连）是**连接时**写死进连接请求的：`resolve_proxy_plan` 会把模式翻译成"规则集 + 兜底策略"一并落盘给 worker，之后主进程与 worker 之间没有任何运行期控制通道。因此界面在连接后必须锁死模式按钮，用户要换模式只能先断开、再重连——而重连要走提权、启 openconnect、等隧道、下发路由，很慢。

## 2. 目标

1. VPN 已连接时，点击模式按钮**立即生效**，隧道、网卡、系统路由表、worker 进程都不重建（不重连）。
2. 模式可以反复来回切，始终复用同一条隧道。
3. 模式切换后：**引擎侧已建立的连接被主动重置**，应用会立刻按新模式重连；新连接一律按新模式走。
4. 全局 / 直连模式下规则集**完全不参与**（不编译、不校验、不匹配）。
5. VPN 断开、应用退出（含 Cmd+Q / 菜单退出 / 强制退出）后，不残留任何路由、进程、控制文件。

## 3. 非目标

- 规则列表本身的运行期热重载（连接期间改规则仍需重连才生效）。
- 把直连侧已存在的连接迁移进隧道（见第 4 节约束）。
- 在 status 里回显"已生效模式"（界面以配置里的 `proxy_mode` 为准）。

## 4. 关键事实与约束

### 4.1 Clash 的做法

Clash 是本地代理：应用的连接先到 Clash（HTTP/SOCKS 监听或 TUN 抓包），由 Clash 决定上游出口并自己 dial。模式只是规则引擎里的一个运行期开关，在新连接建立时被读取。所以 Clash 切模式**不重启、不重连，已建立的连接保持原出口走到结束**——它本身并不迁移存量连接。

本设计的隧道复用、运行期改判与 Clash 一致；差异是本项目额外把引擎侧已建立的连接重置掉，让它们立刻按新模式重建。

### 4.2 本项目的物理约束

- **VPN 侧连接**由引擎在用户态终结（smoltcp 收包 + 自己发上游 socket），引擎可以主动 RST 并回收，因此可以立即"迁移"。
- **直连侧连接**是应用自己的 socket，创建时就被 `IP_BOUND_IF` 绑死在物理网卡上（`router.rs` 的 `bind_fd_to_if`），数据包不进 TUN。内核不允许修改已 bind 的 socket 出口，所以这部分既不可见也不可控，只能等应用自己结束。

结论：`VPN → 任意模式` 立即全部迁移；`直连 → 全局` 时，已存在的直连连接会继续直连到自然结束。界面文案需要如实说明。

## 5. 总体数据流

```
连接：UI 选好的模式 + 规则集 ──▶ vpn_connect
                              └─ resolve_proxy_plan(mode, rules, default_policy)
                                 └─▶ request{ proxy_mode, rules(全量), default_policy }
                                     └─▶ worker：只有 rule 模式才编译规则，装配引擎

切换：UI 点模式 ──▶ 持久化 profile.proxy_mode（配置存储）
               └─▶ vpn_set_proxy_mode(mode) ──▶ 写 <session>/mode（一行文本）

worker 每 200ms：读 <session>/mode
               ├─ 变化为 rule：先编译规则（失败 → 记日志 + 回写原模式，不切）
               └─ 引擎在跑：engine.set_mode(mode)
                             ├─ 改判（后续新流按新模式）
                             └─ reset_flows()：TCP 全部 RST + 回收 /32 路由，UDP 回收
```

## 6. 详细设计

### 6.1 连接请求

`VpnRequest`（`models.rs`）新增 `proxy_mode: String`。

`resolve_proxy_plan`（`commands.rs`）改为返回 `ProxyPlan { mode: ProxyMode, rules: Vec<VpnRule>, default_policy: String }`：

- 解析模式，非法值直接报错（保留现有错误文案）。
- 规则**始终全量下发**（过滤 `enabled == false`），因为切回规则模式时 worker 手里必须有规则；全局 / 直连模式只是不使用它。
- 编译校验只在 `rule` 模式做：失败 → 连接失败（fail-closed，保持现状）。全局 / 直连模式**完全不编译、不校验**（用户明确要求"规则集直接忽略"）。

### 6.2 引擎运行期改判（`router.rs`）

新增：

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProxyMode { Rule, Global, Direct }
impl ProxyMode { pub fn parse(&str) -> Result<Self, String>; pub fn as_str(&self) -> &'static str }

/// 纯函数，便于单测：全局强制 VPN、直连强制 DIRECT、规则模式保持规则判定结果
pub fn apply_mode(mode: ProxyMode, egress: Egress) -> Egress
```

`Engine` 字段变化：

- `mode: ProxyMode`
- `rules: Option<CompiledRuleSet>`（`Direct`/`Global` 且未切过规则模式时为 `None`）

`Engine::assemble(...)` 增加 `mode: ProxyMode`、`rules: Option<CompiledRuleSet>` 参数。

新增方法：

- `pub fn set_mode(&mut self, mode: ProxyMode)`：模式未变直接返回；变了先改字段，再 `reset_flows()`。
- `pub fn set_rules(&mut self, rules: CompiledRuleSet)`。
- `fn reset_flows(&mut self)`：遍历 `router.flow_keys()`，对每条 TCP 流先 `netstack.abort_tcp(key)`（向应用发 RST）再 `router.close_flow(key)`；遍历 `router.udp_flow_keys()` 逐条 `router.close_flow`。

判定点统一走模式包装（`Rule` 模式下 `rules` 为 `None` 属异常状态，退化为 DIRECT 并记一次日志）：

- TCP 新流：域名判定、仅 IP 判定、解析后真实 IP 复判（3 处）
- UDP 新流：同上（3 处）
- `dns_respond` 为"过滤域名"选 DNS 出口的 policy（1 处）

`Engine::shutdown()` 不动。

### 6.3 控制通道（`session.rs` + `worker.rs`）

- 文件 `<session>/mode`，内容为单行 `rule|global|direct`，复用 `write_atomic` 原子写。
- `VpnSession` 新增：`mode_path()`、`write_mode()`、`read_mode() -> Option<String>`（trim，空串视为 None）、`remove_mode()`。
- `prepare()` 的清理列表加入 `mode_path()`，避免上一次会话的模式串到下一次连接。
- worker 主循环顶部（`poll_learn_clear()` 旁边）新增 `poll_mode_change()`，200ms 节流：
  - 读到值与 `applied_mode` 相同 → 返回。
  - 目标为 `rule` 且尚未编译过规则 → 用现有 `compiled_rules()` 编译；失败则记日志、**保持当前模式**、把 `mode` 文件写回当前模式（不把正常隧道搞断）。
  - 引擎已装配 → `set_rules`（仅切到 rule 时）+ `set_mode`，更新 `applied_mode`，记日志 `[模式] 已切换为全局，已建立的连接会断开重连`。
  - 引擎未装配（隧道还没起来）→ 记为 `pending_mode`，装配完成后立刻应用。
- 启动时按 `request.proxy_mode` 决定是否编译规则：`rule` 才编译（失败照旧连接失败），其余模式传 `None`。

### 6.4 命令与前端

- 新增命令 `vpn_set_proxy_mode(mode: String)`：校验取值 → 当前有活动会话（status 非 `Disconnected`/`Failed`）则写 `mode` 文件；否则直接 `Ok`（下次连接按配置生效）。纯文件操作，不需要提权。
- `useVpn.setProxyMode`：先 `updateProfile` 持久化，若 `isConnected` 再调用 `vpn_set_proxy_mode`；日志改为「已切换为 X，已建立的连接会断开重连」。
- `VpnPanel`：`modeLocked` 只在 `connecting || phaseBusy` 时为真（连接中 / 断开中引擎不在稳态）；按钮 `title` 与下方提示文案改为「切换立即生效：已建立的连接会断开重连一次；直连中的已有连接无法接管，会自然结束」。

### 6.5 生命周期与清理

| 时机 | 动作 |
|---|---|
| 连接准备 | `prepare()` 删除旧的 `mode`、`main.alive`、status/log/learn |
| 模式切换 | 覆盖写 `mode`（原子） |
| 断开（用户点断开 / 托盘断开） | worker 正常收尾：关闭所有流 → 还原路由 → 删 `mode`、`request`（含明文密码）→ 写 disconnected |
| 托盘「退出」 | 现有 `shutdown()` 请求 worker 收尾（保留） |
| **Cmd+Q / 菜单退出 / 系统关机** | 新增 `RunEvent::ExitRequested` 钩子：调用同一个 `shutdown()`（幂等），并删除 `mode` / `main.alive` |
| **主进程被强杀** | worker 侧"父进程心跳守卫"：主进程在连接期间每 2s 刷新 `<session>/main.alive` 的 mtime；worker 每 1s 检查一次，超过 8s 未刷新（或文件消失）即认为主进程已死，走与正常停止相同的收尾流程后自行退出 |

心跳线程只在有活动会话时存在（连接建立后启动，检测到 disconnected/failed 后自行退出），不引入常驻后台线程。

## 7. 错误处理

| 场景 | 行为 |
|---|---|
| `mode` 文件写入失败 | 命令返回错误，前端记日志，沿用旧模式 |
| 切到规则模式但规则编译失败 | worker 记日志、保持原模式、回写原模式到 `mode` 文件 |
| worker 未装配引擎时切换 | 记为 pending，装配后应用 |
| 非法模式字符串 | 命令 / 请求解析直接报错 |
| 心跳文件缺失 | 启动后 10s 宽限期内忽略，之后视为父进程已死 |

## 8. 测试计划

单元测试：

- `apply_mode`：Rule 保持原判、Global 强制 VPN、Direct 强制直连。
- `ProxyMode::parse`：合法三值、大小写、非法值报错。
- `resolve_proxy_plan`：三种模式都返回全量（启用中的）规则 + 正确模式；非法模式报错；规则模式校验失败报错、全局/直连不校验。
- `VpnSession`：`mode` 文件写/读、`prepare` 清掉旧文件。
- 现有 worker 测试夹具补 `proxy_mode` 字段。

回归：`cargo test --lib`、`pnpm -s exec vue-tsc --noEmit`、`pnpm -s build`。

手动验收：

1. 连接后切「全局」→ 立即生效，日志出现模式切换，正在进行的请求断开一次后重连；
2. 全局 ↔ 直连 ↔ 规则来回切多次，隧道地址不变、无重连日志；
3. 断开后 `mode` 文件不存在、无残留路由；
4. 托盘退出 / Cmd+Q 后确认 worker 进程已退出、路由已还原；
5. 连接状态下强杀主进程，8s 内 worker 自行收尾退出。

## 9. 影响面与风险

- 改动文件：`models.rs`、`router.rs`、`worker.rs`、`session.rs`、`commands.rs`、`lib.rs`、`useVpn.ts`、`VpnPanel.vue`；约 250 行（含测试与心跳守卫），不新增依赖。
- 风险点：模式切换时重置所有流会让正在进行的下载/上传中断一次——这是用户明确接受的行为，文案里已提示。
- 回滚：全部改动由 `proxy_mode` 字段与 `mode` 文件驱动，文件不存在时行为与现状一致，可按文件回滚。
