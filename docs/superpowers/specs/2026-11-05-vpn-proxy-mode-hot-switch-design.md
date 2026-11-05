# VPN 配置模型改造 + 代理模式热切换 + 规则热更新 + 退出清理 设计

- 日期：2026-11-05
- 状态：待评审（第 3 版：规则列表内嵌进配置；面板内直接内嵌规则表格）
- 范围：`src-tauri/src/vpn/*`、`src-tauri/src/lib.rs`、`src/components/vpn/*`、`src/composables/useVpn.ts`、`src/types/*`

## 1. 背景

三件事叠在一起，必须一起设计：

1. **代理模式和规则集都是连接时写死的**：`resolve_proxy_plan` 把模式翻译成"规则集 + 兜底策略"落盘给 worker，worker 启动时编译一次。之后主进程和 worker 之间没有任何运行期通道，所以连接后既不能切模式、也不能改规则，只能断开重连（提权 + 启 openconnect + 等隧道 + 下发路由，很慢）。
2. **规则列表是全局共享对象**：`AppConfig.vpn_rule_sets[]` + `VpnProfile.rule_set_id` 引用。多份列表共享导致"改一份影响多个配置"，概念也多一层（列表选择、"当前生效"）。
3. **退出清理有两个缺口**：Cmd+Q/菜单退出没有钩子；主进程被强杀时 worker 会残留并继续维护路由。

## 2. 目标

1. **一个配置只带一份规则列表**：规则内嵌进 `VpnProfile`，配置之间完全隔离；多套规则 = 多个配置。
2. **面板里直接内嵌规则表格**，增删改、启停、MATCH 走向都在页面上完成，规则列表选择器与规则弹窗移除。
3. 已连接时切换代理模式**立即生效且不打断已建立的连接**（它们按建立时的路径走完，只有新连接按新模式）；隧道、网卡、路由表、worker 进程都不重建，可反复来回切。
4. 规则变更**热生效**：重新编译，并**只重置判定结果发生变化的已建立连接**，新连接按新规则。
5. 全局 / 直连模式下规则集完全不参与判定（不编译、不校验、不匹配）。
6. 断开 / 退出（含 Cmd+Q、菜单退出、强杀）后不残留路由、进程、控制文件。
7. 补后端并发连接守卫：已有活动会话时拒绝再次连接，避免第二个 worker 抢同一套路由。

## 3. 非目标

- 把已建立的直连连接迁移进隧道（物理约束，见 4.2）。
- 规则语法/匹配能力调整。
- 规则列表跨配置共享（已按目标 1 取消）。
- status 回显"已生效模式"（界面以配置里的 `proxy_mode` 为准）。

## 4. 关键事实与约束

### 4.1 Clash 的做法

Clash 是本地代理，模式只是规则引擎的运行期开关，在新连接建立时读取。切模式不重启、不重连，已建立的连接保持原出口走到结束——它不迁移存量连接。本设计的模式切换与 Clash 一致。

### 4.2 本项目的物理约束

- VPN 侧连接由引擎在用户态终结（smoltcp 收包 + 自己发上游 socket），引擎能主动终止。
- 直连侧连接是应用自己的 socket，创建时被 `IP_BOUND_IF` 绑死物理网卡（`router.rs` 的 `bind_fd_to_if`），包不进 TUN；内核不允许改已 bind socket 的出口。这类连接不可见、不可控。

因此：切模式时所有存量连接按原路径走完；规则变更时，判定结果变了的流（无论原出口是 VPN 还是直连）会被 RST 重建。

## 5. 数据模型变更

### 5.1 前端类型（`src/types/vpn.ts` / `src/types/index.ts`）

- `VpnProfile` 增加：`rules: VpnRule[]`、`rules_default_policy: VpnPolicy`；删除 `rule_set_id`。
- `AppConfig.vpn_rule_sets` 改为**可选的 legacy 字段**：只读、仅供一次性迁移使用，之后不再被 UI 或运行期读取，也不会被改写或删除（老数据原样保留在配置 JSON 里，可回溯）。
- 删除 `VpnRuleSet` 的运行时角色，类型保留为 legacy 迁移输入。
- `VpnRule` / `VpnRuleKind` / `VpnPolicy` 保持不变。

### 5.2 后端（`src-tauri/src/vpn/models.rs`）

- `VpnProfile` 删除 `rule_set_id`（连接时规则本就通过 `vpn_connect` 的 `rules` 参数传入，不依赖它）。
- `VpnRequest` 增加 `proxy_mode: String`（已有 `rules` / `default_policy`）。

### 5.3 老配置迁移（`useVpn` 加载配置时执行一次，幂等）

