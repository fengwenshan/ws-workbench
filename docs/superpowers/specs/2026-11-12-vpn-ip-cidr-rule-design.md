# VPN IP 规则（恢复 `IP-CIDR`）：内网网站用 IP 访问也能走隧道

- 日期：2026-11-12
- 范围：`src-tauri/src/vpn`（rule_engine / split / route 接线 / worker / commands）与 VPN 面板；
  不涉及服务端与 DNS 配置
- 现场：公司内网 OA 是 `https://192.168.6.115:51006/oa/web`，只能按 IP 访问。
  规则模式只有 `DOMAIN` / `DOMAIN-SUFFIX` 两类，用户把 IP 写进 `DOMAIN-SUFFIX`
  能保存、能写 `/etc/resolver/192.168.6.115`，但永远不生效
  （自检日志：`192.168.6.115 解析到 192.168.6.115（还没见到隧道路由）`）。

## 1. 结论

恢复 `IP-CIDR` 为**独立规则类型**，与域名规则并列：

- 取值支持单个 IP（自动按 `/32`）与 CIDR 网段；
- 走向支持 `VPN` / `DIRECT`；
- 判定与域名规则一致：**最长前缀优先，前缀相同时先出现的规则胜出**，未命中走直连；
- IP 规则**不依赖 DNS**：连接完成后立即装好路由，不需要先访问一次域名；
- 纯 IP 规则（没有域名规则）时**不起 DNS 转发器、不写 `/etc/resolver`、不写 NRPT、
  不改系统 DNS**，只装路由；
- `DIRECT` 例外用「更具体的直连路由（走原默认网关）」表达，由内核最长前缀匹配裁决。

`IP-CIDR` 的 kind 字符串沿用旧值，老配置与导入导出不需要迁移。

## 2. 背景与根因

2026-09 的捕获式引擎曾支持 `IP-CIDR`；2026-11 改成路由注入后下线，理由是
「整段注入做不到按网段排除，与其半生效不如不做」（见 `rule_engine.rs` 与
`split.rs` 模块文档）。

根因不是「IP 规则没法实现」，而是**注入时机**：现在的 `/32` 隧道路由只在
本机转发器**看到 DNS 应答**时注入。IP 字面量不会产生 DNS 查询，转发器永远
看不到应答，所以以任何「域名规则」形式写 IP 都不会生效。规则模式本身已经
拥有「装固定隧道路由」的能力（VPN DNS 的 `/32` 走的就是 `static_routes`），
把命中的 IP 网段接进同一条链路即可。

「按网段排除」这次用路由表本身表达：VPN 网段装隧道路由，例外装更具体的
直连路由，最长前缀匹配自动裁决，不需要在 plan 里做区间裁剪。

## 3. 目标 / 非目标

目标：

1. `IP-CIDR` 规则在 macOS 与 Windows 的规则模式生效；
2. 支持单 IP 与网段，支持 VPN / DIRECT；
3. 校验、预览、状态面板、日志、热更新、断开回收、崩溃清理全链路覆盖；
4. 纯 IP 规则不碰系统 DNS 与默认路由，保持「结构上不会一连上就断网」的边界。

非目标：

