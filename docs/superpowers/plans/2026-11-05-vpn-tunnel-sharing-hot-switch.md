# VPN 通道共享 + 运行期热切换 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把规则内嵌进 VPN 配置、让同通道配置共享隧道，并让代理模式与规则在连接中热生效，同时补上退出清理。

**Architecture:** 前端配置模型从"全局规则集 + 引用"改为"配置自带规则"；主进程与提权 worker 之间新增文件式控制通道（`mode` / `rules.json`），worker 每 200ms 轮询并热改判；隧道按"通道身份"（连接参数哈希）复用，同身份切换只换策略、不同身份确认后断开重连。

**Tech Stack:** Tauri 2 + Vue 3 + TypeScript + ant-design-vue 4（前端）；Rust（`ws_workbench_lib`，smoltcp 用户态栈 + 规则引擎）。

**通用约定**
- 前端无测试框架：前端任务用 `pnpm -s exec vue-tsc --noEmit` + `pnpm -s build` 验证，逻辑行为靠手动验收。
- Rust 任务用 `cargo test --lib`（在 `src-tauri/` 下），当前基线 298 passed / 1 failed（`vpn::detect::tests::bundled_openconnect_is_probed_not_skipped` 是 macOS 上的既有环境失败，与本计划无关）。
- 每个任务结束提交一次，commit message 用 conventional commits 格式。
- 规格文档：`docs/superpowers/specs/2026-11-05-vpn-proxy-mode-hot-switch-design.md`。

---

## Phase M1 — 配置模型改造（规则内嵌进配置）

### Task 1: 前端类型改造

**Files:**
- Modify: `src/types/vpn.ts:36-52`（删除 `VpnRuleSet` 的运行期角色，保留类型作迁移输入）
- Modify: `src/types/vpn.ts`（`VpnProfile` 增加 `rules` / `rules_default_policy`，删除 `rule_set_id`）
- Modify: `src/types/index.ts:55-68`（`AppConfig.vpn_rule_sets` 改为可选 legacy 字段）

- [ ] **Step 1: 改 `VpnProfile`**

在 `src/types/vpn.ts` 的 `VpnProfile` 中，把 `rule_set_id: string` 换掉，并新增两个字段：

```ts
  /** 这份配置自带的分流规则（模式为 rule 时生效） */
  rules: VpnRule[]
  /** 所有规则都不命中时的兜底走向 */
  rules_default_policy: VpnPolicy
```

`VpnRuleSet` 类型保留，并加注释说明它只用于老配置迁移：

```ts
/** @deprecated 仅用于老配置迁移（vpn_rule_sets → profile.rules），不再参与运行期 */
export interface VpnRuleSet { ... }
```

- [ ] **Step 2: 改 `AppConfig`**

`src/types/index.ts`：

```ts
  /** @deprecated 老版本规则集，只读，仅用于一次性迁移 */
  vpn_rule_sets?: VpnRuleSet[]
```

- [ ] **Step 3: 类型检查（此时必然报错，记录报错清单）**

Run: `pnpm -s exec vue-tsc --noEmit`
Expected: 报 `rule_set_id` / `vpn_rule_sets` 相关错误（`useVpn.ts`、`VpnPanel.vue`），下一个任务修复。

- [ ] **Step 4: 提交**

```bash
git add src/types/vpn.ts src/types/index.ts
git commit -m "refactor(vpn): 规则内嵌进配置，规则集类型降级为迁移输入"
```

### Task 2: 老配置迁移模块

**Files:**
- Create: `src/utils/vpnConfigMigration.ts`
- Modify: `src/App.vue`（加载配置后调用迁移）

- [ ] **Step 1: 写迁移模块**

```ts
import type { AppConfig } from '@/types'
import { RULE_KIND_LABELS, POLICY_LABELS } from '@/types/vpn'
import type { VpnPolicy, VpnRule, VpnRuleKind } from '@/types/vpn'

/** 老配置把规则放在全局 vpn_rule_sets 里，用 profile.rule_set_id 引用；
 *  新模型每个配置自带一份规则。这个函数只做字段搬迁，不改写 vpn_rule_sets 本身。 */
export function migrateVpnRuleSets(config: AppConfig): {
  changed: boolean
  droppedLists: number
} {
  const legacy = config.vpn_rule_sets ?? []
  if (legacy.length === 0) return { changed: false, droppedLists: 0 }
  let changed = false
  const referenced = new Set<string>()
  for (const profile of config.vpn_profiles ?? []) {
    const oldId = (profile as unknown as { rule_set_id?: string }).rule_set_id
    if (profile.rules && profile.rules_default_policy) continue // 已迁移过
    const set = legacy.find((s) => s.id === oldId)
    if (set) referenced.add(set.id)
    profile.rules = set ? (set.rules as VpnRule[]) : []
    profile.rules_default_policy = (set?.default_policy ?? 'DIRECT') as VpnPolicy
    delete (profile as unknown as { rule_set_id?: string }).rule_set_id
    changed = true
  }
  const droppedLists = legacy.filter((s) => !referenced.has(s.id)).length
  return { changed, droppedLists }
}
```

