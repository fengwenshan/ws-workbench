# VPN 配置模型改造 + 通道共享 + 运行期热切换 + 退出清理 设计

- 日期：2026-11-05
- 状态：已实现（第 4 版；M1–M4 代码完成，手动验收待真机执行）
- 范围：`src-tauri/src/vpn/*`、`src-tauri/src/lib.rs`、`src/components/vpn/*`、`src/composables/useVpn.ts`、`src/types/*`

## 1. 背景

四件事叠在一起，必须一起设计：

1. **代理模式和规则集都是连接时写死的**：`resolve_proxy_plan` 把模式翻译成"规则集 + 兜底策略"落盘给 worker，worker 启动时编译一次；之后主进程和 worker 之间没有运行期通道。连接后既不能切模式也不能改规则，只能断开重连（提权 + 启 openconnect + 等隧道 + 下发路由，很慢）。
2. **规则列表是全局共享对象**：`AppConfig.vpn_rule_sets[]` + `VpnProfile.rule_set_id` 引用，改一份影响多个配置，概念也多一层。
3. **同一台服务器的多套规则无法共存**：想要"公司网一套、家里网一套"，只能拆成多个配置，而多配置之间又必须断开重连。
4. **退出清理有两个缺口**：Cmd+Q/菜单退出没有钩子；主进程被强杀时 worker 残留并继续维护路由。

## 2. 目标

1. **一个配置只带一份规则列表**：规则内嵌进 `VpnProfile`，配置之间完全隔离；多套规则 = 多个配置。
2. **面板里直接内嵌规则表格**，增删改、启停、MATCH 走向都在页面上完成；规则列表选择器与规则弹窗移除。
3. **同通道配置共享隧道**：连接参数完全相同的两个配置（名称、规则、模式除外），切换时**不重启隧道**，直接把运行期策略换成新配置的；旧配置在界面上显示为"共用通道"（灰色），点它可瞬间切回。
4. 已连接时切换代理模式**立即生效且不打断已建立的连接**（它们按建立时的路径走完，只有新连接按新模式）；可反复来回切。
5. 规则变更**热生效**：重新编译，只重置判定结果发生变化的已建立连接。
6. 全局 / 直连模式下规则集完全不参与判定（不编译、不校验、不匹配）。
7. 断开 / 退出（含 Cmd+Q、菜单退出、强杀）后不残留路由、进程、控制文件。
8. 连接参数不同时，点另一个配置要**先确认再自动断开重连**。

## 3. 非目标

- 同时维持两条隧道（两个不同服务器同时连接）。
- 把已建立的直连连接迁移进隧道（物理约束，见 4.2）。
- 规则语法/匹配能力调整。
- `fake_ip_range` / `fake_ip_filter` 的运行期热切换（它们算通道身份，改了要求重连）。

## 4. 关键事实与约束

### 4.1 Clash 的做法

Clash 是本地代理，模式只是规则引擎的运行期开关，在新连接建立时读取。切模式不重启、不重连，已建立连接保持原出口走到结束。本设计的模式切换与 Clash 一致。

### 4.2 本项目的物理约束

- VPN 侧连接由引擎在用户态终结（smoltcp 收包 + 自己发上游 socket），引擎能主动终止。
- 直连侧连接是应用自己的 socket，创建时被 `IP_BOUND_IF` 绑死物理网卡（`router.rs` 的 `bind_fd_to_if`），包不进 TUN；内核不允许改已 bind socket 的出口。

因此：切模式时存量连接按原路径走完；规则变更时判定结果变了的流会被 RST 重建。

### 4.3 为什么不能同时连两条隧道

系统里只有一份 session 目录、一个提权 worker、一块 TUN 网卡、一套系统路由与 socket 绑定策略。第二条隧道会与第一条抢路由与出口绑定，必然互相破坏。所以"多个配置"只能是"共享同一条通道"或"断开重连换通道"。

## 5. 数据模型变更

### 5.1 前端类型

- `VpnProfile` 增加 `rules: VpnRule[]`、`rules_default_policy: VpnPolicy`；删除 `rule_set_id`。
- `AppConfig.vpn_rule_sets` 改为**可选 legacy 字段**：只读、仅供一次性迁移，之后不被 UI/运行期读写，也不删除（老数据留在配置 JSON 里可回溯）。
- 删除 `VpnRuleSet` 的运行时角色（类型保留为迁移输入）。
- `VpnRule` / `VpnRuleKind` / `VpnPolicy` 不变。

### 5.2 后端

