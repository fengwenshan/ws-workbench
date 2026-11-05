# VPN 代理模式热切换 + 规则热更新 + 退出清理 设计

- 日期：2026-11-05
- 状态：待评审（第 2 版：模式切换不再重置连接，改为规则变更时定向重置）
- 范围：`src-tauri/src/vpn/*`、`src/components/vpn/VpnPanel.vue`、`src/composables/useVpn.ts`、`src-tauri/src/lib.rs`

## 1. 背景

当前代理模式（规则 / 全局 / 直连）和规则集都是**连接时**写死进连接请求的：`resolve_proxy_plan` 把模式翻译成"规则集 + 兜底策略"落盘给 worker，worker 启动时编译一次存进引擎。之后主进程与 worker 之间没有任何运行期控制通道。因此：

- 连接后必须锁死模式按钮，换模式只能先断开再重连（重连要提权、启 openconnect、等隧道、下发路由，很慢）；
- 连接状态下改规则（增删改、启停、切规则列表、改 MATCH 走向）全都只存盘不生效，同样要重连。

## 2. 目标

1. VPN 已连接时切换代理模式**立即生效**，且**不打断已建立的连接**（它们按建立时的路径走完，只有新连接按新模式）；隧道、网卡、路由表、worker 进程都不重建。
2. 模式可反复来回切，始终复用同一条隧道。
3. 规则变更**热生效**：重新编译规则，并**只重置判定结果发生变化的已建立连接**（其余连接不受影响），新连接按新规则。
4. 全局 / 直连模式下规则集**完全不参与**（不编译、不校验、不匹配）。
5. VPN 断开、应用退出（含 Cmd+Q / 菜单退出 / 强制退出）后不残留任何路由、进程、控制文件。

## 3. 非目标

- 把已建立的直连连接迁移进隧道（见第 4 节物理约束）。
- 规则语法/匹配能力的调整。
- 在 status 里回显"已生效模式"（界面以配置里的 `proxy_mode` 为准）。

## 4. 关键事实与约束

### 4.1 Clash 的做法

Clash 是本地代理：应用的连接先到 Clash（HTTP/SOCKS 监听或 TUN 抓包），由 Clash 决定上游出口并自己 dial。模式只是规则引擎里的一个运行期开关，在新连接建立时被读取。所以 Clash 切模式不重启、不重连，**已建立的连接保持原出口走到结束**——它不迁移存量连接。

本设计的模式切换与 Clash 一致。

### 4.2 本项目的物理约束

- **VPN 侧连接**由引擎在用户态终结（smoltcp 收包 + 自己发上游 socket），引擎能主动终止它。
- **直连侧连接**是应用自己的 socket，创建时就被 `IP_BOUND_IF` 绑死在物理网卡上（`router.rs` 的 `bind_fd_to_if`），数据包不进 TUN。内核不允许修改已 bind 的 socket 出口，因此这类连接既不可见也不可控。

因此：切换模式时所有存量连接都按原路径走完（与 Clash 相同）；规则变更时，凡是判定结果变了的流（不论原出口是 VPN 还是直连）都会被 RST 重置，让应用按新规则重建。

## 5. 总体数据流

```
连接：UI 选好的模式 + 规则集 ──▶ vpn_connect
                              └─ resolve_proxy_plan(mode, rules, default_policy)
                                 └─▶ request{ proxy_mode, rules(全量), default_policy }
                                     └─▶ worker：只有 rule 模式才编译规则，装配引擎

切模式：UI 点模式 ──▶ 持久化 profile.proxy_mode
                 └─▶ vpn_set_proxy_mode(mode) ──▶ 写 <session>/mode
                 worker 每 200ms 读：变了就 engine.set_mode(mode)（只改判，不重置流）

改规则：UI 规则变更 ──▶ 防抖 300ms ──▶ vpn_push_rules(rules, default_policy)
                 └─ 主进程先编译校验（失败直接报错，不落盘）
                 └─▶ 写 <session>/rules.json
                 worker 每 200ms 读：内容变了就重新编译
                     ├─ rule 模式：engine.set_rules() + reset_changed_flows()
                     └─ global/direct：只更新手里保存的规则（不参与判定）
```

## 6. 详细设计

### 6.1 连接请求

`VpnRequest`（`models.rs`）新增 `proxy_mode: String`。

`resolve_proxy_plan`（`commands.rs`）改为返回 `ProxyPlan { mode: ProxyMode, rules: Vec<VpnRule>, default_policy: String }`：