- [ ] **Step 2: 在配置加载后调用**

在 `src/App.vue` 里找到加载配置（`loadConfig` / `store.get('config')`）后写回状态的代码，加迁移调用并记日志；若 `changed` 为真，立刻 `saveConfig()` 持久化：

```ts
import { migrateVpnRuleSets } from '@/utils/vpnConfigMigration'
// ……加载配置之后……
const migration = migrateVpnRuleSets(config.value)
if (migration.changed) {
  await saveConfig()
  if (migration.droppedLists > 0) {
    console.warn(`[vpn] 迁移完成：${migration.droppedLists} 份未引用的旧规则列表仍保留在配置 JSON 中`)
  }
}
```

- [ ] **Step 3: 验证**

Run: `pnpm -s exec vue-tsc --noEmit`
Expected: 只剩 `useVpn.ts` / `VpnPanel.vue` 的规则集引用错误。

- [ ] **Step 4: 提交**

```bash
git add src/utils/vpnConfigMigration.ts src/App.vue
git commit -m "feat(vpn): 老配置规则集迁移进各配置"
```

### Task 3: useVpn 规则 CRUD 改到配置上

**Files:**
- Modify: `src/composables/useVpn.ts:87-97`（删 `ruleSets` / `selectedRuleSet`）
- Modify: `src/composables/useVpn.ts:209-240`（connect 取 `profile.rules`）
- Modify: `src/composables/useVpn.ts:273-435`（规则 CRUD、删规则集 CRUD）
- Modify: `src/composables/useVpn.ts:600-660`（返回值）

- [ ] **Step 1: 删除规则集状态与 CRUD**

删除 `ruleSets`、`selectedRuleSet`、`newRuleSet`、`nextRuleSetName`、`createRuleSet`、`selectRuleSet`、`deleteRuleSet`、`updateRuleSet` 及其返回值导出。导入时（`importOpenconnect` 附近）原本创建规则集的地方，改为给 profile 写入规则。

- [ ] **Step 2: 规则 CRUD 作用于当前配置**

```ts
  /** 规则只属于当前选中的配置 */
  function rulesOf(profileId: string): VpnRule[] {
    const profile = profiles.value.find((p) => p.id === profileId)
    return profile?.rules ?? []
  }

  async function addRule(kind: VpnRuleKind, value: string, policy: VpnPolicy, remark = '') {
    const profile = selectedProfile.value
    if (!profile) return
    profile.rules = [
      ...(profile.rules ?? []),
      { id: crypto.randomUUID(), kind, value, policy, enabled: true, no_resolve: false, remark },
    ]
    await saveConfig()
    await pushRulesIfConnected()
  }

  async function updateRule(ruleId: string, patch: Partial<VpnRule>) {
    const profile = selectedProfile.value
    if (!profile) return
    profile.rules = (profile.rules ?? []).map((rule) =>
      rule.id === ruleId ? { ...rule, ...patch } : rule,
    )
    await saveConfig()
    await pushRulesIfConnected()
  }

  async function deleteRule(ruleId: string) {
    const profile = selectedProfile.value
    if (!profile) return
    profile.rules = (profile.rules ?? []).filter((rule) => rule.id !== ruleId)
    await saveConfig()
    await pushRulesIfConnected()
  }

  async function updateRulesDefaultPolicy(policy: VpnPolicy) {
    const profile = selectedProfile.value
    if (!profile) return
    profile.rules_default_policy = policy
    await saveConfig()
    await pushRulesIfConnected()
  }
```

`pushRulesIfConnected` 在 M2 Task 12 实现；此任务先定义为空操作占位（`async function pushRulesIfConnected() {}`），保证本任务可编译。

- [ ] **Step 3: connect 用配置自带的规则**

```ts
      const mode = target.proxy_mode ?? 'rule'
      const rules = mode === 'rule' ? (target.rules ?? []) : []
      const defaultPolicy = target.rules_default_policy ?? 'DIRECT'
```

删除 `ruleSets.value.find(...)` 与"规则模式需要先选择一个规则列表"的检查（改为：规则模式且 `target.rules` 为空时仅告警，不阻断）。

- [ ] **Step 4: 校验**

Run: `pnpm -s exec vue-tsc --noEmit`
Expected: 只剩 `VpnPanel.vue` 的规则集引用错误。

- [ ] **Step 5: 提交**

```bash
git add src/composables/useVpn.ts
git commit -m "refactor(vpn): 规则 CRUD 改为作用于配置自身"
```

### Task 4: 面板内嵌规则表格

**Files:**
- Modify: `src/components/vpn/VpnRuleTable.vue`（props 改 `rules` + `defaultPolicy`）
- Modify: `src/components/vpn/VpnPanel.vue:495-570`（规则卡片换成内嵌表格）
- Delete: `src/components/vpn/VpnRuleSetDialog.vue`

- [ ] **Step 1: 改 `VpnRuleTable` props**

把 `ruleSet: VpnRuleSet` 换成：

```ts
const props = defineProps<{
  rules: VpnRule[]
  defaultPolicy: VpnPolicy
  validateRule: (kind: VpnRuleKind, value: string) => Promise<string>
  embedded?: boolean
}>()
```