- `VpnProfile` 删除 `rule_set_id`。
- `VpnRequest` 增加 `proxy_mode: String`、`conn_identity: String`。
- `VpnStatus` 增加 `conn_identity: String`（`#[serde(default)]`）。
- `VpnSnapshotPayload` 增加 `policy_profile_id: String`（运行期"当前生效配置"）。

### 5.3 老配置迁移（前端加载时执行一次，幂等）

```
if (config.vpn_rule_sets?.length) {
  for (profile of config.vpn_profiles) {
    set = vpn_rule_sets.find(s => s.id === profile.rule_set_id)
    profile.rules = set?.rules ?? []
    profile.rules_default_policy = set?.default_policy ?? 'DIRECT'
    delete profile.rule_set_id
  }
  // 迁移输入原样保留，不改写、不删除；只统计"没被任何配置引用"的份数写日志
}
```

- 多个配置引用同一份列表 → 各自独立副本。
- 迁移函数独立成模块并写单测（纯函数）。

## 6. 通道身份（决定"共享还是重连"）

### 6.1 字段划分

| 通道身份（变了必须重连） | 运行期策略（可热切换） |
|---|---|
| `server`、`username`、`password`、`protocol`、`connector`、`connector_path`、`extra_args`、`fake_ip_range`、`fake_ip_filter` | `name`、`rules`、`rules_default_policy`、`proxy_mode` |

### 6.2 计算

- 后端 `fn conn_identity(profile: &VpnProfile) -> String`：把上表左侧字段按固定顺序拼接（`\x1f` 分隔，集合字段排序后用 `\x1e` 连接），做稳定哈希（本地实现 FNV-1a 64，输出 16 位十六进制），**不把密码明文写进任何文件**。
- 连接时算一次，写入 `request.conn_identity`；worker 把它复制进每次写出的 `status.conn_identity`。
- 前端需要给列表里每个配置标注状态，因此新增命令 `vpn_conn_identities(profiles) -> Vec<String>`（纯计算，无状态），前端在配置变化时刷新一次。

## 7. 运行期通道

### 7.1 连接请求

`resolve_proxy_plan` 返回 `ProxyPlan { mode, rules, default_policy }`：解析模式（非法报错）、规则始终全量下发（过滤未启用）；**编译校验只在 `rule` 模式做**，全局/直连模式不编译、不校验。

### 7.2 引擎（`router.rs`）

```rust
pub enum ProxyMode { Rule, Global, Direct }
pub fn apply_mode(mode: ProxyMode, egress: Egress) -> Egress   // 纯函数
```

`Engine`：`mode: ProxyMode`、`rules: Option<CompiledRuleSet>`；`assemble` 增加这两个参数。

- `set_mode(mode)`：只改字段，**不重置流**。
- `set_rules(rules)`。
- `reset_changed_flows()`：对每条已建立的流用新规则重算，只重置走向变了的。
- 判定点（TCP 3 处、UDP 3 处、DNS 应答 policy 1 处）统一走 `apply_mode`。
- 定向重置依据：`FlowEgress` / `UdpEgress` 记录 `egress`、`target`、`domain`、`process`。

### 7.3 控制文件与轮询（`session.rs` + `worker.rs`）

- `<session>/mode`：单行 `rule|global|direct`
- `<session>/rules.json`：`{ "rules": [...], "default_policy": "DIRECT" }`
- 均 `write_atomic`；`prepare()` 清理；断开 / 退出时删除。
- worker 每 200ms 节流轮询：
  - `poll_mode_change()`：切到 `rule` 前先编译（失败则记日志 + 回写原模式）；引擎已装配则 `set_rules` + `set_mode`，否则记 pending。
  - `poll_rules_change()`：按内容比较，变了才编译；成功更新 `raw_rules` / `default_policy`，`rule` 模式下 `set_rules` + `reset_changed_flows()`；失败保留旧规则。

### 7.4 命令

- `vpn_set_proxy_mode(mode)`：校验 → 有活动会话则写 `mode` 文件，否则 `Ok`。
- `vpn_push_rules(rules, default_policy)`：先编译校验（失败报错、不落盘）→ 有活动会话则写 `rules.json`，否则 `Ok`。
- `vpn_switch_profile(profile) -> Result<bool, String>`：
  - 无活动会话 → `Err("当前未连接")`；
  - `conn_identity(profile) == status.conn_identity` → 校验规则后写 `rules.json` 与 `mode` 文件，`runtime.policy_profile_id = profile.id`，返回 `Ok(true)`（共享切换完成，隧道没动）；
  - 不一致 → `Ok(false)`（前端弹确认框，确认后断开重连）。