- 解析模式，非法值直接报错（保留现有错误文案）。
- 规则**始终全量下发**（过滤 `enabled == false`），因为切到规则模式时 worker 手里必须有规则；全局 / 直连模式只是不使用它。
- 编译校验只在 `rule` 模式做：失败 → 连接失败（fail-closed，保持现状）。全局 / 直连模式完全不编译、不校验（用户要求"规则集直接忽略"）。

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
- `rules: Option<CompiledRuleSet>`（`global`/`direct` 且未切过规则模式时为 `None`）

`Engine::assemble(...)` 增加 `mode: ProxyMode`、`rules: Option<CompiledRuleSet>` 参数。

新增 / 调整方法：

- `pub fn set_mode(&mut self, mode: ProxyMode)`：只改字段，**不重置任何流**。
- `pub fn set_rules(&mut self, rules: CompiledRuleSet)`。
- `pub fn reset_changed_flows(&mut self)`：对每条已建立的流用新规则重算判定，只重置走向变了的。
- `fn reset_flows(&mut self, keys)`：内部工具，逐条 `netstack.abort_tcp(key)`（向应用发 RST）+ `router.close_flow(key)`。

判定点统一走模式包装（`Rule` 模式下 `rules` 为 `None` 属异常状态，退化为 DIRECT 并记一次日志）：

- TCP 新流：域名判定、仅 IP 判定、解析后真实 IP 复判（3 处）
- UDP 新流：同上（3 处）
- `dns_respond` 为"过滤域名"选 DNS 出口的 policy（1 处）

**定向重置的实现基础**：`FlowEgress` / `UdpEgress` 增加 `egress: Egress`、`target: SocketAddrV4`、`domain: Option<String>`、`process: Option<String>`（建立时的判定输入与结果）。`reset_changed_flows` 用这些输入重跑 `decide_domain_at`，结果与记录的 `egress` 相同则跳过，不同则重置该流。UDP 流同理（直接 `close_flow`）。

`Engine::shutdown()` 保持"关闭所有流"语义不变。

### 6.3 控制通道（`session.rs` + `worker.rs`）

两个控制文件，均放在会话目录、复用 `write_atomic` 原子写：

- `<session>/mode`：单行 `rule|global|direct`
- `<session>/rules.json`：`{ "rules": [...], "default_policy": "DIRECT" }`

`VpnSession` 新增：`mode_path()` / `write_mode()` / `read_mode()`（trim，空串视为 None）/ `rules_path()` / `write_rules()` / `read_rules_raw()` / `remove_control_files()`。`prepare()` 的清理列表加入这两个文件，避免上一次会话的状态串到下一次连接。

worker 主循环顶部（`poll_learn_clear()` 旁边）新增两个 200ms 节流的轮询：

- `poll_mode_change()`：
  - 目标 == 当前已应用模式 → 返回；
  - 目标为 `rule` 且还没编译过规则 → 编译手里的规则（失败则记日志、保持当前模式、把 `mode` 文件写回当前模式）；
  - 引擎已装配 → `set_rules`（仅切到 rule）+ `set_mode`，记日志；未装配 → 记为 `pending_mode`，装配后立即应用。
- `poll_rules_change()`：
  - 记录上一次读到的文件内容，内容不变直接返回（避免重新编译）；
  - 变化 → 编译新规则：成功则更新 `raw_rules` / `default_policy`，`rule` 模式下 `set_rules` + `reset_changed_flows()` 并记日志；失败则记日志并保留旧规则（主进程已校验，这里是兜底）；
  - `global`/`direct` 模式下只更新手里的规则，不做任何重置。

worker 启动时按 `request.proxy_mode` 决定是否编译规则：`rule` 才编译（失败照旧连接失败），其余模式传 `None`。

### 6.4 命令与前端

新增命令：

- `vpn_set_proxy_mode(mode: String)`：校验取值 → 有活动会话（status 非 `Disconnected`/`Failed`）则写 `mode` 文件，否则直接 `Ok`。纯文件操作，不需要提权。
- `vpn_push_rules(rules: Vec<VpnRule>, default_policy: String)`：先编译校验（失败直接返回错误，前端记日志、不落盘）→ 有活动会话则写 `rules.json`，否则 `Ok`（下次连接按配置生效）。

前端：