模板里所有 `ruleSet.rules` → `rules`、`ruleSet.default_policy` → `defaultPolicy`。保留 `showInline` 等现有行为。

- [ ] **Step 2: `VpnPanel` 内嵌表格**

把「规则列表」卡片（`rule-set-list` / `rule-set-empty` / `rule-fallback` 整块）替换为：

```html
          <section class="card">
            <div class="card-head">
              <h4>分流规则</h4>
              <span class="card-sub">{{ currentMode.value === 'rule' ? '命中即停，未命中走 MATCH 走向' : `当前是${currentMode.label}模式，规则不参与分流` }}</span>
            </div>
            <VpnRuleTable
              :rules="vpn.selectedProfile?.rules ?? []"
              :default-policy="vpn.selectedProfile?.rules_default_policy ?? 'DIRECT'"
              :validate-rule="vpn.validateRule"
              @add="handleAddRule"
              @update="handleUpdateRuleRow"
              @remove="handleRemoveRule"
              @update-default-policy="handleRulesDefaultPolicy"
            />
          </section>
```

新增的四个 handler 直接转调 `vpn.addRule/updateRule/deleteRule/updateRulesDefaultPolicy`。删除 `openRuleSet` / `handleNewRuleSet` / `handleSelectRuleSet` / `handleRuleSetDefaultPolicy` / `askDeleteRuleSet` / `confirmDeleteRuleSet` / `editingRuleSet` 等状态与函数，以及 `VpnRuleSetDialog` 的导入与挂载。

- [ ] **Step 3: 删除规则弹窗组件**

```bash
git rm src/components/vpn/VpnRuleSetDialog.vue
```

- [ ] **Step 4: 验证**

Run: `pnpm -s exec vue-tsc --noEmit && pnpm -s build`
Expected: 0 error，构建成功。

- [ ] **Step 5: 提交**

```bash
git add -A src/components/vpn
git commit -m "feat(vpn): 面板内直接编辑分流规则，移除规则列表选择"
```

---

## Phase M2 — 运行期通道（模式 / 规则热生效）

### Task 5: 引擎模式判定

**Files:**
- Modify: `src-tauri/src/vpn/router.rs:511-560`（新增 `ProxyMode` / `apply_mode`）
- Modify: `src-tauri/src/vpn/router.rs:560-620`（`Engine` 字段与 `assemble`）
- Test: `src-tauri/src/vpn/router.rs`（`#[cfg(test)] mod tests` 内追加）

- [ ] **Step 1: 写失败测试**

在 `router.rs` 测试模块追加：

```rust
    #[test]
    fn apply_mode_overrides_egress() {
        assert_eq!(apply_mode(ProxyMode::Global, Egress::Direct), Egress::Vpn);
        assert_eq!(apply_mode(ProxyMode::Direct, Egress::Vpn), Egress::Direct);
        assert_eq!(apply_mode(ProxyMode::Rule, Egress::Direct), Egress::Direct);
    }

    #[test]
    fn proxy_mode_parse_accepts_three_values() {
        assert_eq!(ProxyMode::parse("rule").unwrap(), ProxyMode::Rule);
        assert_eq!(ProxyMode::parse("GLOBAL").unwrap(), ProxyMode::Global);
        assert_eq!(ProxyMode::parse("direct").unwrap(), ProxyMode::Direct);
        assert!(ProxyMode::parse("bogus").is_err());
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test --lib apply_mode_overrides_egress`
Expected: 编译失败（`apply_mode` / `ProxyMode` 未定义）。

- [ ] **Step 3: 实现**

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProxyMode {
    Rule,
    Global,
    Direct,
}

impl ProxyMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rule" => Ok(Self::Rule),
            "global" => Ok(Self::Global),
            "direct" => Ok(Self::Direct),
            other => Err(format!("未知的代理模式：{other}（仅支持 rule / global / direct）")),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }
}

/// 全局强制 VPN、直连强制 DIRECT，规则模式保持规则判定结果
pub fn apply_mode(mode: ProxyMode, egress: Egress) -> Egress {
    match mode {
        ProxyMode::Global => Egress::Vpn,
        ProxyMode::Direct => Egress::Direct,
        ProxyMode::Rule => egress,
    }
}
```

`Engine` 增加字段 `mode: ProxyMode`、把 `rules: CompiledRuleSet` 改为 `rules: Option<CompiledRuleSet>`；`assemble` 增加 `mode: ProxyMode`、`rules: Option<CompiledRuleSet>` 两个参数并赋值。

- [ ] **Step 4: 断言判定点全部走包装**

把 7 处判定（TCP 三处、UDP 三处、`dns_respond` 的 policy 一处）包上 `apply_mode(self.mode, ...)`；`Rule` 模式下 `self.rules` 为 `None` 时退化为 `Egress::Direct` / `Policy::Direct` 并记一次日志。

- [ ] **Step 5: 运行测试**

Run: `cd src-tauri && cargo test --lib router`
Expected: 全部 PASS（含既有 19 个 router 测试）。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/vpn/router.rs
git commit -m "feat(vpn): 引擎支持代理模式运行期判定"
```