```
if (config.vpn_rule_sets?.length) {
  for (profile of config.vpn_profiles) {
    set = vpn_rule_sets.find(s => s.id === profile.rule_set_id)
    profile.rules = set?.rules ?? []
    profile.rules_default_policy = set?.default_policy ?? 'DIRECT'
    delete profile.rule_set_id
  }
  // 迁移输入原样保留：config.vpn_rule_sets 不改写、不删除；
  // 只统计一次"没有被任何配置引用的列表"份数写进日志，便于回溯
}
```

- 多个配置引用同一份列表 → 各自得到独立副本（B 的隔离语义）。
- 迁移函数单独成模块并写单测（纯函数，输入旧配置输出新配置）。

## 6. 运行期通道设计

### 6.1 连接请求

`resolve_proxy_plan`（`commands.rs`）返回 `ProxyPlan { mode, rules, default_policy }`：

- 解析模式，非法值报错；规则始终全量下发（过滤 `enabled == false`）。
- 编译校验只在 `rule` 模式做（失败 = 连接失败，fail-closed）；全局 / 直连模式不编译、不校验。

### 6.2 引擎（`router.rs`）

```rust
pub enum ProxyMode { Rule, Global, Direct }   // parse / as_str
pub fn apply_mode(mode: ProxyMode, egress: Egress) -> Egress  // 纯函数
```

`Engine`：`mode: ProxyMode`、`rules: Option<CompiledRuleSet>`；`assemble` 增加这两个参数。

- `set_mode(mode)`：只改字段，**不重置流**。
- `set_rules(rules)`。
- `reset_changed_flows()`：对每条已建立的流用新规则重算，只重置走向变了的。
- 判定点（TCP 3 处、UDP 3 处、DNS 应答 policy 1 处）统一走 `apply_mode` 包装。

定向重置的依据：`FlowEgress` / `UdpEgress` 记录 `egress`、`target`、`domain`、`process`（建立时的判定输入与结果），重算时用同一套输入比较。

### 6.3 控制文件与轮询（`session.rs` + `worker.rs`）

- `<session>/mode`：单行 `rule|global|direct`。
- `<session>/rules.json`：`{ "rules": [...], "default_policy": "DIRECT" }`。
- 均用 `write_atomic`；`prepare()` 清理；断开与退出时删除。
- worker 主循环顶部两个 200ms 节流轮询：
  - `poll_mode_change()`：变化时（切到 rule 需先编译，失败则记日志 + 回写原模式）`set_rules` + `set_mode`；未装配引擎记为 pending。
  - `poll_rules_change()`：按文件内容比较，变了才编译；成功则更新 `raw_rules` / `default_policy`，`rule` 模式下 `set_rules` + `reset_changed_flows()`；失败保留旧规则（主进程已校验，此处兜底）。

### 6.4 命令与前端联动

- `vpn_set_proxy_mode(mode)`：校验 → 有活动会话则写 `mode` 文件，否则 `Ok`。无需提权。
- `vpn_push_rules(rules, default_policy)`：先编译校验（失败直接报错、不落盘）→ 有活动会话则写 `rules.json`，否则 `Ok`。
- `vpn_connect` 增加并发守卫：`status.phase` 非 `Disconnected`/`Failed` 时返回「请先断开当前连接」。
- `useVpn.setProxyMode`：`updateProfile` 持久化 → 若已连接调 `vpn_set_proxy_mode`。
- 规则变更（增删改、启停、MATCH 走向）→ `saveConfig()` + 防抖 300ms 的 `pushRulesIfConnected()`。
- **推送目标必须等于连接中的配置**：`pushRulesIfConnected()` 内部检查 `已连接 && selectedProfile.id === status.profile_id` 才调用 `vpn_push_rules`；否则只存盘。这样"连着 A 却在编辑 B 的规则"不会把 B 的规则推给正在跑的 A（运行时规则表只有一份，推错就是运行期错乱）。

## 7. UI 变更（`VpnPanel.vue` 等）

- 「规则列表」卡片 → 「分流规则」：**直接内嵌 `VpnRuleTable`**（增删改、启停、MATCH 走向都在面板里完成），顶部保留一句说明与「N / M 条启用」。
- 移除：规则列表单选、当前生效 badge、新建/编辑/删除列表按钮、`VpnRuleSetDialog` 组件（删除文件）。
- `VpnRuleTable` props 由 `ruleSet` 改为 `rules` + `defaultPolicy`，事件不变。
- 非规则模式时表格保持可编辑（规则先攒着，切到规则模式即生效），顶部提示「当前是 X 模式，规则不参与分流」。
- 当前选中的配置不是连接中的那个时，表格照常可编辑，顶部提示「该配置未连接，改动将在下次连接生效」（不推送运行期）。
- 代理模式：`modeLocked` 只在 `connecting || phaseBusy` 时锁；文案「切换立即生效：新连接按新模式，已建立的连接保持原路径」。
- 规则表格里的校验仍走 `vpn_validate_rule`。