- 不恢复 `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `RULE-SET`；
- 不支持 `0.0.0.0/0`（那是全局模式，规则里写 `/0` 直接拒绝）；
- 不做「IP + 端口」维度（端口仍不参与判定）；
- 不改变「服务端下发的 split-include 网段只展示、不并入」的严格规则模式边界。

## 4. 规则语义

### 4.1 取值与归一

- `192.168.6.115` → `192.168.6.115/32`
- `192.168.6.0/24` → 归一为 `192.168.6.0/24`（`route::parse_cidr` + `normalize_cidr`，
  主机位会被抹掉）
- `0.0.0.0/0` → 校验报错：`IP 规则不支持 0.0.0.0/0，需要全部流量走 VPN 请用全局模式`
- IPv6、非法前缀（`/33` 等）、空值 → 报错，报错文案带原值；IP 规则只支持 IPv4
- `vpn_validate_rule` 对 IP 规则返回**归一后的值**（单 IP 补成 `/32`），前端保存时用它

归一函数：`split::normalize_ip_rule(value) -> Result<String, String>`，
校验（`commands::validate_rule_input`）与计划编译（`split::plan`）共用它，
避免「校验通过、连接时被跳过」的两套逻辑。

### 4.2 判定

- 最长前缀优先（`/24` 盖住 `/16`）；
- 前缀长度相同时按规则顺序，**先出现的胜出**；
- 没有命中任何 IP 规则 → `DIRECT`（与域名规则一致；兜底策略仍不参与路由注入）。

示例：

| 规则（按顺序） | 目标 | 结果 |
| --- | --- | --- |
| `192.168.6.0/24 → VPN` | `192.168.6.115` | 进隧道 |
| `192.168.6.0/24 → VPN`，`192.168.6.9/32 → DIRECT` | `192.168.6.9` | 直连 |
| `192.168.6.0/24 → VPN`，`192.168.6.9/32 → DIRECT` | `192.168.6.10` | 进隧道 |
| `192.168.6.0/16 → DIRECT`，`192.168.6.0/24 → VPN` | `192.168.6.115` | 进隧道（更长前缀胜出） |
| 两条同 `/24` 规则，一 VPN 一 DIRECT | 该网段 | 先出现的那条胜出，另一条进 `notes` |

### 4.3 预览

`vpn_preview_rules` 的参数从 `domain` 改为 `target`（前后端同步改），
输入是单个 IP 或域名：

- 能解析成 IP / CIDR → 用 `SplitPlan::ip_tunnels` 判定（输入是 CIDR 时按网络地址判定）；
- 否则按域名判定（现有 `SplitPlan::tunnels`）；
- 两者都不合法 → 返回「请提供要预览的域名或 IP」。

预览只按规则判定，§6 里依赖本机环境（本机网段、物理网关、服务器 IP）的跳过项
不在预览里体现，由连接时的日志与 `notes` 给出原因——这与现有域名预览的边界一致。

## 5. 编译与数据模型（`split.rs`）

```rust
/// 一条 IP 规则的数据面语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRule {
    pub network: u32, // 归一后的网络地址
    pub prefix: u8,
    pub policy: Policy,
}

impl IpRule {
    pub fn cidr(self) -> String; // "192.168.6.0/24"
}

pub struct SplitPlan {
    pub rules: Vec<DomainRule>,
    pub domains: Vec<String>,
    pub ip_rules: Vec<IpRule>, // 新增
    pub notes: Vec<String>,
}

impl SplitPlan {
    pub fn tunnels(&self, name: &str) -> bool;              // 现有
    pub fn ip_tunnels(&self, ip: std::net::Ipv4Addr) -> bool; // 新增：最长前缀 + 先到先得
}
```

纯函数编译「要装哪些路由」，便于不起 root 做单测：

```rust
pub struct IpRoutePlan {
    /// 要装到隧道接口上的网段（含 /32）
    pub tunnel: Vec<String>,
    /// 要装到原默认网关上的直连例外网段
    pub direct: Vec<String>,
    pub notes: Vec<String>,
}