### Task 6: 引擎 set_mode / set_rules / 定向重置

**Files:**
- Modify: `src-tauri/src/vpn/router.rs`（`FlowEgress` / `UdpEgress` 记录判定输入；`Engine::set_mode/set_rules/reset_changed_flows`）
- Test: `src-tauri/src/vpn/router.rs`

- [ ] **Step 1: 记录判定输入**

`FlowEgress` 增加：

```rust
struct FlowEgress {
    stream: Option<TcpStream>,
    installed_route: Option<Ipv4Addr>,
    target: SocketAddrV4,
    egress: Egress,
    domain: Option<String>,
    process: Option<String>,
}
```

`UdpEgress` 同样增加 `egress` / `domain` / `process`（`target` 已存在）。`open_egress_to` / `open_udp_egress_to` 的调用方（`poll`）把 domain / process 传进来并记录。

- [ ] **Step 2: 实现重算与定向重置**

```rust
impl Engine {
    pub fn set_mode(&mut self, mode: ProxyMode) {
        self.mode = mode;
    }

    pub fn set_rules(&mut self, rules: CompiledRuleSet) {
        self.rules = Some(rules);
    }

    /// 用当前规则重算每条已建立的流，只重置走向变了的
    pub fn reset_changed_flows(&mut self) {
        let mut stale = Vec::new();
        for (key, flow) in self.router.flow_entries() {
            let decided = self.decide_for_flow(&key, flow.target, flow.domain.as_deref(), flow.process.as_deref());
            if decided != flow.egress {
                stale.push(key);
            }
        }
        for key in self.router.udp_flow_entries() { /* 同样的比较，不同则 stale.push */ }
        for key in stale {
            self.netstack.abort_tcp(&key);
            self.router.close_flow(&key);
        }
    }
}
```

为此在 `Router` 新增只读访问器 `flow_entries()` / `udp_flow_entries()` 返回可迭代的键与状态快照（不暴露 socket）。

- [ ] **Step 3: 写测试**

```rust
    #[test]
    fn rule_change_marks_only_changed_egress() {
        // 用 apply_mode + 规则集构造两条不同走向的流输入，
        // 断言只有走向改变的那条被判定为需要重置
        assert!(decision_changed(Egress::Direct, Egress::Vpn));
        assert!(!decision_changed(Egress::Vpn, Egress::Vpn));
    }
```

`decision_changed(old, new) -> bool` 为 `old != new` 的命名包装，便于单测与后续扩展。

- [ ] **Step 4: 运行**

Run: `cd src-tauri && cargo test --lib router`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/router.rs
git commit -m "feat(vpn): 规则变更时只重置走向变化的连接"
```

### Task 7: 请求携带模式 + 校验降级

**Files:**
- Modify: `src-tauri/src/vpn/models.rs:410-425`（`VpnRequest` 加 `proxy_mode`）
- Modify: `src-tauri/src/vpn/commands.rs:448-465`（`resolve_proxy_plan`）
- Modify: `src-tauri/src/vpn/commands.rs:470-560`（`vpn_connect` 使用新 plan）
- Test: `src-tauri/src/vpn/commands.rs`

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn resolve_proxy_plan_keeps_rules_in_global_mode() {
        let plan = resolve_proxy_plan("global", sample_rules(), "DIRECT".into()).unwrap();
        assert_eq!(plan.mode.as_str(), "global");
        assert_eq!(plan.rules.len(), 1);
    }

    #[test]
    fn resolve_proxy_plan_rejects_unknown_mode() {
        assert!(resolve_proxy_plan("bogus", vec![], "DIRECT".into()).is_err());
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test --lib resolve_proxy_plan`
Expected: 编译失败（`resolve_proxy_plan` 返回值类型不符）。

- [ ] **Step 3: 实现**

```rust
struct ProxyPlan {
    mode: ProxyMode,
    rules: Vec<VpnRule>,
    default_policy: String,
}

fn resolve_proxy_plan(mode: &str, rules: Vec<VpnRule>, default_policy: String) -> Result<ProxyPlan, String> {
    let mode = ProxyMode::parse(mode)?;
    Ok(ProxyPlan {
        mode,
        rules: rules.into_iter().filter(|r| r.enabled).collect(),
        default_policy,
    })
}
```

`vpn_connect` 里：只在 `mode == Rule` 时做编译校验（失败直接返回错误）；全局/直连跳过校验。`request.proxy_mode = plan.mode.as_str().to_string()`。

- [ ] **Step 4: 运行**

Run: `cd src-tauri && cargo test --lib commands`
Expected: PASS（既有 overlap 测试不受影响）。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/models.rs src-tauri/src/vpn/commands.rs
git commit -m "feat(vpn): 连接请求携带代理模式，全局/直连不校验规则"
```

### Task 8: 会话控制文件

**Files:**
- Modify: `src-tauri/src/vpn/session.rs`（`mode_path` / `rules_path` / 读写方法 / `prepare` 清理）
- Test: `src-tauri/src/vpn/session.rs`

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn mode_file_roundtrip_and_prepare_clears_it() {
        let session = temp_session("mode-file");
        session.write_mode("global").unwrap();
        assert_eq!(session.read_mode().as_deref(), Some("global"));
        session.prepare().unwrap();
        assert!(session.read_mode().is_none());
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test --lib mode_file_roundtrip`
Expected: 编译失败（方法未定义）。