## 8. 生命周期与清理

| 时机 | 动作 |
|---|---|
| 连接准备 | `prepare()` 删除 `mode`、`rules.json`、`main.alive`、status/log/learn |
| 切模式 / 改规则 | 原子覆盖对应文件 |
| 断开 | worker 正常收尾：关闭所有流 → 还原路由 → 删控制文件与 `request`（含明文密码）→ 写 disconnected |
| 托盘「退出」 | 现有 `shutdown()` 请求 worker 收尾 |
| **Cmd+Q / 菜单退出 / 关机** | 新增 `RunEvent::ExitRequested` 钩子调同一个 `shutdown()`（幂等）并清控制文件 |
| **主进程被强杀** | worker 父进程心跳守卫：主进程连接期间每 2s 刷 `<session>/main.alive` mtime；worker 每 1s 检查，8s 未刷新即认为父进程已死，走正常收尾后退出 |

心跳线程只在有活动会话时存在，不引入常驻线程。

## 9. 错误处理

| 场景 | 行为 |
|---|---|
| 控制文件写入失败 | 命令返回错误，前端记日志，沿用旧状态 |
| 切到规则模式但编译失败 | worker 记日志、保持原模式、回写原模式 |
| `vpn_push_rules` 校验失败 | 命令返回错误，不写文件，前端提示 |
| 已有活动会话时再次 connect | 返回「请先断开当前连接」 |
| 引擎未装配时切换 | 记为 pending，装配后应用 |
| 心跳文件缺失 | 启动 10s 宽限期内忽略，之后视为父进程已死 |

## 10. 实施顺序

- **M1 配置模型改造**：类型改造 + 迁移函数与单测 + 规则内嵌 UI（表格进面板、删规则弹窗与列表选择）+ `useVpn` 规则 CRUD 改造 + 连接取 `profile.rules`。
- **M2 运行期通道**：`ProxyMode` / `apply_mode` / `set_mode` / `reset_changed_flows`、两个控制文件与轮询、`vpn_set_proxy_mode` / `vpn_push_rules`、前端防抖推送与模式解锁。
- **M3 退出清理**：`ExitRequested` 钩子、父进程心跳守卫、并发连接守卫。

每个里程碑独立可验证：M1 完成后规则编辑仍需重连才生效，但模型与 UI 已是最终形态。

## 11. 测试计划

单元测试：

- `apply_mode` 三态；`ProxyMode::parse` 合法/非法。
- `resolve_proxy_plan`：三种模式都返回全量启用规则 + 正确模式；非法模式报错；规则模式校验失败报错、全局/直连不校验。
- 定向重置的判定比较：构造"新规则下走向改变"的输入，验证只挑出该流。
- `VpnSession`：`mode` / `rules.json` 写读与 `prepare` 清理。
- 迁移函数：单配置/多配置共享列表/未引用列表/`rule_set_id` 指向不存在的列表。
- 现有 worker 测试夹具补 `proxy_mode`。

回归：`cargo test --lib`、`pnpm -s exec vue-tsc --noEmit`、`pnpm -s build`。

手动验收：

1. 老配置升级后每个配置带上一份独立规则副本，规则内容与迁移前一致；
2. 面板内直接增删改规则、改 MATCH 走向，断开连接后重连生效（M1）；
3. M2 后：连接中切模式不断连、新连接按新模式；改一条规则只重置受影响的连接；
4. 断开后 `mode` / `rules.json` 不存在、无残留路由；
5. 托盘退出 / Cmd+Q 后 worker 已退出、路由已还原；
6. 连接状态下强杀主进程，8s 内 worker 自行收尾；
7. 已连接时对另一个配置发起连接被拒绝并提示；
8. 连接 A 时在左侧切到 B 并编辑 B 的规则：A 的运行期规则不受影响，B 的改动只存盘并提示"下次连接生效"；
9. 连接 A 时编辑 A 的规则：只重置受影响的连接即生效。

## 12. 影响面与风险

- 改动文件：`types/vpn.ts`、`types/index.ts`、`useVpn.ts`、`VpnPanel.vue`、`VpnRuleTable.vue`、删除 `VpnRuleSetDialog.vue`、新增迁移模块；后端 `models.rs`、`router.rs`、`worker.rs`、`session.rs`、`commands.rs`、`lib.rs`。约 700 行（含测试与心跳守卫），不新增依赖。
- 风险点：
  - 迁移不可逆性：采用"只复制不删除旧字段"的策略，旧数据仍留在配置 JSON 里可回溯。
  - 定向重置依赖建立时记录的判定输入；输入不可复现时按"判定不变"处理，属保守失败。
  - 面板内嵌表格让页面变长，需要控制表格高度（最大高度 + 内部滚动）。