pub fn ip_route_plan(
    plan: &SplitPlan,
    snapshot: &models::NetworkSnapshot,
    server_ips: &[std::net::Ipv4Addr],
    vpn_dns: &[std::net::Ipv4Addr],
) -> IpRoutePlan;
```

规则：

- 同 `network` + `prefix` 的规则只取**先出现**的一条（与 §4.2 判定语义一致），
  后出现的写进 `notes`；
- `tunnel`：所有 `policy = VPN` 的网段，去重，并按 §6 决策表剔除冲突项；
- `direct`：只保留「比覆盖它的 VPN 网段更具体」的 `DIRECT` 网段——比 VPN 网段更宽的
  直连规则不产生路由（写出去也只是回到默认路由），落在本机网段内的也跳过
  （系统 on-link 路由本来就是直连），避免无意义的表项和误删别人的路由；
- `notes`：`/0`、冲突、跳过原因等逐条记录，由 worker 写进日志并合并进
  `SplitPlan::notes` 的展示链路。

## 6. 安全边界（决策表）

| 情形 | 决策 |
| --- | --- |
| 规则是 `0.0.0.0/0` | 校验/编译时报错，提示用全局模式 |
| 规则网段恰为 VPN 服务器 IP 的 `/32` 或 VPN DNS 的 `/32` | 跳过该条 + `notes`（与已有主机路由同前缀，装不上也不该装） |
| 规则网段包含 VPN 服务器 IP（如 `/16` 盖住服务器） | 正常装隧道网段；脚本已有的服务器 `/32` 主机路由更具体，服务器流量仍走物理网卡，不会打环 |
| 规则网段包含本机所在网段（如 `192.168.0.0/16` 盖住本机 `192.168.1.0/24`） | 正常装隧道网段；系统已有的 on-link `/24` 更具体，局域网访问不受影响 |
| 规则网段恰好等于本机所在网段 | 跳过该条 + `notes`（与 on-link 路由同前缀冲突） |
| 规则网段落在本机所在网段内部（`/25` 在 `/24` 里） | 跳过该条 + `notes`（会部分劫持局域网） |
| `DIRECT` 例外同时落在隧道网段内 | 装直连路由（原默认网关），由最长前缀匹配生效 |
| `DIRECT` 例外落在本机网段内 | 跳过（系统 on-link 路由已直连），不装、也不在断开时删别人的路由 |
| 会话快照里没有物理默认网关（`snapshot.default_gateway` 为空） | 跳过所有直连例外 + `notes`（没有可用的直连下一跳） |

**不做**的隐式行为：不因为规则覆盖某处就改 DNS、不接管默认路由、不动系统解析器；
服务端下发的网段仍然只展示。

## 7. 运行期接线

### 7.1 `SplitTunnel::apply`

- 入参新增 `snapshot: &models::NetworkSnapshot`；
- 早退条件从「`plan.domains` 为空」改成「`domains` 与 `ip_rules` 都为空」，
  文案改为：`规则里没有可用的分流动作：请加一条「域名后缀」或「IP / 网段」规则`；
- 调 `ip_route_plan()`，`notes` 写日志：`[分流] IP 规则：192.168.6.0/24 → 隧道 utun4`；
- 隧道网段并入现有的 `static_routes`：复用 `add_tunnel_route` / `guard()` 补装 /
  `teardown()` 回收，不新增第二套账本；
- 直连例外装到「原默认网关」：`RouteTarget { if_index: snapshot.default_if_index,
  if_name: snapshot.default_if_alias, next_hop: snapshot.default_gateway }`，
  用 `route::ensure_route` / `route::delete_route`；记录到新字段
  `direct_routes: Vec<(String, RouteTarget)>`；
- **纯 IP 规则时跳过**：`install_resolver` / `start_resolver` / `spawn_self_check`，
  也不再要求 `vpn_dns` 非空（服务端没下发 DNS 也能用）；
- `routes_for_status()` 同时列出隧道网段（`via = "vpn"`）与直连例外
  （`via = "local"`），面板「当前生效的分流」能直接对照。

### 7.2 热更新、模式切换、断开

三个入口都走现有 `restart_split` / `switch_engine` 的「先 teardown 再重装」，
无需新增路径：

- 规则热更新（`apply_reloaded_rules`）→ `restart_split` → 旧隧道网段与直连例外一起删、
  按新计划重装；
- 切到全局/直连模式 → 走 `teardown()`，直连例外一并删除；
- 断开 → `teardown()`；
- `guard()` 在规则模式下周期性补装丢失的路由：隧道网段沿用现有逻辑，
  直连例外同样补装（`route_exists` 判存在性）。

### 7.3 崩溃与换网残留

- 隧道网段随 utun 消失，无需额外清理；
- **直连例外**装在物理网卡上，SIGKILL 后会残留。换到别的网络后旧网关不可达，
  残留路由会让那个网段「明明该直连却不通」。处理：
  - 会话目录新增 `direct-routes.txt`，逐行记录 `cidr|gateway|if_index|if_name`；
  - `cleanup_stale()` 在 worker 启动与每次 `apply` 前读取该清单，逐条删除
    （macOS 用 `route -n delete -net <cidr> <gateway>`，Windows 用
    `Remove-NetRoute -DestinationPrefix`），然后删除清单；
  - `teardown()` 成功后清空清单；正常断开时清单为空，换网不会踩到残留。

## 8. UI 与状态

- `src/types/vpn.ts`：`VpnRuleKind` 增加 `'IP-CIDR'`；从 `RETIRED_RULE_KINDS` 移除
  `'IP-CIDR'`；补 kind → 中文标签映射（`IP / 网段`）。
- `src/components/vpn/VpnRuleDialog.vue`：
  - 下拉新增 `IP / 网段 (IP-CIDR)`；
  - 输入占位随类型切换：域名 → `example.com`；IP → `192.168.6.115 或 192.168.6.0/16`；
  - 保存时使用后端返回的规范化值（单 IP 会补成 `/32`）；
  - **非阻断提示**：类型是 `DOMAIN` / `DOMAIN-SUFFIX` 但值能解析成 IPv4 时，
    提示「这看起来是 IP，建议改用「IP / 网段」，域名规则填 IP 不会生效」——不阻止保存，
    兼容历史配置。
- 预览输入（`useVpn.previewRule` 的调用方）：标签从「域名」改为「域名或 IP」，
  参数名 `domain` → `target`。
- 状态面板（`VpnRoutePreview.vue`）：路由表的出口列把 `vpn` 显示为「VPN」、
  `local` 显示为「直连」；不新增区块。
- `README.md` 的 VPN 规则章节：规则类型表补 `IP-CIDR`，写明「IP 规则不走 DNS、
  连接后立即生效」「DIRECT 例外用更具体的直连路由表达」。
- 历史规则：之前显示「不生效」的 `IP-CIDR` 规则从本版本起生效；
  `DOMAIN-SUFFIX` 里写 IP 的旧写法仍不生效，只给提示。

## 9. 测试矩阵

后端单测（不起 root，全部纯函数/模型层）：

- `rule_engine.rs`：`IP-CIDR` 解析成功、`as_str()` 往返、已下线名单里不再包含它；
  `DOMAIN-KEYWORD` 等仍报错。
- `split.rs::plan`：单 IP → `/32`；CIDR 归一（抹主机位）；非法值进 `notes`；
  `/0` 拒绝；同前缀先到先得；IP + 域名混合；`ip_tunnels` 判定（最长前缀、等长先到）。
- `split.rs::ip_route_plan`：服务器 `/32` 精确冲突跳过；VPN DNS `/32` 精确冲突跳过；
  盖住服务器 IP 的正常装；等于本机网段跳过；落在本机网段内跳过；含本机网段的正常装；
  `DIRECT` 例外仅在更具体时产出；无物理默认网关时跳过并给 note。
- `commands.rs`：`validate_rule_input("IP-CIDR", ...)` 的合法/非法用例；
  `preview_verdict` 对 IP、CIDR、域名三种输入；替换现有
  `retired_ip_cidr_is_rejected_everywhere`（改为「已生效」断言）。
- `worker.rs`：`rules.json` 里只有 IP 规则时 `rule_plan()` 产出 `ip_rules`、
  `domains` 为空；「纯 IP 规则不起解析器」由 `apply` 的 `domains.is_empty()` 分支保证，
  不引入新的会话桩，真机验收第 5 项覆盖。

真机验收（macOS + Windows 各跑一遍）：

1. 加 `192.168.6.0/24 → VPN`，连接后 `route -n get 192.168.6.115` 指向 `utun*`，
   `https://192.168.6.115:51006/oa/web` 可打开；