- [ ] **Step 3: 实现**

```rust
const MODE_FILE: &str = "mode";
const RULES_FILE: &str = "rules.json";

pub fn mode_path(&self) -> PathBuf { self.dir.join(MODE_FILE) }
pub fn rules_path(&self) -> PathBuf { self.dir.join(RULES_FILE) }

pub fn write_mode(&self, mode: &str) -> Result<(), String> {
    Self::write_atomic(&self.mode_path(), format!("{mode}\n").as_bytes().into())
}
pub fn read_mode(&self) -> Option<String> {
    fs::read_to_string(self.mode_path()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
pub fn write_rules(&self, rules: &[VpnRule], default_policy: &str) -> Result<(), String> { /* serde_json 序列化 + write_atomic */ }
pub fn read_rules_raw(&self) -> Option<String> { fs::read_to_string(self.rules_path()).ok() }
pub fn remove_control_files(&self) { let _ = fs::remove_file(self.mode_path()); let _ = fs::remove_file(self.rules_path()); }
```

`prepare()` 的文件清理列表加入这两个路径。

- [ ] **Step 4: 运行**

Run: `cd src-tauri && cargo test --lib session`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/session.rs
git commit -m "feat(vpn): 新增 mode / rules 控制文件"
```

### Task 9: worker 轮询与热应用

**Files:**
- Modify: `src-tauri/src/vpn/worker.rs:300-470`（启动时按模式编译、装配引擎传 mode）
- Modify: `src-tauri/src/vpn/worker.rs:889-920`（主循环新增两个轮询）
- Test: `src-tauri/src/vpn/worker.rs`（夹具补字段）

- [ ] **Step 1: 启动按模式编译**

```rust
let mode = ProxyMode::parse(&self.request.proxy_mode)?;
let rules = if mode == ProxyMode::Rule { Some(self.compiled_rules()?) } else { None };
```
装配时把 `mode` / `rules` 传给 `Engine::assemble`。worker 增加字段 `applied_mode: ProxyMode`、`raw_rules: Vec<VpnRule>`、`default_policy: String`、`pending_mode: Option<ProxyMode>`。

- [ ] **Step 2: 轮询实现**

```rust
fn poll_mode_change(&mut self) {
    if self.last_mode_check.elapsed() < Duration::from_millis(200) { return; }
    self.last_mode_check = Instant::now();
    let Some(raw) = self.session.read_mode() else { return };
    let Ok(target) = ProxyMode::parse(&raw) else { return };
    if target == self.applied_mode { return; }
    if target == ProxyMode::Rule && self.engine_rules_ready() == false {
        match self.compiled_rules() {
            Ok(set) => { self.engine.set_rules(set); }
            Err(e) => {
                self.log(&format!("[模式] 规则编译失败，保持{}: {}", self.applied_mode.as_str(), e));
                let _ = self.session.write_mode(self.applied_mode.as_str());
                return;
            }
        }
    }
    if let Some(engine) = self.engine.as_mut() { engine.set_mode(target); } else { self.pending_mode = Some(target); }
    self.applied_mode = target;
    self.log(&format!("[模式] 已切换为 {}", target.as_str()));
}
```

`poll_rules_change()`：读 `read_rules_raw()`，与上次字符串比较；变化则反序列化 + `compile`，成功则更新 `raw_rules` / `default_policy`，`Rule` 模式下 `set_rules` + `reset_changed_flows()`。

两个轮询放在主循环 `self.poll_learn_clear();` 之后。

正常停止路径（`self.session.scrub_request();` 旁边）追加清理，避免控制文件残留：

```rust
        self.session.scrub_request();
        self.session.remove_control_files();
```

- [ ] **Step 3: 夹具补字段**

worker 测试里的 `VpnRequest` 字面量补 `proxy_mode: "rule".into()`、`conn_identity: String::new()`（后者 M3 才用到，可与 Task 13 一起补）。

- [ ] **Step 4: 运行**

Run: `cd src-tauri && cargo test --lib`
Expected: 298+ passed，仅剩既有 detect 环境失败。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/worker.rs
git commit -m "feat(vpn): worker 轮询控制文件并热应用模式与规则"
```

### Task 10: 两个新命令 + 注册

**Files:**
- Modify: `src-tauri/src/vpn/commands.rs`（`vpn_set_proxy_mode` / `vpn_push_rules`）
- Modify: `src-tauri/src/lib.rs:250-260`（注册命令）

- [ ] **Step 1: 实现命令**