- `useVpn.setProxyMode`：先 `updateProfile` 持久化，若 `isConnected` 再调 `vpn_set_proxy_mode`；日志「已切换为 X，新连接按新模式走」。
- 规则相关入口（`addRule` / `updateRule` / `deleteRule` / 启停 / 选中规则列表切换 / `default_policy` 修改）统一走一个**防抖 300ms** 的 `pushRulesIfConnected()`，避免连续编辑反复重建流。
- `VpnPanel`：`modeLocked` 只在 `connecting || phaseBusy` 为真；按钮 `title` 与提示文案改为「切换立即生效：新连接按新模式，已建立的连接保持原路径」。

### 6.5 生命周期与清理

| 时机 | 动作 |
|---|---|
| 连接准备 | `prepare()` 删除旧的 `mode`、`rules.json`、`main.alive`、status/log/learn |
| 切模式 / 改规则 | 原子覆盖对应文件 |
| 断开（点断开 / 托盘断开） | worker 正常收尾：关闭所有流 → 还原路由 → 删 `mode`、`rules.json`、`request`（含明文密码）→ 写 disconnected |
| 托盘「退出」 | 现有 `shutdown()` 请求 worker 收尾（保留） |
| **Cmd+Q / 菜单退出 / 系统关机** | 新增 `RunEvent::ExitRequested` 钩子：调用同一个 `shutdown()`（幂等），并清掉控制文件 |
| **主进程被强杀** | worker 侧"父进程心跳守卫"：主进程在连接期间每 2s 刷新 `<session>/main.alive` 的 mtime；worker 每 1s 检查，超过 8s 未刷新（或文件消失）即认为主进程已死，走与正常停止相同的收尾流程后退出 |

心跳线程只在有活动会话时存在（连接建立后启动，检测到 disconnected/failed 后自行退出），不引入常驻后台线程。

## 7. 错误处理

| 场景 | 行为 |
|---|---|
| `mode` / `rules.json` 写入失败 | 命令返回错误，前端记日志，沿用旧状态 |
| 切到规则模式但规则编译失败 | worker 记日志、保持原模式、把 `mode` 文件写回原模式 |
| `vpn_push_rules` 校验失败 | 命令返回错误，不写文件，前端提示 |
| worker 未装配引擎时切换 | 记为 pending，装配后应用 |
| 非法模式字符串 | 命令 / 请求解析直接报错 |
| 心跳文件缺失 | 启动后 10s 宽限期内忽略，之后视为父进程已死 |

## 8. 测试计划

单元测试：

- `apply_mode`：Rule 保持原判、Global 强制 VPN、Direct 强制直连。
- `ProxyMode::parse`：合法三值、大小写、非法值报错。
- `resolve_proxy_plan`：三种模式都返回全量（启用中的）规则 + 正确模式；非法模式报错；规则模式校验失败报错、全局/直连不校验。
- 判定输入记录与比较：构造出"新规则下判定发生变化"的输入，验证只挑出该流。
- `VpnSession`：`mode` / `rules.json` 写读、`prepare` 清掉旧文件。
- 现有 worker 测试夹具补 `proxy_mode` 字段。

回归：`cargo test --lib`、`pnpm -s exec vue-tsc --noEmit`、`pnpm -s build`。

手动验收：

1. 连接后切「全局」→ 立即生效，**现有连接不断**，新连接走隧道；
2. 全局 ↔ 直连 ↔ 规则来回切多次，隧道地址不变、无重连、原有长连接不断；
3. 规则模式下改一条规则（如把某站点从直连改成走 VPN）→ 该站点的连接被重置并按新规则重建，其它连接不受影响；
4. 断开后 `mode` / `rules.json` 不存在、无残留路由；
5. 托盘退出 / Cmd+Q 后 worker 进程已退出、路由已还原；
6. 连接状态下强杀主进程，8s 内 worker 自行收尾退出。

## 9. 影响面与风险

- 改动文件：`models.rs`、`router.rs`、`worker.rs`、`session.rs`、`commands.rs`、`lib.rs`、`useVpn.ts`、`VpnPanel.vue`；约 350 行（含测试、心跳守卫、定向重置），不新增依赖。
- 风险点：
  - 定向重置依赖建立时记录的判定输入；若输入已不可复现（进程名查询失败等），该流按"判定不变"处理、不重置，属于保守失败。
  - 规则频繁变更由前端 300ms 防抖兜住；worker 端按文件内容比较，内容不变不重编译。
- 回滚：行为由 `proxy_mode` 字段与两个控制文件驱动，文件不存在时与现状一致，可按文件回滚。