2. 断开后该路由消失，本机局域网（`192.168.1.0/24`）访问不受影响；
3. 热更新：连接中新增/删除 IP 规则，路由立即出现/消失；
4. `192.168.6.0/24 → VPN` + `192.168.6.9/32 → DIRECT`：`.9` 走原网关，其余进隧道；
5. 纯 IP 规则时不产生 `/etc/resolver` / NRPT 规则，系统 DNS 不变；
6. Windows 用 `Get-NetRoute -DestinationPrefix 192.168.6.0/24` 确认接口是 Wintun、
   NextHop 为 `0.0.0.0`；直连例外 NextHop 是物理网关；
7. 拔掉/切换网络后重新连接，旧直连例外清单被 `cleanup_stale` 清掉。

前端：`pnpm build`（`vue-tsc --noEmit`）通过；规则弹窗、预览、状态面板手动过一遍。

## 10. 兼容与发布

- 规则 JSON 格式不变（`kind` 仍是 `IP-CIDR`），不需要迁移；
- 历史 `IP-CIDR` 规则从「不生效」变为「生效」：发布说明里明确写出，
  并提示「如果之前留着一条只想做占位的 IP-CIDR 规则，请确认它该走 VPN 还是删除」；
- 不改变 `DOMAIN` / `DOMAIN-SUFFIX` 的既有语义与直连例外规则；
- 版本号由 `scripts/release.mjs` 统一处理，本次不手改。