- `vpn_connect` 增加守卫：`phase` 非 `Disconnected`/`Failed` 时返回「已有活动连接，请先断开」。
- `vpn_disconnect` 清空 `runtime.policy_profile_id`。

## 8. UI 变更

### 8.1 规则卡片

- 「规则列表」→「分流规则」：**直接内嵌 `VpnRuleTable`**（增删改、启停、MATCH 走向都在面板内完成），顶部保留说明与「N / M 条启用」，表格限高内部滚动。
- 移除规则列表单选、"当前生效"badge、新建/编辑/删除列表按钮、`VpnRuleSetDialog.vue`。
- `VpnRuleTable` props 由 `ruleSet` 改为 `rules` + `defaultPolicy`。
- 非规则模式：表格可编辑，顶部提示「当前是 X 模式，规则不参与分流」。
- 选中配置不是"当前生效配置"时：顶部提示「该配置未连接，改动将在下次连接生效」，不推送运行期。

### 8.2 配置列表三态

| 状态 | 判定 | 样式与行为 |
|---|---|---|
| 当前生效 | `status.profile_id` 是通道拥有者且 `policy_profile_id` 等于它 | 绿点 + 高亮「已连接」 |
| 共用通道 | 与通道身份相同、但不是当前生效 | **灰色**，标注「共用通道」；点击 = 热切换（不重连） |
| 其它 | 其余 | 常规未连接态 |

- 热切换成功后把 `policy_profile_id` 同步到 `config.vpn_active_profile_id`（已有的"上次选中配置"字段），刷新/重启后选中的仍是当前生效配置。

### 8.3 连接按钮

- 未连接 → 「连接 VPN」
- 已连接且选中 = 当前生效 → 「断开 VPN」
- 已连接且选中 ≠ 当前生效：
  - 同通道 → 「切换到该配置」（点击即热切换）
  - 不同通道 → 「切换（需断开重连）」→ 弹确认框「B 与当前通道参数不同，需要断开并重新连接，是否继续？」→ 确认后断开 + 连接。

### 8.4 运行期设置归属

- 代理模式按钮只在"选中配置 = 当前生效配置"时可点；其它配置下置灰并提示「该配置未连接」。
- 规则表格始终可编辑（规则先存着，切换到该配置或下次连接时生效）。

## 9. 生命周期与清理

| 时机 | 动作 |
|---|---|
| 连接准备 | `prepare()` 删除 `mode`、`rules.json`、`main.alive`、status/log |
| 切模式 / 改规则 / 共享切换 | 原子覆盖对应控制文件 |
| 断开 | worker 正常收尾：关闭流 → 还原路由 → 删控制文件与 `request`（含明文密码）→ 写 disconnected |
| 托盘退出 | 现有 `shutdown()` 请求 worker 收尾 |
| **Cmd+Q / 菜单退出 / 关机** | 新增 `RunEvent::ExitRequested` 钩子调同一个 `shutdown()`（幂等）并清控制文件 |
| **主进程被强杀** | worker 父进程心跳守卫：主进程连接期间每 2s 刷 `<session>/main.alive` mtime；worker 每 1s 检查，8s 未刷新即认为父进程已死，走正常收尾后退出 |

## 10. 错误处理

| 场景 | 行为 |
|---|---|
| 控制文件写入失败 | 命令返回错误，前端记日志，沿用旧状态 |
| 切到规则模式但编译失败 | worker 记日志、保持原模式、回写原模式 |
| `vpn_push_rules` 校验失败 | 命令报错、不写文件、前端提示 |
| 活动会话下直接 `vpn_connect` | 「已有活动连接，请先断开」 |
| 共享切换时规则非法 | 命令报错，不切换，隧道不受影响 |
| 引擎未装配时切换 | 记 pending，装配后应用 |
| 心跳文件缺失 | 启动 10s 宽限期，之后视为父进程已死 |

## 11. 实施顺序

- **M1 配置模型改造**：类型改造 + 迁移模块与单测 + 规则内嵌 UI（表格进面板、删列表选择与规则弹窗）+ `useVpn` 规则 CRUD 改造 + 连接取 `profile.rules`。
- **M2 运行期通道**：`ProxyMode` / `apply_mode` / `set_mode` / `reset_changed_flows`、两个控制文件与轮询、`vpn_set_proxy_mode` / `vpn_push_rules`、前端防抖推送与模式解锁。
- **M3 通道共享**：`conn_identity` 计算与下发、`vpn_switch_profile`、`vpn_conn_identities`、按钮与三态 UI、确认框、`policy_profile_id` 贯通。
- **M4 退出清理与守卫**：`ExitRequested` 钩子、父进程心跳守卫、并发连接守卫。