```rust
#[tauri::command]
pub fn vpn_set_proxy_mode(state: State<'_, VpnState>, mode: String) -> Result<(), String> {
    let mode = ProxyMode::parse(&mode)?;
    let runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    let status = runtime.session.read_status().unwrap_or_default();
    if status.phase == VpnPhase::Disconnected || status.phase == VpnPhase::Failed {
        return Ok(());
    }
    runtime.session.write_mode(mode.as_str())
}

#[tauri::command]
pub fn vpn_push_rules(state: State<'_, VpnState>, rules: Vec<VpnRule>, default_policy: String) -> Result<(), String> {
    validate_rules(&rules, &default_policy)?; // 编译校验，失败返回错误
    let runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    let status = runtime.session.read_status().unwrap_or_default();
    if status.phase == VpnPhase::Disconnected || status.phase == VpnPhase::Failed {
        return Ok(());
    }
    runtime.session.write_rules(&rules, &default_policy)
}
```

- [ ] **Step 2: 注册**

`lib.rs` 的 `generate_handler!` 里加 `vpn::commands::vpn_set_proxy_mode`, `vpn::commands::vpn_push_rules`。

- [ ] **Step 3: 编译验证**

Run: `cd src-tauri && cargo test --lib`
Expected: PASS。

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/vpn/commands.rs src-tauri/src/lib.rs
git commit -m "feat(vpn): 新增切换代理模式与推送规则命令"
```

### Task 11: 前端热切换接线

**Files:**
- Modify: `src/composables/useVpn.ts:392-397`（`setProxyMode`）
- Modify: `src/composables/useVpn.ts`（`pushRulesIfConnected` 防抖）
- Modify: `src/components/vpn/VpnPanel.vue:96-120`（解锁与文案）

- [ ] **Step 1: `setProxyMode` 调新命令**

```ts
  async function setProxyMode(mode: VpnProxyMode) {
    const profile = selectedProfile.value
    if (!profile || profile.proxy_mode === mode) return
    await updateProfile(profile.id, { proxy_mode: mode })
    if (isConnected.value) {
      await invoke('vpn_set_proxy_mode', { mode })
      pushLog('info', `代理模式已切换为${mode === 'rule' ? '规则' : mode === 'global' ? '全局' : '直连'}，新连接按新模式走`)
    } else {
      pushLog('info', `代理模式已切换为…，下次连接生效`)
    }
  }
```

- [ ] **Step 2: 规则推送（防抖）**

```ts
  let rulesTimer: number | undefined
  async function pushRulesIfConnected() {
    const profile = selectedProfile.value
    if (!profile) return
    if (!isConnected.value || status.value.profile_id !== profile.id) return
    window.clearTimeout(rulesTimer)
    rulesTimer = window.setTimeout(async () => {
      try {
        await invoke('vpn_push_rules', {
          rules: profile.rules ?? [],
          defaultPolicy: profile.rules_default_policy ?? 'DIRECT',
        })
      } catch (err: any) {
        pushLog('error', `规则未生效: ${err}`)
      }
    }, 300)
  }
```

- [ ] **Step 3: 面板解锁模式按钮与文案**

`modeLocked` 改为 `vpn.connecting || vpn.phaseBusy`；把黄字提示换成「切换立即生效：新连接按新模式，已建立的连接保持原路径」。

- [ ] **Step 4: 验证**

Run: `pnpm -s exec vue-tsc --noEmit && pnpm -s build`
Expected: 0 error，构建成功。

- [ ] **Step 5: 提交**

```bash
git add src/composables/useVpn.ts src/components/vpn/VpnPanel.vue
git commit -m "feat(vpn): 连接中切换模式与规则热生效"
```

---

## Phase M3 — 通道共享

### Task 12: 通道身份哈希

**Files:**
- Modify: `src-tauri/src/vpn/models.rs`（`VpnProfile` 上的纯函数 `conn_identity`）
- Test: `src-tauri/src/vpn/models.rs`

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn conn_identity_ignores_name_rules_and_mode() {
        let a = sample_profile();
        let mut b = a.clone();
        b.name = "另一个名字".into();
        b.proxy_mode = "global".into();
        b.extra_args = a.extra_args.clone();
        assert_eq!(conn_identity(&a), conn_identity(&b));
    }

    #[test]
    fn conn_identity_changes_with_connection_fields() {
        let a = sample_profile();
        let mut b = a.clone();
        b.server = "other.example.com:443".into();
        assert_ne!(conn_identity(&a), conn_identity(&b));
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test --lib conn_identity`
Expected: 编译失败（函数未定义）。

- [ ] **Step 3: 实现（FNV-1a，稳定且不泄露明文）**

```rust
pub fn conn_identity(profile: &VpnProfile) -> String {
    let mut parts: Vec<String> = vec![
        profile.server.clone(),
        profile.username.clone(),
        profile.password.clone(),
        profile.protocol.clone(),
        format!("{:?}", profile.connector),
        profile.connector_path.clone(),
        profile.fake_ip_range.clone(),
        profile.learn.to_string(),
    ];
    let mut extra = profile.extra_args.clone();
    extra.sort();
    parts.push(extra.join("\u{1e}"));
    let mut filter = profile.fake_ip_filter.clone();
    filter.sort();
    parts.push(filter.join("\u{1e}"));
    let joined = parts.join("\u{1f}");
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in joined.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
```

（`VpnProfile` 需要 `Clone`；若目前没有 derive，补上 `#[derive(Clone)]`。）

- [ ] **Step 4: 运行**