## 11. 风险

| 风险 | 处理 |
| --- | --- |
| 与 Clash TUN / 其它 VPN 抢路由 | 隧道网段由 `guard()` 补装；不新增对抗逻辑，日志里能看到重复注入 |
| 直连例外在崩溃后残留到别的网络 | 7.3 的 `direct-routes.txt` 清单 + `cleanup_stale` |
| 规则写宽（`/16`）误伤本机网段 | 6 的决策表：包含本机网段正常装（on-link 更具体），等于/落在内则跳过并提示 |
| Windows 直连例外 NextHop 用法 | 验收 6 明确检查 `NextHop`；实现时用 `Get-NetRoute` 断言，失败只跳过该条并记日志，不影响隧道 |
| 用户以为 IP 规则也会改 DNS | README 与日志写明「IP 规则不碰解析器」 |

## 12. 涉及文件

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/vpn/rule_engine.rs` | 增加 `RuleKind::IpCidr` 与解析/文案/单测 |
| `src-tauri/src/vpn/models.rs` | `VpnRule` 注释更新；沿用现有 `NetworkSnapshot` / `AppliedRoute` |
| `src-tauri/src/vpn/split.rs` | `IpRule`、`SplitPlan.ip_rules`、`normalize_ip_rule`、`ip_tunnels`、`ip_route_plan`、`SplitTunnel` 直连路由与清理、模块文档 |
| `src-tauri/src/vpn/route.rs` | 复用现有 `parse_cidr` / `normalize_cidr` / `add_route` / `ensure_route` / `delete_route`，不新增 API |
| `src-tauri/src/vpn/commands.rs` | `validate_rule_input`、`vpn_validate_rule` 返回归一值、`preview_verdict`、`vpn_preview_rules` 参数、单测 |
| `src-tauri/src/vpn/worker.rs` | `start_split_rules` 传 snapshot / 日志与文案、`connected_message`、纯 IP 规则路径 |
| `src/types/vpn.ts` | `VpnRuleKind`、`RETIRED_RULE_KINDS`、标签映射 |
| `src/components/vpn/VpnRuleDialog.vue` | 类型下拉、占位符、提示、规范化保存、规则列表的类型标签 |
| `src/composables/useVpn.ts` | 预览参数 `domain` → `target`；`usableRules` 的「已下线」注释更新（IP-CIDR 不再剔除） |
| `src/components/vpn/VpnRoutePreview.vue` | 出口列显示 VPN / 直连 |
| `README.md` | VPN 规则章节补充 IP 规则语义 |