每个里程碑独立可验证：M1 后模型与 UI 即最终形态（规则仍需重连生效）；M2 后模式与规则热生效；M3 后同通道配置可秒切。

## 12. 测试计划

单元测试：

- `apply_mode` 三态；`ProxyMode::parse` 合法/非法。
- `resolve_proxy_plan`：三种模式都返回全量启用规则 + 正确模式；非法模式报错；规则模式校验失败报错、全局/直连不校验。
- `conn_identity`：名称/规则/模式变化不影响；每个通道身份字段变化都会改变结果；同一配置多次计算结果稳定。
- 共享切换决策：同身份 → 共享；不同身份 → 需要重连。
- 定向重置的判定比较：构造"新规则下走向改变"的输入，验证只挑出该流。
- `VpnSession`：`mode` / `rules.json` 写读与 `prepare` 清理。
- 迁移函数：单配置 / 多配置共享列表 / 未引用列表 / `rule_set_id` 指向不存在的列表。
- 现有 worker 测试夹具补 `proxy_mode`、`conn_identity`。

回归：`cargo test --lib`、`pnpm -s exec vue-tsc --noEmit`、`pnpm -s build`。

手动验收：

1. 老配置升级后每个配置带独立规则副本，内容与迁移前一致；
2. 面板内增删改规则、改 MATCH 走向正常，断开重连后生效（M1）；
3. 连接中切模式：不断连、新连接按新模式；改一条规则只重置受影响的连接（M2）；
4. 同通道两个配置（仅名称/规则/模式不同）：连接 A 后点 B → 隧道不重建、B 的规则立刻生效、A 变灰；点 A → 瞬间切回；
5. 不同通道两个配置：点另一个配置 → 弹确认框 → 确认后自动断开重连；
6. 已连接时编辑未生效配置的规则：不影响当前运行期，只存盘；
7. 断开后 `mode` / `rules.json` 不存在、无残留路由；
8. 托盘退出 / Cmd+Q 后 worker 已退出、路由已还原；
9. 连接状态下强杀主进程，8s 内 worker 自行收尾；
10. 活动会话下再次 `vpn_connect` 被拒绝。

## 13. 影响面与风险

- 改动文件：`types/vpn.ts`、`types/index.ts`、`useVpn.ts`、`VpnPanel.vue`、`VpnRuleTable.vue`、删除 `VpnRuleSetDialog.vue`、新增迁移与 identity 模块；后端 `models.rs`、`router.rs`、`worker.rs`、`session.rs`、`commands.rs`、`lib.rs`。约 950 行（含测试与心跳守卫），不新增依赖。
- 风险点：
  - 通道身份字段划分若漏项（例如换 connector 路径没识别），会导致"以为共享、实际环境已变"；由 `conn_identity` 单测逐字段覆盖。
  - 共享切换需要在主进程侧维护 `policy_profile_id`；应用重启后无法恢复该值，退回"通道拥有者即当前生效"，属可接受的降级。
  - 定向重置依赖建立时记录的判定输入，输入不可复现时按"判定不变"处理。
  - 面板内嵌规则表格需限高 + 内部滚动，避免页面过长。

## 后续变更

- **学习模式整块移除**：面板卡片、`vpn/learn.rs`（632 行）、`vpn_read_learn_report` / `vpn_clear_learn_report` 命令、`learn.json` / `learn.clear` 会话文件、配置里的 `learn` 字段全部删除。理由是规则按域名写，采集回来的 IP/CIDR 对用户没有可操作性；顺带把 `learn` 从通道身份里去掉，两个配置仅因学习开关不同就不能共享隧道的问题一并消失。

## 修复：macOS 一连上就全网断

**现象**：连接后规则模式下访问内网或外网全部不通，浏览器直接报「网络连接已断开」，系统路由表被捕获网卡的 `0.0.0.0/1` + `128.0.0.0/1` 接管。

**根因**：引擎把 DIRECT 出口钉在「物理网卡索引」上，而 macOS 的快照里索引恒为 0（`unix_list_interfaces` 写死），装配引擎时退回了捕获网卡索引。于是所有直连流量（规则模式下绝大多数）的出站 socket 被 `IP_BOUND_IF` 钉到捕获 utun 自己身上，数据包从自己的捕获口绕回来，等于黑洞。

**改法**：macOS 按默认路由给出的接口名（如 `en0`）用 `if_nametoindex` 取真实索引（`route::if_index_for_name`）；拿不到索引直接中止连接，不再退回捕获网卡。日志里新增 `[引擎] 直连出口 if#N（en0）` 便于确认。