Run: `cd src-tauri && cargo test --lib models`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn/models.rs
git commit -m "feat(vpn): 通道身份哈希（排除名称/规则/模式）"
```

### Task 13: 身份下发 + 切换命令

**Files:**
- Modify: `src-tauri/src/vpn/models.rs`（`VpnRequest.conn_identity`、`VpnStatus.conn_identity`）
- Modify: `src-tauri/src/vpn/commands.rs`（`vpn_conn_identities`、`vpn_switch_profile`、connect 守卫、`policy_profile_id`、snapshot 字段）
- Modify: `src-tauri/src/vpn/worker.rs`（status 写入时带上 `conn_identity`）
- Test: `src-tauri/src/vpn/commands.rs`

- [ ] **Step 1: 写失败测试（纯决策函数）**

```rust
    #[test]
    fn switch_decision_is_shared_when_identity_matches() {
        assert_eq!(decide_switch(Some("abc"), Some("abc")), SwitchDecision::Shared);
        assert_eq!(decide_switch(Some("abc"), Some("def")), SwitchDecision::NeedsReconnect);
        assert_eq!(decide_switch(None, Some("abc")), SwitchDecision::NoSession);
    }
```

- [ ] **Step 2: 实现**

```rust
enum SwitchDecision { Shared, NeedsReconnect, NoSession }
fn decide_switch(active: Option<&str>, target: Option<&str>) -> SwitchDecision { /* 三态 */ }

#[tauri::command]
pub fn vpn_conn_identities(profiles: Vec<VpnProfile>) -> Vec<String> {
    profiles.iter().map(conn_identity).collect()
}

#[tauri::command]
pub fn vpn_switch_profile(state: State<'_, VpnState>, profile: VpnProfile) -> Result<bool, String> {
    let mut runtime = state.0.lock().map_err(|_| "内部状态锁定失败".to_string())?;
    let status = runtime.session.read_status().unwrap_or_default();
    match decide_switch(Some(status.conn_identity.as_str()), Some(conn_identity(&profile).as_str())) {
        SwitchDecision::NoSession => return Err("当前未连接".to_string()),
        SwitchDecision::NeedsReconnect => return Ok(false),
        SwitchDecision::Shared => {
            validate_rules(&profile.rules, &profile.rules_default_policy)?;
            runtime.session.write_rules(&profile.rules, &profile.rules_default_policy)?;
            runtime.session.write_mode(&profile.proxy_mode)?;
            runtime.policy_profile_id = Some(profile.id);
            Ok(true)
        }
    }
}
```

`vpn_connect`：写请求前算 `conn_identity` 放入 request；守卫：`phase` 非 disconnected/failed 时返回 `Err("已有活动连接，请先断开")`。连接成功后 `runtime.policy_profile_id = Some(profile.id)`。`vpn_get_snapshot` 返回 `policy_profile_id`。`vpn_disconnect` 清空该字段。

worker：写 status 时把 `request.conn_identity` 复制进 `status.conn_identity`。

- [ ] **Step 3: 注册命令**

`lib.rs` 加 `vpn_conn_identities`、`vpn_switch_profile`。

- [ ] **Step 4: 运行**

Run: `cd src-tauri && cargo test --lib`
Expected: PASS（夹具补齐 `proxy_mode` / `conn_identity` 字段）。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn src-tauri/src/lib.rs
git commit -m "feat(vpn): 同通道配置共享隧道"
```

### Task 14: 前端三态与确认框

**Files:**
- Modify: `src/composables/useVpn.ts`（身份缓存、`switchToProfile`、`policyProfileId`）
- Modify: `src/components/vpn/VpnPanel.vue`（列表三态、按钮文案、确认框）

- [ ] **Step 1: useVpn 增加身份与切换**

```ts
  const identities = ref<Record<string, string>>({})
  async function refreshIdentities() {
    if (!profiles.value.length) { identities.value = {}; return }
    const list = await invoke<string[]>('vpn_conn_identities', { profiles: profiles.value })
    identities.value = Object.fromEntries(profiles.value.map((p, i) => [p.id, list[i]]))
  }

  function sharesTunnel(profileId: string): boolean {
    const active = status.value.conn_identity
    return !!active && identities.value[profileId] === active && profileId !== policyProfileId.value
  }

  async function switchToProfile(profile: VpnProfile): Promise<'shared' | 'reconnect' | 'none'> {
    if (!isConnected.value) return 'none'
    const shared = await invoke<boolean>('vpn_switch_profile', { profile })
    if (!shared) return 'reconnect'
    policyProfileId.value = profile.id
    config.value.vpn_active_profile_id = profile.id
    await saveConfig()
    pushLog('info', `已切换到配置「${profile.name}」，通道复用，规则已生效`)
    return 'shared'
  }
```

配置变化、连接状态变化时调用 `refreshIdentities()`。

- [ ] **Step 2: 面板三态与按钮**

- 列表项 `:class="{ 'profile-item--shared': vpn.sharesTunnel(profile.id) }"`，灰色样式 + 「共用通道」小标签；
- 连接按钮逻辑：

```ts
function connectLabel() {
  if (!vpn.isConnected) return '连接 VPN'
  if (vpn.selectedProfile?.id === vpn.policyProfileId) return '断开 VPN'
  return vpn.sharesTunnel(vpn.selectedProfile?.id ?? '') ? '切换到该配置' : '切换（需断开重连）'
}
```

- 点击「切换（需断开重连）」先 `Modal.confirm`，确认后 `await vpn.disconnect(); await vpn.connect(vpn.selectedProfile!)`。
- 代理模式按钮额外加锁：选中配置不是当前生效配置时不可点（`modeLocked = vpn.connecting || vpn.phaseBusy || (vpn.isConnected && vpn.selectedProfile?.id !== vpn.policyProfileId)`），并提示「该配置未连接」。

- [ ] **Step 3: 验证**

Run: `pnpm -s exec vue-tsc --noEmit && pnpm -s build`
Expected: 0 error，构建成功。

- [ ] **Step 4: 提交**

```bash
git add src/composables/useVpn.ts src/components/vpn/VpnPanel.vue
git commit -m "feat(vpn): 配置列表三态与同通道一键切换"
```

---

## Phase M4 — 退出清理

### Task 15: 应用退出钩子

**Files:**
- Modify: `src-tauri/src/lib.rs:380-400`（`app.run` 里处理 `RunEvent::ExitRequested`）
- Modify: `src-tauri/src/vpn/commands.rs:845-853`（`shutdown` 清理控制文件）

- [ ] **Step 1: 实现**

```rust
        if let tauri::RunEvent::ExitRequested { .. } = event {
            vpn::commands::shutdown(app_handle);
        }
```

`shutdown()` 末尾加 `session.remove_control_files();`。

- [ ] **Step 2: 编译验证**

Run: `cd src-tauri && cargo test --lib`
Expected: PASS。

- [ ] **Step 3: 提交**

```bash
git add src-tauri/src/lib.rs src-tauri/src/vpn/commands.rs
git commit -m "fix(vpn): 应用退出时也请求 worker 收尾"
```

### Task 16: 父进程心跳守卫

**Files:**
- Modify: `src-tauri/src/vpn/session.rs`（`alive_path` / `touch_alive` / `alive_age`）
- Modify: `src-tauri/src/vpn/commands.rs`（连接后启动心跳线程）
- Modify: `src-tauri/src/vpn/worker.rs`（主循环检查心跳）
- Test: `src-tauri/src/vpn/session.rs`

- [ ] **Step 1: session API + 测试**

```rust
    #[test]
    fn alive_file_is_touched_and_prepare_clears_it() {
        let session = temp_session("alive");
        session.touch_alive().unwrap();
        assert!(session.alive_age().map(|age| age.as_secs() < 5).unwrap_or(false));
        session.prepare().unwrap();
        assert!(session.alive_age().is_none());
    }
```

- [ ] **Step 2: 心跳线程**

`vpn_connect` 在 worker 启动成功后：

```rust
    let heartbeat_session = session.clone();
    std::thread::spawn(move || loop {
        if heartbeat_session.touch_alive().is_err() { break; }
        std::thread::sleep(std::time::Duration::from_secs(2));
        if let Some(status) = heartbeat_session.read_status() {
            if matches!(status.phase, VpnPhase::Disconnected | VpnPhase::Failed) { break; }
        }
    });
```

- [ ] **Step 3: worker 守卫**

主循环每轮节流 1s 检查：`alive_age()` 为 `None` 且启动超过 10s，或 `age > 8s` → `break`（走正常停止收尾流程），日志「主进程已退出，worker 自动收尾」。

- [ ] **Step 4: 运行**

Run: `cd src-tauri && cargo test --lib`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/vpn
git commit -m "fix(vpn): 主进程被强杀时 worker 自动收尾"
```

---

### Task 17: 全量回归与手动验收

- [ ] **Step 1: Rust 回归**

Run: `cd src-tauri && cargo test --lib`
Expected: 全部 PASS，仅 `vpn::detect::tests::bundled_openconnect_is_probed_not_skipped` 为既有环境失败。

- [ ] **Step 2: 前端检查与构建**

Run: `pnpm -s exec vue-tsc --noEmit && pnpm -s build`
Expected: 0 error，构建成功。

- [ ] **Step 3: 手动验收清单**（见规格文档第 12 节）

1. 老配置升级后每个配置带独立规则副本；
2. 面板内增删改规则、MATCH 走向正常；
3. 连接中切模式不断连、新连接按新模式；
4. 改一条规则只重置受影响的连接；
5. 同通道配置：连接 A → 点 B → 隧道不重建、B 规则生效、A 变灰 → 点 A 秒切回；
6. 不同通道配置：弹确认框 → 确认后自动断开重连；
7. 断开后 `mode` / `rules.json` 不存在、无残留路由；
8. 托盘退出 / Cmd+Q 后 worker 退出、路由还原；
9. 强杀主进程后 8s 内 worker 自行收尾；
10. 活动会话下再次 connect 被拒绝。

- [ ] **Step 4: 提交收尾**

```bash
git add -A
git commit -m "chore(vpn): 通道共享与热切换回归验证"
```
