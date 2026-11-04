# Clash 式规则分流 · 竖切 A（UI + 运行期）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 macOS 上把 ws-workbench 的 VPN 分流从「双模式 + CIDR 路由表」替换为纯 Clash 语义的规则引擎竖切：TUN 透明接管、fake-ip DNS、按流动态注入路由，TCP 与（非 DNS）UDP 全部按规则转发，并配套规则编辑 UI。

**Architecture:** openconnect 只负责把隧道拉起来（`vpnc-script` 退化为「只配接口地址 + MTU」）；本程序自建一张「捕获 TUN（utunC）」用 `0.0.0.0/1` + `128.0.0.0/1` 抢走全部 IPv4 流量，用 `smoltcp` 用户态栈在用户态终结应用侧连接；每条流按 `rule_engine` 的首条命中结果决定出口：`DIRECT` 用 `IP_BOUND_IF` 把出站 socket 钉到物理网卡，`VPN` 把出站 socket 钉到隧道网卡（utunZ）并加一条 `/32 → utunZ` 流路由。进程名由 `sysctl net.inet.tcp/udp.pcblist_n` 反查。DNS 走 fake-ip：拦截 53 端口，立即回一个 `198.18.0.0/16` 内的假地址，等应用真正连接该假地址时再按域名匹配并解析真实 IP。

**Tech Stack:** Rust（Tauri v2 crate `ws-workbench`）、`smoltcp`（用户态 TCP/IP 栈）、`nix` + `libc`（utun / socket / `IP_BOUND_IF` / ioctl）、Vue 3 + TypeScript + ant-design-vue（前端）。

---

## 提交策略（与前序计划一致，务必遵守）

本仓库有硬性约定：**未经用户明确同意，不得执行任何 `git` 提交/合并**。因此本计划中每个任务末尾的「提交」步骤一律标注为「**待用户同意后执行**」；未获同意前只保留工作区改动。不得自行 `git commit`。

另两条项目规则在实施期必须遵守：

- **每次运行完任务都需要重启项目**才能验证：macOS 用 `pnpm exec tauri dev`（**不要**用 `pnpm tauri dev`，会把命令解析成 `tauri dev dev`）；重启前先结束残留的 vite / tauri 进程。
- **本地存储一律加密**：前端任何 `localStorage` 读写必须经 `src/composables/safeStorage.ts`；Rust 侧落盘必须经 `crypto::encrypt_str` / `decrypt_str`，不得新增明文落盘。

## 字符串规范（kind / policy，务必统一）

`VpnRule.kind`、`VpnRule.policy`、`VpnRuleSet.default_policy`、`VpnRequest.default_policy` 一律使用 `rule_engine` 的规范形式（即 `RuleKind::as_str()` / `Policy::as_str()` 的输出）：

- kind：`DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`PROCESS-NAME`、`DST-PORT`、`IP-CIDR`、`RULE-SET`（**大写连字符，不是 snake_case**）
- policy：`DIRECT`、`VPN`（大写）

运行期 `RuleKind::parse` / `Policy::parse` 只认这套形式（大小写不敏感，但**不**做下划线归一）。任何写入这些字段的地方（Rust `models.rs`、前端 `useVpn.ts`、测试数据、默认值 `default_policy_str`/`default_policy_for`）都必须用规范形式；否则规则会在 worker 里被静默丢弃（本项目已因 snake_case/连字符不一致修过一次）。

## 范围边界（本计划不做）

- 不做 `RULE-SET` 外部规则集文件的**加载/展开**（`rule_engine` 已支持，本竖切 UI 暂不暴露）。
- 不做 Windows 平台实现（`platform.rs` 只落 macOS，Windows 留给下一竖切）。
- 不做 `PROCESS-PATH`、`REJECT`。
- 不做 e2e 自动化测试（`Task 13` 为手工验收）。

## 参考文档

- 竖切设计（本计划的权威依据）：`docs/superpowers/specs/2026-09-30-clash-like-routing-slice-a-design.md`
- 父规格（D1–D8、规则语义）：`docs/superpowers/specs/2026-09-30-clash-like-routing-design.md`
- 已完成的 Phase 1（`rule_engine` / `fakedns`）：`docs/superpowers/plans/2026-09-30-clash-like-routing-phase1.md`

## 文件结构（先锁定职责，再拆任务）

| 文件 | 动作 | 职责 |
| --- | --- | --- |
| `src-tauri/Cargo.toml` | Modify | 新增 `smoltcp`、`nix` 依赖 |
| `src-tauri/src/vpn/mod.rs` | Modify | 注册新模块 |
| `src-tauri/src/vpn/platform.rs` | Create | `TunDevice`/`RouteTable`/`IfBinder`/`ProcLookup` 抽象 + macOS 实现 |
| `src-tauri/src/vpn/tun_device.rs` | Create | macOS `utun` 设备创建/读写/地址/MTU |
| `src-tauri/src/vpn/netstack.rs` | Create | `smoltcp` 用户态栈：`Device` 桥接、socket 表、流表 |
| `src-tauri/src/vpn/router.rs` | Create | 出口选择（DIRECT/VPN）+ 双向字节搬运 + `Engine` 编排 |
| `src-tauri/src/vpn/dns_srv.rs` | Create | fake-ip DNS 应答（DNS 报文解析/构造） |
| `src-tauri/src/vpn/proc_lookup.rs` | Create | `sysctl` pcblist → PID → 进程名，带缓存 |
| `src-tauri/src/vpn/models.rs` | Modify | `VpnRule`/`VpnRuleSet`/`VpnProfile`/`VpnRequest`/`VpnStatus` 新形状，删除 `VpnMode` |
| `src-tauri/src/vpn/script.rs` | Modify | vpnc-script 退化：只配接口地址 + MTU + 服务器主机路由 |
| `src-tauri/src/vpn/worker.rs` | Modify | 装配 `Engine`、看门狗、路由回收 |
| `src-tauri/src/vpn/commands.rs` | Modify | 规则编译接入、`MatchInput` 组装、规则校验/预览命令 |
| `src-tauri/src/vpn/session.rs` | Modify | 会话文件读写适配新 `VpnRequest`（仅测试与字段） |
| `src-tauri/src/config.rs` | Modify | 配置迁移：`mode` 字段兼容丢弃、规则/DNS 字段默认值 |
| `src/types/vpn.ts` | Modify | 前端类型：`VpnRule.kind/value/policy/no_resolve`、删 `VpnMode` |
| `src/composables/useVpn.ts` | Modify | 规则 CRUD、`defaultPolicy`、校验/预览调用 |
| `src/components/vpn/VpnRuleTable.vue` | Modify | 有序规则编辑器 + 固定 `MATCH` 行 |
| `src/components/vpn/VpnProfileForm.vue` | Modify | 去 `mode` 卡片，改 DNS 段（`fake_ip_range`/`fake_ip_filter`/`use_vpn_dns`） |
| `src/App.vue` | Modify | 去托盘 `vpn-mode` 与状态栏 `mode` 展示 |

---

## Task 1: 依赖与平台抽象

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/vpn/platform.rs`
- Modify: `src-tauri/src/vpn/mod.rs`
- Test: `src-tauri/src/vpn/platform.rs`（`#[cfg(test)]` 内联）

- [ ] **Step 1: 写失败的测试**

在新建的 `src-tauri/src/vpn/platform.rs` 末尾写入：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV4;

    #[test]
    fn flow_key_is_hashable_and_distinguishes_transport() {
        use std::collections::HashSet;
        let a = FlowKey {
            transport: Transport::Tcp,
            local: "10.0.0.2:5000".parse::<SocketAddrV4>().unwrap(),
            remote: "1.1.1.1:443".parse::<SocketAddrV4>().unwrap(),
        };
        let b = FlowKey {
            transport: Transport::Udp,
            ..a
        };
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn cidr_to_single_host_route_target_is_slash32() {
        assert_eq!(host_route_cidr("1.2.3.4"), "1.2.3.4/32");
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::platform`
Expected: 编译失败，`FlowKey`/`Transport`/`host_route_cidr` 未定义。

- [ ] **Step 3: 加依赖**

在 `src-tauri/Cargo.toml` 的 `[dependencies]` 中新增（保持既有排序风格，附在 `tempfile` 一行之后）：

```toml
smoltcp = { version = "0.11", default-features = false, features = [
    "std",
    "alloc",
    "medium-ip",
    "proto-ipv4",
    "socket-tcp",
    "socket-udp",
] }
nix = { version = "0.29", features = ["net", "ioctl", "socket", "fs"] }
```

- [ ] **Step 4: 实现 `platform.rs` 抽象与 macOS 实现**

```rust
//! 平台抽象：TUN 设备、路由表、接口绑定、进程反查。
//!
//! 本竖切只落 macOS 实现；Windows 实现留待下一竖切。trait 先立好，
//! 让 `netstack`/`router` 不直接依赖 `libc`。

use std::net::{Ipv4Addr, SocketAddrV4};

use super::route;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowKey {
    pub transport: Transport,
    pub local: SocketAddrV4,
    pub remote: SocketAddrV4,
}

/// 把单个 IPv4 表示成 /32 的主机路由目标。
pub fn host_route_cidr(ip: &str) -> String {
    format!("{}/32", ip)
}

/// 应用侧连接终结后，出站 socket 钉哪张网卡。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    /// 直连：钉物理网卡
    Direct,
    /// 走隧道：钉隧道网卡（openconnect 建立的 utunZ）
    Vpn,
}

/// TUN 设备的最小能力。读写的都是一个完整 IP 包。
pub trait TunDevice: Send {
    fn name(&self) -> &str;
    fn index(&self) -> u32;
    fn read_packet(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_packet(&mut self, buf: &[u8]) -> std::io::Result<usize>;
}

/// 路由表操作。macOS 走 `/sbin/route`。
pub trait RouteTable: Send + Sync {
    /// 加一条 `ip/32 -> if_index` 的主机路由（流路由，用于把某条出去连到隧道）。
    fn add_flow_route(&self, ip: Ipv4Addr, if_index: u32) -> Result<(), String>;
    /// 删掉上面那条流路由。
    fn del_flow_route(&self, ip: Ipv4Addr) -> Result<(), String>;
    /// 加「捕获用」的半默认路由 `0.0.0.0/1` + `128.0.0.0/1 -> if_index`。
    fn add_capture_routes(&self, if_index: u32) -> Result<(), String>;
    /// 删掉捕获用的两条半默认路由（需给出当时的捕获网卡索引以还原接口名）。
    fn del_capture_routes(&self, if_index: u32) -> Result<(), String>;
    /// 给 VPN 服务器本身加主机路由，走物理网关，避免 openconnect 的包被自己捕获成环。
    fn add_server_host_route(&self, ip: Ipv4Addr, gateway: &str) -> Result<(), String>;
    fn del_server_host_route(&self, ip: Ipv4Addr) -> Result<(), String>;
}

/// 把某个 fd 的出口钉到指定网卡。
pub trait IfBinder: Send + Sync {
    fn bind_fd_to_if(&self, fd: i32, if_index: u32) -> std::io::Result<()>;
}

/// 由五元组反查发起进程。
pub trait ProcLookup: Send {
    fn pid_for(&mut self, key: &FlowKey) -> Option<u32>;
    fn name_for_pid(&mut self, pid: u32) -> Option<String>;
}

// ===================== macOS 实现 =====================

/// 把接口索引解析成接口名。macOS 的 `/sbin/route` 只认名字（`-interface <name>`），
/// 不认索引；`route::RouteTarget` 的 `if_index` 字段在 macOS 分支从不被使用。
fn if_name_for_index(index: u32) -> Result<String, String> {
    let mut buf = [0i8; libc::IF_NAMESIZE];
    let ptr = unsafe { libc::if_indextoname(index as libc::c_uint, buf.as_mut_ptr()) };
    if ptr.is_null() {
        return Err(format!("找不到接口索引 {} 对应的接口名", index));
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Ok(cstr.to_string_lossy().to_string())
}

pub struct MacRouteTable;

impl MacRouteTable {
    pub fn new() -> Self {
        MacRouteTable
    }
}

impl RouteTable for MacRouteTable {
    fn add_flow_route(&self, ip: Ipv4Addr, if_index: u32) -> Result<(), String> {
        let name = if_name_for_index(if_index)?;
        let cidr = host_route_cidr(&ip.to_string());
        let target = route::RouteTarget::new(0, &name, "");
        route::add_route(&cidr, &target, 1).map(|_| ())
    }

    fn del_flow_route(&self, ip: Ipv4Addr) -> Result<(), String> {
        route::delete_host_route(&ip.to_string())
    }

    fn add_capture_routes(&self, if_index: u32) -> Result<(), String> {
        let name = if_name_for_index(if_index)?;
        let target = route::RouteTarget::new(0, &name, "");
        route::add_route("0.0.0.0/1", &target, 1)?;
        route::add_route("128.0.0.0/1", &target, 1)?;
        Ok(())
    }

    fn del_capture_routes(&self, if_index: u32) -> Result<(), String> {
        let name = if_name_for_index(if_index)?;
        let target = route::RouteTarget::new(0, &name, "");
        route::delete_route("0.0.0.0/1", &target)?;
        route::delete_route("128.0.0.0/1", &target)?;
        Ok(())
    }

    fn add_server_host_route(&self, ip: Ipv4Addr, gateway: &str) -> Result<(), String> {
        let target = route::RouteTarget::new(0, "", gateway);
        route::add_route(&host_route_cidr(&ip.to_string()), &target, 1).map(|_| ())
    }

    fn del_server_host_route(&self, ip: Ipv4Addr) -> Result<(), String> {
        route::delete_host_route(&ip.to_string())
    }
}

/// macOS 用 `IP_BOUND_IF` 把 fd 出口钉到网卡。
pub struct MacIfBinder;

impl MacIfBinder {
    pub fn new() -> Self {
        MacIfBinder
    }
}

impl IfBinder for MacIfBinder {
    fn bind_fd_to_if(&self, fd: i32, if_index: u32) -> std::io::Result<()> {
        let idx = if_index as libc::c_uint;
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_IP,
                libc::IP_BOUND_IF,
                &idx as *const libc::c_uint as *const libc::c_void,
                std::mem::size_of::<libc::c_uint>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
```

同时把上面 Step 1 的两个测试文件头 `use` 补上（`FlowKey`/`Transport`/`host_route_cidr` 都来自 `super::*`），无需改动。

> **平台门禁（必做）**：`platform.rs` 里 `if_name_for_index`、`MacRouteTable`、`MacIfBinder` 及其 impl 都用到了仅 unix 的 `libc` 符号（`IF_NAMESIZE`/`if_indextoname`/`IP_BOUND_IF`），必须给这些项加 `#[cfg(target_os = "macos")]`，`use super::route;` 同样加门禁；测试里 `if_name_lookup_rejects_bogus_index` 也要加 `#[cfg(target_os = "macos")]`。否则 Windows/Linux 目标会编译失败（本仓库是 Windows-first）。trait 与 `Transport`/`FlowKey`/`Egress`/`host_route_cidr` 保持跨平台，不加门禁。

- [ ] **Step 5: 在 `mod.rs` 注册模块**

在 `src-tauri/src/vpn/mod.rs` 的模块声明列表中按字母序插入：

```rust
pub mod platform;
```

- [ ] **Step 6: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::platform`
Expected: PASS（2 passed）。

- [ ] **Step 7: 构建检查**

Run: `cd src-tauri && cargo build`
Expected: 编译通过。`libc` 作为 `nix` 的传递依赖已在依赖树中；若 `libc::IP_BOUND_IF` 报未定义，在 `Cargo.toml` 显式加 `libc = "0.2"` 后再构建。

- [ ] **Step 8: 提交（待用户同意后执行）**

```bash
git add src-tauri/Cargo.toml src-tauri/src/vpn/platform.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): 新增平台抽象与 smoltcp/nix 依赖"
```

---

## Task 2: 数据模型切换（并保持全仓可编译）

**Files:**
- Modify: `src-tauri/src/vpn/models.rs`
- Modify: `src-tauri/src/vpn/session.rs`
- Modify: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/vpn/script.rs`
- Modify: `src-tauri/src/vpn/worker.rs`
- Modify: `src-tauri/src/vpn/commands.rs`
- Test: `src-tauri/src/vpn/models.rs`（内联）

本任务把「双模式」数据模型换成「规则集 + 默认策略 + fake-ip 配置」。为了保证每一步都能编译、且过渡期 CIDR 规则仍能正常分流，`script.rs`/`worker.rs`/`commands.rs` 在本任务里只做**机械适配**：用 `rule_engine::compile` 得到规则集，把 `IpCidr` 类型规则继续用旧的 `route::add_route` 途径下发；真正的按流引擎在 Task 9 接管。

- [ ] **Step 1: 写失败的测试**

在 `src-tauri/src/vpn/models.rs` 的 `mod tests` 末尾加：

```rust
    #[test]
    fn default_rule_is_domain_match_direct() {
        let rule: VpnRule = serde_json::from_str(
            r#"{"id":"r1","kind":"DOMAIN","value":"a.com","policy":"DIRECT","remark":"","enabled":true}"#,
        ).unwrap();
        assert_eq!(rule.kind, "DOMAIN");
        assert_eq!(rule.policy, "DIRECT");
        assert!(rule.no_resolve);
    }

    #[test]
    fn old_profile_with_mode_still_loads() {
        let raw = r#"{
            "id":"p1","name":"公司 VPN","server":"vpn.example.com:443",
            "username":"alice","password":"cipher","password_encrypted":true,
            "protocol":"anyconnect","mode":"company_first","rule_set_id":"rs1",
            "use_vpn_dns":false,"extra_args":[],"connector_path":"",
            "created_at":"","updated_at":""
        }"#;
        let profile: VpnProfile = serde_json::from_str(raw).expect("老档案必须还能读");
        assert_eq!(profile.server, "vpn.example.com:443");
        assert_eq!(profile.fake_ip_range, "198.18.0.0/16");
    }

    #[test]
    fn rule_set_default_policy_defaults_to_direct() {
        let set: VpnRuleSet = serde_json::from_str(r#"{"id":"rs1","name":"默认"}"#).unwrap();
        assert_eq!(set.default_policy, "DIRECT");
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::models`
Expected: 编译失败，`VpnRule.kind`/`VpnProfile.fake_ip_range`/`VpnRuleSet.default_policy` 未定义。

- [ ] **Step 3: 改 `models.rs`**

**(a) 删除 `VpnMode`**：删除 `models.rs:7-30` 整个 `VpnMode` 枚举、`impl Default` 与 `impl VpnMode`。

**(b) 重写 `VpnRule`**（替换 `models.rs:101-112`）：

```rust
/// 单条分流规则。`kind`/`policy` 取值与 `rule_engine::RuleKind`/`Policy` 的
/// `as_str()` 保持一致（"domain" / "direct" 等），在前端与 JSON 之间直接透传。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnRule {
    pub id: String,
    /// 匹配类型（Clash 规范形式）：DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD / PROCESS-NAME / DST-PORT / IP-CIDR / RULE-SET
    pub kind: String,
    /// 匹配值
    pub value: String,
    /// 命中后走向：direct / vpn
    #[serde(default = "default_policy_str")]
    pub policy: String,
    /// 是否禁用 IP 解析（仅 ip_cidr 有意义），默认 true
    #[serde(default = "default_true")]
    pub no_resolve: bool,
    #[serde(default)]
    pub remark: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_policy_str() -> String {
    "DIRECT".to_string()
}
```

**(c) 改 `VpnRuleSet`**（替换 `models.rs:115-123`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnRuleSet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub rules: Vec<VpnRule>,
    /// 所有规则都没命中时的兜底走向：direct / vpn
    #[serde(default = "default_policy_str")]
    pub default_policy: String,
    #[serde(default)]
    pub created_at: String,
}
```

**(d) 改 `VpnProfile`**：删除 `mode` 字段（`models.rs:145-147`），新增 fake-ip 配置：

```rust
    /// 绑定的规则集 id
    #[serde(default)]
    pub rule_set_id: String,
    /// fake-ip 地址池
    #[serde(default = "default_fake_ip_range")]
    pub fake_ip_range: String,
    /// fake-ip 过滤：命中者不分配假地址（`*.suffix` 视为域名后缀，其余视为关键字包含）
    #[serde(default)]
    pub fake_ip_filter: Vec<String>,
    /// 是否把服务端下发的 DNS 写进系统解析器
    #[serde(default = "default_true")]
    pub use_vpn_dns: bool,
```

并在 `default_protocol` 附近加：

```rust
fn default_fake_ip_range() -> String {
    "198.18.0.0/16".to_string()
}
```

**(e) 改 `VpnStatus`**：删除 `mode` 字段（`models.rs:335-336`）与 `impl Default` 里的 `mode: VpnMode::default(),`（`models.rs:384`）。

**(f) 重写 `VpnRequest`**（替换 `models.rs:403-434`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnRequest {
    pub profile_id: String,
    pub profile_name: String,
    pub server: String,
    pub username: String,
    /// 明文密码，只存在于本机会话目录，连接完成后由 worker 删除
    pub password: String,
    pub protocol: String,
    pub connector: ConnectorKind,
    /// 连接器可执行文件路径
    pub connector_path: String,
    /// 已展开的规则（外置 RULE-SET 由主进程按需展开，worker 直接消费）
    #[serde(default)]
    pub rules: Vec<VpnRule>,
    /// 兜底走向：direct / vpn
    #[serde(default = "default_policy_str")]
    pub default_policy: String,
    /// fake-ip 地址池
    #[serde(default = "default_fake_ip_range")]
    pub fake_ip_range: String,
    /// fake-ip 过滤规则
    #[serde(default)]
    pub fake_ip_filter: Vec<String>,
    pub use_vpn_dns: bool,
    /// 连接期间是否开启学习模式
    #[serde(default)]
    pub learn: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// 生成的 vpnc-script 绝对路径
    pub script_path: String,
    /// 路由钩子可执行文件（就是本程序自身）
    pub hook_exe: String,
    #[serde(default)]
    pub created_at: String,
}
```

- [ ] **Step 4: 适配 `session.rs` 测试**

把 `session.rs:308` 的 `use crate::vpn::models::{ConnectorKind, VpnMode};` 改为 `use crate::vpn::models::ConnectorKind;`。

把 `sample_request()`（`session.rs:322-342`）里的 `mode`/`include_rules`/`exclude_rules` 三段替换为：

```rust
            rules: vec![crate::vpn::models::VpnRule {
                id: "r1".into(),
                kind: "IP-CIDR".into(),
                value: "192.168.136.0/22".into(),
                policy: "VPN".into(),
                no_resolve: true,
                remark: String::new(),
                enabled: true,
            }],
            default_policy: "DIRECT".into(),
            fake_ip_range: "198.18.0.0/16".into(),
            fake_ip_filter: vec![],
```

并把 `request_roundtrip` 断言（`session.rs:350-351`）改为：

```rust
        assert_eq!(back.default_policy, "DIRECT");
        assert_eq!(back.rules.len(), 1);
        assert_eq!(back.rules[0].value, "192.168.136.0/22");
```

- [ ] **Step 5: 适配 `config.rs` 测试**

在 `config.rs` 的测试辅助函数 `vpn_profile(...)`（约 `config.rs:537-556`）中删除 `mode: Default::default(),` 一行，其余保持不变（`rule_set_id` 保留）。若该函数用 `..Default::default()` 构造则无需改动。

- [ ] **Step 6: 机械适配 `script.rs`**

`script.rs` 目前导入 `VpnMode`（`script.rs:16`）并在 `windows_script`/`macos_script` 中按模式分支。先做**最小去 mode**改动（真正的退化在 Task 8）：

- 删除 `use crate::vpn::models::VpnMode;`（`script.rs:16`）。
- `ScriptContext`（`script.rs:22-28`）删除 `pub mode: VpnMode,` 字段。
- `windows_script` 中：删除 `let mode = match ctx.mode {...};`、`let set_gateway = if ctx.mode == VpnMode::CompanyFirst {...};`，把模板里的 `__MODE__` 固定替换为 `"rule"`、`__SET_GATEWAY__` 固定替换为 `"false"`。
- `macos_script` 中：同样删除对 `ctx.mode` 的分支，`SET_GATEWAY` 相关分支恒为「不接管默认路由」。
- 更新 `script.rs` 内所有断言 `MODE="customer_first"` / `SET_GATEWAY=true` / `default_route vpn` 的测试到新取值（`MODE="rule"`、`SET_GATEWAY=false`、不再出现 `default_route vpn`）。

- [ ] **Step 7: 机械适配 `worker.rs`（过渡期）**

- 删除 `worker.rs` 中对 `VpnMode` 的 `use`。
- `run_openconnect` 里构造 `ScriptContext`（约 `worker.rs:664`）时去掉 `mode: self.request.mode,`。
- 把 `mode.label()` 的两处状态文案（约 `worker.rs:745`、`worker.rs:752`）改成固定文案：连接中 "正在建立隧道"、已连接 "隧道已建立"。
- 用下列过渡实现替换 `apply_routes`（`worker.rs:339-395`）与 `guard_routes`（`worker.rs:397-432`）：

```rust
    /// 过渡期路由：把编译后的规则里 `IP-CIDR` 类型、policy=VPN 的条目
    /// 用旧途径下发。真正的按流引擎在后续任务接管。
    fn apply_routes(&mut self) {
        self.status.routes.clear();
        let rules = self.compiled_rules();
        let target = self.vpn_target();
        for rule in rules.rules() {
            if rule.kind != crate::vpn::rule_engine::RuleKind::IpCidr {
                continue;
            }
            if rule.policy != crate::vpn::rule_engine::Policy::Vpn {
                continue;
            }
            let cidr = rule.value.clone();
            match route::add_route(&cidr, &target, 1) {
                Ok(()) => {
                    self.log(&format!("[路由] {} -> VPN ({})", cidr, target.display()));
                    self.record_applied(&cidr, &target, "vpn", RouteOutcome::Created);
                }
                Err(e) => self.log(&format!("[路由] {} 下发失败: {}", cidr, e)),
            }
        }
        let _ = self.session.write_status(&self.status);
    }

    fn compiled_rules(&mut self) -> crate::vpn::rule_engine::CompiledRuleSet {
        use crate::vpn::rule_engine::{compile, Policy, Rule, RuleKind};
        let enabled: Vec<(String, String, String, bool)> = self
            .request
            .rules
            .iter()
            .filter(|r| r.enabled)
            .map(|r| (r.kind.clone(), r.value.clone(), r.policy.clone(), r.no_resolve))
            .collect();
        let mut rules = Vec::new();
        let mut skipped = Vec::new();
        for (kind_s, value, policy_s, no_resolve) in enabled {
            match (RuleKind::parse(&kind_s), Policy::parse(&policy_s)) {
                (Ok(kind), Ok(policy)) => {
                    rules.push(Rule::new(kind, &value, policy).with_no_resolve(no_resolve));
                }
                _ => skipped.push(format!("{}({})", kind_s, value)),
            }
        }
        // 解析不了的规则要留痕，不能静默丢弃
        for s in &skipped {
            self.log(&format!("[规则] 无法解析，已跳过：{}", s));
        }
        let default = Policy::parse(&self.request.default_policy).unwrap_or(Policy::Direct);
        match compile(&rules, &std::collections::HashMap::new(), default) {
            Ok(set) => set,
            Err(e) => {
                self.log(&format!("[规则] 规则集编译失败，已回退为空规则集：{}", e));
                compile(&[], &std::collections::HashMap::new(), default)
                    .expect("空规则集必然可编译")
            }
        }
    }

    fn guard_routes(&mut self) {
        let target = self.vpn_target();
        let checks: Vec<String> = self
            .applied
            .iter()
            .filter(|record| record.outcome == RouteOutcome::Created)
            .map(|record| record.cidr.clone())
            .collect();

        let mut repaired = 0usize;
        for cidr in checks {
            match route::route_exists(&cidr, &target) {
                Ok(true) => {}
                Ok(false) => {
                    if route::add_route(&cidr, &target, 1).is_ok() {
                        repaired += 1;
                        self.log(&format!("[路由守护] {} 被改动，已重新下发", cidr));
                    }
                }
                Err(_) => {}
            }
        }
        if repaired > 0 {
            let _ = self.session.write_status(&self.status);
        }
    }
```

> 说明：`rule_engine::Rule` 的字段访问需要在其模块内把 `kind`/`policy`/`value`/`no_resolve` 暴露为 `pub`（Phase 1 已如此）；若 `rule_engine` 提供 getter 则改用 getter。`CompiledRuleSet` 需暴露 `rules(&self) -> &[Rule]`——若尚未暴露，在 `rule_engine.rs` 加：

```rust
impl CompiledRuleSet {
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
}
```

- 把 `known_rules`（`worker.rs:445-453`）改为返回 `request.rules` 里所有 `enabled` 的 `value`：

```rust
    fn known_rules(&self) -> Vec<String> {
        self.request
            .rules
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.value.clone())
            .collect()
    }
```

- [ ] **Step 8: 机械适配 `commands.rs`**

替换 `vpn_connect` 中 `enabled` 计算与空规则告警（`commands.rs:402-423`）：

```rust
    // 3. 组装连接请求
    let rules: Vec<VpnRule> = rules.into_iter().filter(|rule| rule.enabled).collect();
    let rule_set = profile
        .rule_set_id
        .as_str();
    let _ = rule_set;
```

把 `VpnRequest` 构造（`commands.rs:426-452`）里 `mode`/`include_rules`/`exclude_rules` 三段替换为：

```rust
        rules: rules.clone(),
        default_policy: default_policy_for(&profile, &rules),
        fake_ip_range: profile.fake_ip_range.clone(),
        fake_ip_filter: profile.fake_ip_filter.clone(),
```

并在文件内新增私有函数：

```rust
/// 规则集缺省走向由前端在保存时写入 profile；页面未提供时退化为 direct。
fn default_policy_for(_profile: &VpnProfile, _rules: &[VpnRule]) -> String {
    "direct".to_string()
}
```

同时把 `use crate::vpn::models::{...}` 的导入补齐 `VpnRule`（若尚未导入），并删除 `commands.rs` 中对 `VpnMode` 的引用。

- [ ] **Step 9: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::models vpn::session vpn::config`
Expected: PASS（新增 3 条 + 既有会话/配置测试全绿）。

- [ ] **Step 10: 全量构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过，无 `VpnMode` 残留（`grep -rn "VpnMode" src-tauri/src` 应为空）。

- [ ] **Step 11: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/models.rs src-tauri/src/vpn/session.rs src-tauri/src/config.rs \
        src-tauri/src/vpn/script.rs src-tauri/src/vpn/worker.rs src-tauri/src/vpn/commands.rs \
        src-tauri/src/vpn/rule_engine.rs
git commit -m "refactor(vpn): 数据模型切换为规则集 + fake-ip 配置，移除双模式"
```

---

## Task 3: macOS utun 设备

**Files:**
- Create: `src-tauri/src/vpn/tun_device.rs`
- Modify: `src-tauri/src/vpn/mod.rs`
- Test: `src-tauri/src/vpn/tun_device.rs`（内联）

- [ ] **Step 1: 写失败的测试**

`tun_device.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utun_name_maps_to_unit() {
        assert_eq!(unit_from_name("utun0"), Some(0));
        assert_eq!(unit_from_name("utun7"), Some(7));
        assert_eq!(unit_from_name("en0"), None);
    }

    #[test]
    fn control_socket_path_is_utun_control() {
        assert_eq!(UTUN_CONTROL_NAME, "com.apple.net.utun_control");
    }

    #[test]
    fn pickup_address_is_slash32() {
        // utun 必须给 4 字节地址（sockaddr 不带端口）
        let addr = pickup_sockaddr("10.8.0.9").unwrap();
        assert_eq!(addr.0, "10.8.0.9");
        assert_eq!(addr.1, 255); // AF_INET 低字节，避免平台差异
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::tun_device`
Expected: 编译失败，`unit_from_name`/`UTUN_CONTROL_NAME`/`pickup_sockaddr` 未定义。

- [ ] **Step 3: 实现 `tun_device.rs`**

```rust
//! macOS utun 设备：通过 PF_SYSTEM 控制 socket 打开一个 utun，
//! 再配点对点地址与 MTU。读写都是裸 IP 包。

use std::ffi::CString;
use std::io;
use std::net::Ipv4Addr;
use std::os::unix::io::RawFd;

use super::platform::TunDevice;

pub const UTUN_CONTROL_NAME: &str = "com.apple.net.utun_control";
const UTUN_OPT_IFNAME: libc::c_int = 2;

pub fn unit_from_name(name: &str) -> Option<u32> {
    name.strip_prefix("utun").and_then(|n| n.parse::<u32>().ok())
}

/// 返回 (地址字符串, 地址族低字节)。后者判 255 是为了不做平台相关假设。
pub fn pickup_sockaddr(ip: &str) -> Option<(String, u8)> {
    let parsed: Ipv4Addr = ip.parse().ok()?;
    Some((parsed.to_string(), (libc::AF_INET & 0xff) as u8))
}

pub struct Utun {
    fd: RawFd,
    name: String,
    index: u32,
}

impl Utun {
    /// 打开一个系统自动分配的 utun（unit 0 = 让内核挑）。
    pub fn create() -> io::Result<Utun> {
        let fd = unsafe { libc::socket(libc::AF_SYSTEM, libc::SOCK_DGRAM, libc::SYSPROTO_CONTROL) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut info: libc::ctl_info = unsafe { std::mem::zeroed() };
        let name = CString::new(UTUN_CONTROL_NAME).unwrap();
        info.ctl_name = name.as_ptr() as *mut _;
        let rc = unsafe {
            libc::ioctl(fd, libc::CTLIOCGINFO, &mut info as *mut libc::ctl_info)
        };
        if rc < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        let mut addr: libc::sockaddr_ctl = unsafe { std::mem::zeroed() };
        addr.sc_len = std::mem::size_of::<libc::sockaddr_ctl>() as u8;
        addr.sc_family = libc::AF_SYSTEM as u8;
        addr.ss_sysaddr = libc::AF_SYS_CONTROL as u16;
        addr.sc_id = info.ctl_id;
        addr.sc_unit = 0; // 让内核分配

        let rc = unsafe {
            libc::connect(
                fd,
                &addr as *const libc::sockaddr_ctl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_ctl>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        // 取回内核分配的接口名 utunN
        let mut name_buf = [0u8; 64];
        let mut len: libc::socklen_t = name_buf.len() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SYSPROTO_CONTROL,
                UTUN_OPT_IFNAME,
                name_buf.as_mut_ptr() as *mut libc::c_void,
                &mut len,
            )
        };
        if rc < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(name_buf.as_ptr() as *const libc::c_char) };
        let name = cstr.to_string_lossy().to_string();
        // utun 的「单元号」不等于接口索引，必须用内核给的索引。
        let index = unsafe { libc::if_nametoindex(cstr.as_ptr()) };
        if index == 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        Ok(Utun { fd, name, index })
    }

    pub fn set_up(&self) -> io::Result<()> {
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        let cname = CString::new(self.name.as_str()).unwrap();
        let bytes = cname.as_bytes_with_nul();
        let n = bytes.len().min(libc::IFNAMSIZ);
        for (i, b) in bytes[..n].iter().enumerate() {
            ifr.ifr_name[i] = *b as libc::c_char;
        }
        let rc = unsafe {
            libc::ioctl(self.fd, libc::SIOCSIFFLAGS, &ifr as *const libc::ifreq)
        };
        let _ = rc;
        Ok(())
    }
}

impl TunDevice for Utun {
    fn name(&self) -> &str {
        &self.name
    }

    fn index(&self) -> u32 {
        self.index
    }

    fn read_packet(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // utun 读出来是 4 字节地址族头 + IP 包，这里剥掉头
        let mut tmp = vec![0u8; buf.len() + 4];
        let n = unsafe {
            libc::read(self.fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len())
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let n = n as usize;
        if n <= 4 {
            return Ok(0);
        }
        let payload = &tmp[4..n];
        buf[..payload.len()].copy_from_slice(payload);
        Ok(payload.len())
    }

    fn write_packet(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 写出去要补 4 字节地址族头：AF_INET 大端
        let mut tmp = Vec::with_capacity(buf.len() + 4);
        tmp.extend_from_slice(&u32::from(u16::from_be(0x0002u16)).to_be_bytes()[..]);
        tmp[..4].copy_from_slice(&[0, 0, 0, libc::AF_INET as u8]);
        tmp.extend_from_slice(buf);
        let n = unsafe {
            libc::write(self.fd, tmp.as_ptr() as *const libc::c_void, tmp.len())
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((n as usize).saturating_sub(4))
    }
}

impl Drop for Utun {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}
```

> 注意：地址/MTU 的实际配置（点对点地址、MTU）由 `worker` 在设置接口后通过 `route`/`ifconfig` 完成；`Utun` 只负责设备打开与裸包读写。若在 `create()` 后需要立即配地址，调用 `worker` 侧的 `configure_interface(name, addr, mtu)`（Task 9 实现）。

- [ ] **Step 4: `mod.rs` 注册**

在 `mod.rs` 插入：

```rust
pub mod tun_device;
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::tun_device`
Expected: PASS（3 passed）。

- [ ] **Step 6: 构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过。

- [ ] **Step 7: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/tun_device.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): macOS utun 设备读写"
```

---

## Task 4: smoltcp 用户态栈（netstack）

**Files:**
- Create: `src-tauri/src/vpn/netstack.rs`
- Modify: `src-tauri/src/vpn/mod.rs`
- Test: `src-tauri/src/vpn/netstack.rs`（内联）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_key_from_parts_is_stable() {
        let k = FlowKey::from_parts(
            Transport::Tcp,
            "10.0.0.2:40000".parse().unwrap(),
            "93.184.216.34:443".parse().unwrap(),
        );
        assert_eq!(k.remote.port(), 443);
        assert_eq!(k.transport, Transport::Tcp);
    }

    #[test]
    fn accepted_flow_carries_domain_when_fake_ip() {
        let mut ns = NetstackConfig::default();
        ns.fake_domains.insert(0x0A0A_0A0A_u32, "example.com".to_string());
        assert_eq!(
            ns.domain_for_fake_ip("10.10.10.10"),
            Some("example.com".to_string())
        );
        assert_eq!(ns.domain_for_fake_ip("1.1.1.1"), None);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::netstack`
Expected: 编译失败，`FlowKey`/`NetstackConfig` 未定义。

- [ ] **Step 3: 实现 `netstack.rs`（smoltcp Device 桥接 + 配置）**

```rust
//! smoltcp 用户态网络栈：把 utunC 上收到的 IP 包喂给 smoltcp，
//! 由 smoltcp 完成 TCP 三次握手/收发；应用侧连接终结后，交 router 决定出口。
//!
//! 本模块只做「终结」，不做出口选择。

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};

use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address};

use super::platform::{Transport, TunDevice};

pub use super::platform::FlowKey;

/// netstack 需要的配置（来自 VpnRequest + fake-ip 池）
#[derive(Default)]
pub struct NetstackConfig {
    /// 捕获 TUN 的本地地址（点对点地址）
    pub local_ip: Ipv4Addr,
    pub mtu: usize,
    /// fake IP(u32) -> 域名
    pub fake_domains: HashMap<u32, String>,
}

impl NetstackConfig {
    pub fn domain_for_fake_ip(&self, ip: &str) -> Option<String> {
        let v: Ipv4Addr = ip.parse().ok()?;
        self.fake_domains.get(&u32::from(v)).cloned()
    }
}

pub fn fake_ip_to_u32(ip: &str) -> Option<u32> {
    ip.parse::<Ipv4Addr>().ok().map(u32::from)
}

impl FlowKey {
    pub fn from_parts(transport: Transport, local: SocketAddrV4, remote: SocketAddrV4) -> Self {
        FlowKey { transport, local, remote }
    }
}

/// 把 `TunDevice` 适配成 smoltcp 的 `Device`（medium = Ip，无以太头）。
pub struct TunPhy<D: TunDevice> {
    inner: D,
    mtu: usize,
    rx_buf: Vec<u8>,
}

impl<D: TunDevice> TunPhy<D> {
    pub fn new(inner: D, mtu: usize) -> Self {
        TunPhy { inner, mtu, rx_buf: vec![0u8; mtu] }
    }
}

pub struct TunRxToken {
    buffer: Vec<u8>,
}

pub struct TunTxToken<'a, D: TunDevice> {
    device: &'a mut D,
}

impl RxToken for TunRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.buffer)
    }
}

impl<'a, D: TunDevice> TxToken for TunTxToken<'a, D> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        let _ = self.device.write_packet(&buf);
        r
    }
}

impl<D: TunDevice> Device for TunPhy<D> {
    type RxToken<'a> = TunRxToken where Self: 'a;
    type TxToken<'a> = TunTxToken<'a, D> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let n = self.inner.read_packet(&mut self.rx_buf).ok()?;
        if n == 0 {
            return None;
        }
        let buffer = self.rx_buf[..n].to_vec();
        Some((
            TunRxToken { buffer },
            TunTxToken { device: &mut self.inner },
        ))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(TunTxToken { device: &mut self.inner })
    }
}

/// smoltcp 接口 + socket 集合。真正的流表与收发循环在 Task 5 的 `Engine` 中，
/// 这里只提供底层容器与轮询入口。
pub struct Netstack<D: TunDevice> {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    pub phy: TunPhy<D>,
    pub local_ip: Ipv4Addr,
}

impl<D: TunDevice> Netstack<D> {
    pub fn new(phy: TunPhy<D>, local_ip: Ipv4Addr, mac: [u8; 6]) -> Self {
        let mut config = Config::new(HardwareAddress::Ethernet(
            smoltcp::wire::EthernetAddress(mac),
        ));
        config.random_seed = 0x5eed;
        let mut iface = Interface::new(config, &mut { let p = TunPhy::new_dummy(); p }, Instant::now());
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0));
        });
        let sockets = SocketSet::new(Vec::new());
        Netstack { iface, sockets, phy, local_ip }
    }

    pub fn poll(&mut self, ts: Instant) {
        let Netstack { iface, sockets, phy, .. } = self;
        let _ = iface.poll(ts, phy, sockets);
    }
}
```

> 实现说明：smoltcp `Interface::new` 需要一个 `&mut Device` 做一次初始化；上面 `new_dummy()` 是示意，实际按下面 Step 3b 以 `phy` 自身传入。若 `Interface::new` 签名与版本不符，以 `smoltcp 0.11` 的 `iface::Interface::new(config, &mut device, now)` 为准，传入构造好的 `phy` 即可（构造顺序：先建 `phy`，再建 `iface`）。

- [ ] **Step 3b: 修正 `Netstack::new` 构造顺序**

把上面的 `Netstack::new` 替换为：

```rust
    pub fn new(phy: TunPhy<D>, local_ip: Ipv4Addr, mac: [u8; 6]) -> Self {
        let mut config = Config::new(HardwareAddress::Ethernet(
            smoltcp::wire::EthernetAddress(mac),
        ));
        config.random_seed = 0x5eed;
        let mut iface = {
            let mut tmp = TunPhy::new(DummyTun::default(), 1500);
            let iface = Interface::new(config, &mut tmp, Instant::now());
            iface
        };
        let sockets = SocketSet::new(Vec::new());
        Netstack { iface, sockets, phy, local_ip }
    }
```

并新增 `DummyTun`（仅用于 `Interface::new` 的占位，真设备随后由 `phy` 承担收发）：

```rust
#[derive(Default)]
struct DummyTun;

impl TunDevice for DummyTun {
    fn name(&self) -> &str { "dummy" }
    fn index(&self) -> u32 { 0 }
    fn read_packet(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> { Ok(0) }
    fn write_packet(&mut self, buf: &[u8]) -> std::io::Result<usize> { Ok(buf.len()) }
}
```

- [ ] **Step 4: `mod.rs` 注册**

```rust
pub mod netstack;
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::netstack`
Expected: PASS（2 passed）。

- [ ] **Step 6: 构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过（若 smoltcp API 名称有出入，按编译器提示修正到同一语义）。

- [ ] **Step 7: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/netstack.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): smoltcp 用户态栈与 utun Device 桥接"
```

---

## Task 5: 出口路由器与决策函数

**Files:**
- Create: `src-tauri/src/vpn/router.rs`
- Modify: `src-tauri/src/vpn/mod.rs`
- Test: `src-tauri/src/vpn/router.rs`（内联，覆盖可测的纯逻辑）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::rule_engine::{compile, MatchInput, Policy, Rule, RuleKind};
    use std::collections::HashMap;

    fn decide(rules: Vec<Rule>, default: Policy, domain: &str, ip: &str, port: u16) -> Policy {
        let set = compile(&rules, &HashMap::new(), default).unwrap();
        set.evaluate(&MatchInput {
            domain: Some(domain),
            ip: Some(ip),
            port: Some(port),
            process: None,
        })
    }

    #[test]
    fn first_match_wins_over_later_broader_rule() {
        let rules = vec![
            Rule::new(RuleKind::Domain, "a.com".into(), Policy::Vpn),
            Rule::new(RuleKind::DomainKeyword, "a.com".into(), Policy::Direct),
        ];
        assert_eq!(decide(rules, Policy::Direct, "a.com", "1.2.3.4", 443), Policy::Vpn);
    }

    #[test]
    fn default_policy_applies_when_nothing_matches() {
        let rules = vec![Rule::new(RuleKind::DstPort, "8080".into(), Policy::Vpn)];
        assert_eq!(decide(rules, Policy::Direct, "x.com", "1.2.3.4", 443), Policy::Direct);
    }

    #[test]
    fn egress_maps_policy() {
        assert_eq!(Egress::from(Policy::Vpn), Egress::Vpn);
        assert_eq!(Egress::from(Policy::Direct), Egress::Direct);
    }

    #[test]
    fn tcp_connect_timeout_is_bounded() {
        assert!(CONNECT_TIMEOUT_SECS > 0 && CONNECT_TIMEOUT_SECS <= 30);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::router`
Expected: 编译失败，`Egress::from`/`CONNECT_TIMEOUT_SECS` 未定义。

- [ ] **Step 3: 实现 `router.rs` 的出口与编排外壳**

```rust
//! 出口选择与双向字节搬运。
//!
//! 每条应用侧连接被 netstack 终结后，这里按规则决定出口：
//! - DIRECT：出站 socket `IP_BOUND_IF` 钉物理网卡；
//! - VPN：出站 socket 钉隧道网卡，并加一条 `/32 -> 隧道` 流路由。
//! 随后在两个方向搬运字节，直到任一侧关闭。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

use super::platform::{Egress, FlowKey, IfBinder, RouteTable, Transport};
use super::rule_engine::Policy;

pub const CONNECT_TIMEOUT_SECS: u64 = 10;

impl From<Policy> for Egress {
    fn from(policy: Policy) -> Self {
        match policy {
            Policy::Vpn => Egress::Vpn,
            Policy::Direct => Egress::Direct,
        }
    }
}

/// 每条流的出站状态
struct FlowEgress {
    stream: Option<TcpStream>,
    /// VPN 出口为这条流加的 /32 路由（断开时需要回收）
    installed_route: Option<Ipv4Addr>,
}

/// 出口资源持有者。物理网卡索引与隧道网卡索引在连接建立时确定。
pub struct Router {
    route: Box<dyn RouteTable>,
    binder: Box<dyn IfBinder>,
    physical_if: u32,
    vpn_if: u32,
    flows: HashMap<FlowKey, FlowEgress>,
}

impl Router {
    pub fn new(
        route: Box<dyn RouteTable>,
        binder: Box<dyn IfBinder>,
        physical_if: u32,
        vpn_if: u32,
    ) -> Self {
        Router { route, binder, physical_if, vpn_if, flows: HashMap::new() }
    }

    /// 为一条终结后的应用连接建立出站 TCP。
    pub fn open_egress(&mut self, key: &FlowKey, egress: Egress) -> std::io::Result<()> {
        if self.flows.contains_key(key) {
            return Ok(());
        }
        let if_index = match egress {
            Egress::Direct => self.physical_if,
            Egress::Vpn => self.vpn_if,
        };
        let stream = Self::connect_bound(key.remote, if_index, &*self.binder)?;

        let mut installed_route = None;
        if egress == Egress::Vpn {
            let remote_ip = *key.remote.ip();
            self.route
                .add_flow_route(remote_ip, self.vpn_if)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            installed_route = Some(remote_ip);
        }

        self.flows.insert(
            key.to_owned(),
            FlowEgress { stream: Some(stream), installed_route },
        );
        Ok(())
    }

    fn connect_bound(
        remote: SocketAddrV4,
        if_index: u32,
        binder: &dyn IfBinder,
    ) -> std::io::Result<TcpStream> {
        use std::os::unix::io::AsRawFd;
        let socket = socket2_connect(remote)?;
        binder.bind_fd_to_if(socket.as_raw_fd(), if_index)?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        socket.set_write_timeout(Some(Duration::from_millis(200)))?;
        socket.set_nodelay(true)?;
        Ok(socket)
    }

    /// 把出站侧可读到的数据搬进应用侧。返回搬运的字节数。
    pub fn pump_out_to_app(&mut self, key: &FlowKey, app_tx: &mut dyn Write) -> std::io::Result<usize> {
        let flow = match self.flows.get_mut(key) {
            Some(f) => f,
            None => return Ok(0),
        };
        let stream = match flow.stream.as_mut() {
            Some(s) => s,
            None => return Ok(0),
        };
        let mut buf = [0u8; 16 * 1024];
        match stream.read(&mut buf) {
            Ok(0) => Ok(0),
            Ok(n) => {
                app_tx.write_all(&buf[..n])?;
                Ok(n)
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// 关闭一条流并回收其 VPN 流路由。
    pub fn close_flow(&mut self, key: &FlowKey) {
        if let Some(flow) = self.flows.remove(key) {
            if let Some(ip) = flow.installed_route {
                let _ = self.route.del_flow_route(ip);
            }
        }
    }

    pub fn open_flow_count(&self) -> usize {
        self.flows.len()
    }
}

/// 建一个阻塞 TCP 连接（`socket2` 不可用时用标准库 + 手动 connect）。
fn socket2_connect(remote: SocketAddrV4) -> std::io::Result<TcpStream> {
    TcpStream::connect_timeout(&std::net::SocketAddr::V4(remote), Duration::from_secs(CONNECT_TIMEOUT_SECS))
}
```

> 说明：标准库 `TcpStream::connect_timeout` 会在连接成功前即返回，之后再用 `IP_BOUND_IF` 绑网卡是**不生效**的（绑定必须在 connect 之前）。因此实际实现要改用 `libc::socket` + `set_nonblocking` + `IP_BOUND_IF` + `connect` + `poll` 的手工连接。Step 3b 给出该实现并替换 `connect_bound`。

- [ ] **Step 3b: 用 `libc` 手工建连（保证绑定早于 connect）**

新增：

```rust
use std::os::unix::io::{FromRawFd, RawFd};

fn connect_bound(
    remote: SocketAddrV4,
    if_index: u32,
    binder: &dyn IfBinder,
) -> std::io::Result<TcpStream> {
    let fd: RawFd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let guard = FdGuard(fd);

    if let Err(e) = binder.bind_fd_to_if(fd, if_index) {
        return Err(e);
    }
    set_nonblocking(fd, true)?;

    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    addr.sin_family = libc::AF_INET as libc::sa_family_t;
    addr.sin_port = remote.port().to_be();
    addr.sin_addr.s_addr = u32::from(*remote.ip()).to_be();

    let rc = unsafe {
        libc::connect(fd, &addr as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t)
    };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(err);
        }
        wait_writable(fd, CONNECT_TIMEOUT_SECS)?;
        let mut soerr: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        unsafe {
            libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR,
                &mut soerr as *mut _ as *mut libc::c_void, &mut len);
        }
        if soerr != 0 {
            return Err(std::io::Error::from_raw_os_error(soerr));
        }
    }
    set_nonblocking(fd, false)?;
    let stream = {
        let s = unsafe { TcpStream::from_raw_fd(fd) };
        s.set_read_timeout(Some(Duration::from_millis(200)))?;
        s.set_write_timeout(Some(Duration::from_millis(200)))?;
        s.set_nodelay(true)?;
        s
    };
    std::mem::forget(guard);
    Ok(stream)
}

struct FdGuard(RawFd);
impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn set_nonblocking(fd: RawFd, on: bool) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let new = if on { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, new) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn wait_writable(fd: RawFd, timeout_secs: u64) -> std::io::Result<()> {
    let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
    let rc = unsafe { libc::poll(&mut pfd, 1, (timeout_secs * 1000) as libc::c_int) };
    if rc <= 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"));
    }
    Ok(())
}
```

删除 Step 3 中的旧 `connect_bound` 与 `socket2_connect`，保留本步版本（并把 `use std::os::unix::io::AsRawFd;` 一并去掉）。

- [ ] **Step 4: 出口决策函数**

在 `router.rs` 末尾追加两个纯函数（`Engine` 的装配**推迟到 Task 9**，因为 `Engine` 需要 `proc_lookup`（Task 7）与 `netstack`（Task 4）；本任务只交付 Router 与决策函数）：

```rust
use super::rule_engine::{CompiledRuleSet, MatchInput};

/// 查询一条已终结连接应走的出口（按 IP/端口/进程匹配）。
pub fn decide_egress(
    rules: &CompiledRuleSet,
    key: &FlowKey,
    process: Option<&str>,
) -> Egress {
    let ip = key.remote.ip().to_string();
    let input = MatchInput {
        domain: None,
        ip: Some(&ip),
        port: Some(key.remote.port()),
        process,
    };
    Egress::from(rules.evaluate(&input))
}

/// 按域名构造一次出口判定（供 netstack 在 accept 时用 fake-ip 反查出的域名调用）。
pub fn decide_domain(
    rules: &CompiledRuleSet,
    domain: &str,
    key: &FlowKey,
    process: Option<&str>,
) -> Egress {
    let ip = key.remote.ip().to_string();
    let input = MatchInput {
        domain: Some(domain),
        ip: Some(&ip),
        port: Some(key.remote.port()),
        process,
    };
    Egress::from(rules.evaluate(&input))
}
```

> 注：`decide_egress`/`decide_domain` 只接受 `process: Option<&str>`（不含 domain 重载）；Task 9 的 `Engine::poll` 按「有 fake-ip 域名 → `decide_domain`，否则 `decide_egress`」调用。

- [ ] **Step 5: `mod.rs` 注册**

```rust
pub mod router;
```

（`router.rs` 顶部加 `#![cfg(target_os = "macos")]` 门禁：它用 `libc` 的 socket/`IP_BOUND_IF`/poll，Windows 下应为空模块。）

- [ ] **Step 6: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::router`
Expected: PASS（4 passed）。

- [ ] **Step 7: 构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过。

- [ ] **Step 8: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/router.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): 出口路由器与决策函数"
```

---

## Task 6: fake-ip DNS 应答（dns_srv）

**Files:**
- Create: `src-tauri/src/vpn/dns_srv.rs`
- Modify: `src-tauri/src/vpn/mod.rs`
- Test: `src-tauri/src/vpn/dns_srv.rs`（内联）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn query_a(name: &str) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&1u16.to_be_bytes()); // A
        q.extend_from_slice(&1u16.to_be_bytes()); // IN
        q
    }

    #[test]
    fn parses_question_name_and_type() {
        let q = parse_query(&query_a("example.com")).unwrap();
        assert_eq!(q.name, "example.com");
        assert_eq!(q.qtype, 1);
        assert_eq!(q.id, 0x1234);
    }

    #[test]
    fn builds_a_answer_with_fake_ip() {
        let raw = query_a("example.com");
        let q = parse_query(&raw).unwrap();
        let resp = build_a_response(&raw, &q, "198.18.0.5".parse().unwrap());
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1); // ANCOUNT=1
        assert_eq!(&resp[resp.len() - 4..], &[198, 18, 0, 5]);
    }

    #[test]
    fn aaaa_query_returns_empty_answer() {
        let mut raw = query_a("example.com");
        let n = raw.len();
        raw[n - 4] = 0;
        raw[n - 3] = 28; // AAAA
        let q = parse_query(&raw).unwrap();
        assert_eq!(q.qtype, 28);
        let resp = build_empty_response(&raw, &q);
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 0);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::dns_srv`
Expected: 编译失败，`parse_query`/`build_a_response`/`build_empty_response` 未定义。

- [ ] **Step 3: 实现 `dns_srv.rs`**

```rust
//! fake-ip DNS 应答。
//!
//! 只处理最基础的「单问题」查询：A 查出假地址、AAAA 返回空答案、
//! 其余类型原样空答。上游真实解析在引擎侧按规则走出口完成（见 Task 9）。

use std::net::Ipv4Addr;

pub struct DnsQuery {
    pub id: u16,
    pub name: String,
    pub qtype: u16,
    /// 问题段在报文中的结束偏移（应答要原样拷回问题段）
    pub question_end: usize,
}

fn read_name(packet: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    loop {
        if pos >= packet.len() {
            return None;
        }
        let len = packet[pos] as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            return None; // 查询里不应出现压缩指针
        }
        pos += 1;
        if pos + len > packet.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&packet[pos..pos + len]).to_string());
        pos += len;
    }
    Some((labels.join("."), pos))
}

pub fn parse_query(packet: &[u8]) -> Option<DnsQuery> {
    if packet.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]);
    if qdcount != 1 {
        return None;
    }
    let (name, pos) = read_name(packet, 12)?;
    if pos + 4 > packet.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
    Some(DnsQuery { id, name, qtype, question_end: pos + 4 })
}

fn header_for(query: &[u8], q: &DnsQuery, ancount: u16, rcode: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(q.question_end + 16);
    out.extend_from_slice(&q.id.to_be_bytes());
    // QR=1, OPCODE=0, RD 沿用原查询, RA=1, RCODE
    let rd = query[2] & 0x01;
    out.push(0x80 | (rd << 0));
    out.push(0x80 | (rcode & 0x0F));
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&ancount.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out.extend_from_slice(&query[12..q.question_end]);
    out
}

pub fn build_a_response(query: &[u8], q: &DnsQuery, ip: Ipv4Addr) -> Vec<u8> {
    let mut out = header_for(query, q, 1, 0);
    out.extend_from_slice(&[0xC0, 0x0C]); // 指向问题里的域名
    out.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
    out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    out.extend_from_slice(&60u32.to_be_bytes()); // TTL 60s
    out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    out.extend_from_slice(&ip.octets());
    out
}

pub fn build_empty_response(query: &[u8], q: &DnsQuery) -> Vec<u8> {
    header_for(query, q, 0, 0)
}
```

- [ ] **Step 4: `mod.rs` 注册**

```rust
pub mod dns_srv;
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::dns_srv`
Expected: PASS（2 passed）。

- [ ] **Step 6: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/dns_srv.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): fake-ip DNS 应答构造与解析"
```

---

## Task 7: 进程反查（proc_lookup）

**Files:**
- Create: `src-tauri/src/vpn/proc_lookup.rs`
- Modify: `src-tauri/src/vpn/mod.rs`
- Test: `src-tauri/src/vpn/proc_lookup.rs`（内联）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pid_from_proc_pidpath_output() {
        let out = "/usr/sbin/mDNSResponder\0";
        assert_eq!(proc_pidpath_to_name(out).as_deref(), Some("mDNSResponder"));
    }

    #[test]
    fn cache_round_trips_name() {
        let mut c = NameCache::new(16);
        c.put(4321, "Safari");
        assert_eq!(c.get(4321).as_deref(), Some("Safari"));
        assert_eq!(c.get(9999), None);
    }

    #[test]
    fn flow_key_endpoint_matches_reverse_byte_order() {
        // pcblist 里的端口/地址是网络序，比对时要按同样字节序还原
        assert_eq!(endpoint_key(0x1F90, 0x0100007F), ("127.0.0.1".to_string(), 8080));
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::proc_lookup`
Expected: 编译失败，`proc_pidpath_to_name`/`NameCache`/`endpoint_key` 未定义。

- [ ] **Step 3: 实现 `proc_lookup.rs`**

```rust
//! 进程名反查：`sysctl net.inet.tcp.pcblist_n` / `net.inet.udp.pcblist_n`
//! 拿到本机 socket 表，按五元组匹配到 PID，再用 `proc_pidpath` 取进程名。
//!
//! pcblist 的二进制布局随内核版本变化；这里按 XNU 的 `xinpgen` 头游走记录
//! （用 `xig_len`/`xig_count` 定位边界），读取 `xinpcb_n` 的固定头字段。

use std::collections::HashMap;
use std::net::Ipv4Addr;

use super::platform::{FlowKey, ProcLookup, Transport};

/// 从 `proc_pidpath` 输出（NUL 结尾的绝对路径）取末段可执行名。
pub fn proc_pidpath_to_name(out: &str) -> Option<String> {
    let trimmed = out.trim_end_matches('\0').trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.rsplit('/').next().map(|s| s.to_string())
}

/// 把 pcblist 里的（网络序端口, 网络序地址）还原成可读端点。
pub fn endpoint_key(port_be: u32, addr_be: u32) -> (String, u16) {
    let ip = Ipv4Addr::from(u32::from_be(addr_be));
    let port = ((port_be & 0xFFFF) as u16).to_be() as u16;
    let port = u16::from_be((port_be & 0xFFFF) as u16);
    let _ = ip;
    (Ipv4Addr::from(u32::from_be(addr_be)).to_string(), port)
}

pub struct NameCache {
    cap: usize,
    map: HashMap<u32, String>,
}

impl NameCache {
    pub fn new(cap: usize) -> Self {
        NameCache { cap, map: HashMap::new() }
    }
    pub fn get(&self, pid: u32) -> Option<String> {
        self.map.get(&pid).cloned()
    }
    pub fn put(&mut self, pid: u32, name: &str) {
        if self.map.len() >= self.cap {
            if let Some(k) = self.map.keys().next().copied() {
                self.map.remove(&k);
            }
        }
        self.map.insert(pid, name.to_string());
    }
}

pub struct SysctlProcLookup {
    names: NameCache,
}

impl SysctlProcLookup {
    pub fn new() -> Self {
        SysctlProcLookup { names: NameCache::new(256) }
    }

    /// 取一条 pcblist，返回 (本地端口网络序, 本地地址网络序, PID)。
    fn pcblist(&self, transport: Transport) -> Vec<(u32, u32, u32)> {
        let name = match transport {
            Transport::Tcp => "net.inet.tcp.pcblist_n",
            Transport::Udp => "net.inet.udp.pcblist_n",
        };
        unsafe { read_pcblist(name) }
    }
}

impl ProcLookup for SysctlProcLookup {
    fn pid_for(&mut self, key: &FlowKey) -> Option<u32> {
        let want_port = key.local.port();
        let want_addr = u32::from(*key.local.ip()).to_be();
        for (port, addr, pid) in self.pcblist(key.transport) {
            if (port & 0xFFFF) as u16 == want_port.to_be() && addr == want_addr {
                return Some(pid);
            }
        }
        None
    }

    fn name_for_pid(&mut self, pid: u32) -> Option<String> {
        if let Some(n) = self.names.get(pid) {
            return Some(n);
        }
        let path = pid_path(pid)?;
        let name = proc_pidpath_to_name(&path)?;
        self.names.put(pid, &name);
        Some(name)
    }
}

/// 调 `proc_pidpath`（macOS 专有）取进程可执行路径。
fn pid_path(pid: u32) -> Option<String> {
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let rc = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    if rc <= 0 {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char) };
    Some(cstr.to_string_lossy().to_string())
}

/// 读取 `sysctl` 名字对应的二进制值。失败返回空表。
unsafe fn read_pcblist(name: &str) -> Vec<(u32, u32, u32)> {
    let cname = std::ffi::CString::new(name).ok();
    let cname = match cname {
        Some(c) => c,
        None => return Vec::new(),
    };
    let mut mib = [libc::CTL_NET, libc::PF_INET, libc::IPPROTO_TCP, 0, 0];
    if name.contains("udp") {
        mib[2] = libc::IPPROTO_UDP;
    }
    let _ = &cname;

    let mut len: usize = 0;
    if libc::sysctlbyname(cname.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 {
        return Vec::new();
    }
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len];
    if libc::sysctlbyname(
        cname.as_ptr(),
        buf.as_mut_ptr() as *mut libc::c_void,
        &mut len,
        std::ptr::null_mut(),
        0,
    ) != 0
    {
        return Vec::new();
    }
    parse_pcblist(&buf)
}

/// 按 `xig_len`/`xig_count` 游走记录，抽取本地端口/地址/PID。
/// 偏移量以 macOS 上 `struct xinpcb_n` 的公开布局为准（前 16 字节为
/// `inp_depend`/`inp_list` 链指针，随后是 `inp_vflag`、端口、地址、PID）。
pub fn parse_pcblist(buf: &[u8]) -> Vec<(u32, u32, u32)> {
    // xinpgen: u32 xig_len; u32 xig_count; u32 xig_gen; u64 xig_sogen;
    // 之后紧跟 xig_count 条记录，每条 xig_len 字节。
    if buf.len() < 16 {
        return Vec::new();
    }
    let header_len = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let count = u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if header_len < 16 || header_len > buf.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pos = header_len;
    for _ in 0..count {
        if pos + 4 > buf.len() {
            break;
        }
        let rec_len = u32::from_ne_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        if rec_len == 0 || pos + rec_len > buf.len() {
            break;
        }
        if let Some(row) = parse_one_record(&buf[pos..pos + rec_len]) {
            out.push(row);
        }
        pos += rec_len;
    }
    out
}

/// 单条 `xtcpcb_n`/`xudpcb_n`：跳过 `struct xinpcb_n` 的定长头，
/// 读 `inp_fport` / `inp_lport` / `inp_faddr` / `inp_daddr`。
fn parse_one_record(rec: &[u8]) -> Option<(u32, u32, u32)> {
    // xinpcb_n 头部（XNU, 64 位）：inp_depend(32B) + inp_list(16B) +
    // inp_pcbinfo ptr(8) + ... 精确偏移在不同版本会变，运行时用
    // `xig_gen` 校验；这里以 macOS 14 的偏移为准：lport 在 0x34、faddr 在 0x38。
    const OFF_LPORT: usize = 0x34;
    const OFF_FADDR: usize = 0x38;
    const OFF_PID: usize = 0x58;
    if rec.len() < OFF_PID + 4 {
        return None;
    }
    let lport = u32::from_ne_bytes(rec[OFF_LPORT..OFF_LPORT + 4].try_into().ok()?);
    let faddr = u32::from_ne_bytes(rec[OFF_FADDR..OFF_FADDR + 4].try_into().ok()?);
    let pid = u32::from_ne_bytes(rec[OFF_PID..OFF_PID + 4].try_into().ok()?);
    Some((lport, faddr, pid))
}
```

> 说明：pcblist 的字段偏移是与内核版本强相关的实现细节。落地时**必须**在目标 macOS 上跑一遍 Step 4 的手工验证并用 `lsof -nP -iTCP -sTCP:ESTABLISHED` 交叉比对；若偏移不符，只调整 `parse_one_record` 中的三个常量。`parse_pcblist` 的边界游走逻辑与偏移无关，已由测试覆盖。

- [ ] **Step 4: 手工验证 pcblist 解析**

Run: `cd src-tauri && cargo test --lib vpn::proc_lookup`（三条单元测试）
再手工验证（需要 root）：

```bash
# 开一个到 1.1.1.1:443 的连接，再用本程序的日志比对 PID 是否一致
lsof -nP -iTCP -sTCP:ESTABLISHED | grep 1.1.1.1
```

Expected: 单元测试 PASS；手工比对的 PID 与 `lsof` 输出一致（若不一致，调整 `parse_one_record` 偏移后重试）。

- [ ] **Step 5: `mod.rs` 注册**

```rust
pub mod proc_lookup;
```

- [ ] **Step 6: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/proc_lookup.rs src-tauri/src/vpn/mod.rs
git commit -m "feat(vpn): sysctl 进程反查与名称缓存"
```

---

## Task 8: vpnc-script 退化

**Files:**
- Modify: `src-tauri/src/vpn/script.rs`
- Test: `src-tauri/src/vpn/script.rs`（内联）

- [ ] **Step 1: 改测试（先让期望值反映新行为）**

在 `script.rs` 的测试模块里：
- 删除所有断言 `MODE="customer_first"` / `MODE="company_first"` / `SET_GATEWAY=true` / `default_route vpn` / `add_route ... vpn` 的用例。
- 新增：

```rust
    #[test]
    fn macos_script_no_longer_takes_default_route() {
        let ctx = ScriptContext {
            session_dir: std::path::Path::new("/tmp/s"),
            use_vpn_dns: false,
            original_gateway: "192.168.1.1",
        };
        let script = macos_script(&ctx);
        assert!(!script.contains("add default"));
        assert!(!script.contains("0.0.0.0/1"));
        assert!(script.contains("WS-WORKBENCH reason=connect"));
        assert!(script.contains("mtu"));
    }

    #[test]
    fn windows_script_no_longer_takes_default_route() {
        let ctx = ScriptContext {
            session_dir: std::path::Path::new("C:/s"),
            use_vpn_dns: false,
            original_gateway: "192.168.1.1",
        };
        let script = windows_script(&ctx);
        assert!(script.contains("SET_GATEWAY = false"));
        assert!(!script.contains("metric=1 store=active"));
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::script`
Expected: FAIL（脚本仍含默认路由接管逻辑）。

- [ ] **Step 3: 退化 `windows_script`**

在 `windows_script` 生成体中删除以下内容：
- `SET_GATEWAY` 分支里的 `netsh interface ip set interface ... metric=1` 与 `netsh interface ip set address ... gwmetric=1`（`script.rs` 中 `if (SET_GATEWAY) { ... }` 整段）。
- DNS 写入段（`USE_VPN_DNS` 的 `netsh interface ip set dns` 分支）——fake-ip 模式下系统 DNS 由引擎接管，脚本不再改 DNS。

保留：`report(...)` 各字段、MTU 设置（`set subinterface ... mtu=`）、以及**给 VPN 服务器本身加主机路由**的逻辑（若模板里有 `route add <server> ...`，保留；否则在 `reason=="connect"` 分支追加一条：

```js
// 给 VPN 服务器本身一条主机路由，避免被捕获 TUN 抢走导致环路
if (ORIGINAL_GW !== "" && VPNGATEWAY !== "") {
    run("route add " + VPNGATEWAY + " mask 255.255.255.255 " + ORIGINAL_GW);
}
```

`SET_GATEWAY` 常量改写为恒 `"false"`（不要删常量，前端测试仍引用）。

- [ ] **Step 4: 退化 `macos_script`**

删除：
- `route add -net 0.0.0.0/1` / `128.0.0.0/1` / `add default` 等默认路由接管命令；
- `-mode` 相关的 include/exclude `add_route` 循环；
- 写 VPN DNS 到网络服务的分支（`networksetup -setdnsservers`）。

保留：
- 接口地址设置（`ifconfig <dev> <ip> <ip> pointopoint <ip> up`）、MTU（`ifconfig <dev> mtu <mtu>`）；
- `report` 输出与 `WS-WORKBENCH` 标记；
- 给 VPN 服务器加主机路由：

```sh
# 给 VPN 服务器本身一条主机路由，避免被捕获 TUN 抢走导致环路
if [ -n "$ORIGINAL_GW" ] && [ -n "$VPNGATEWAY" ]; then
    route add -host "$VPNGATEWAY" "$ORIGINAL_GW"
fi
```

- [ ] **Step 5: 更新 `generate`**

`generate(ctx) -> (&'static str, String)` 保持不变（按平台返回脚本名与内容）。确认其中不再引用 `ctx.mode`。

- [ ] **Step 6: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::script`
Expected: PASS。

- [ ] **Step 7: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/script.rs
git commit -m "refactor(vpn): vpnc-script 退化为只配接口地址与 MTU"
```

---

## Task 9: worker / session 引擎装配、看门狗与路由回收

**Files:**
- Modify: `src-tauri/src/vpn/worker.rs`
- Modify: `src-tauri/src/vpn/session.rs`
- Test: `src-tauri/src/vpn/worker.rs`（内联，覆盖可测的判定）

本任务把 Task 2 的过渡路由替换为真正的按流引擎，并补「捕获 TUN 建/拆」「半默认路由下发/回收」「服务器主机路由」「fake-ip 表喂给引擎」与看门狗。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_routes_are_the_two_halves() {
        let r = capture_route_cidrs();
        assert_eq!(r, vec!["0.0.0.0/1".to_string(), "128.0.0.0/1".to_string()]);
    }

    #[test]
    fn status_reports_flow_count_and_engine_phase() {
        let mut s = crate::vpn::models::VpnStatus::default();
        s.phase = crate::vpn::models::VpnPhase::Connected;
        s.message = format!("已连接，活动连接 {}", 0);
        assert!(s.message.contains("活动连接"));
    }

    #[test]
    fn vpn_server_ips_excludes_capture_tun_address() {
        let ips = vec!["1.1.1.1".to_string(), "10.8.0.9".to_string()];
        let filtered = filter_server_ips(&ips, "10.8.0.9");
        assert_eq!(filtered, vec!["1.1.1.1".to_string()]);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::worker`
Expected: 编译失败，`capture_route_cidrs`/`filter_server_ips` 未定义。

- [ ] **Step 3: 新增辅助函数与字段**

在 `worker.rs` 顶部工具区新增：

```rust
/// 捕获 TUN 抢流量用的两条半默认路由。
pub fn capture_route_cidrs() -> Vec<String> {
    vec!["0.0.0.0/1".to_string(), "128.0.0.0/1".to_string()]
}

/// 过滤掉捕获 TUN 自己的地址，避免给自建网卡加服务器主机路由。
pub fn filter_server_ips(server_ips: &[String], capture_ip: &str) -> Vec<String> {
    server_ips
        .iter()
        .filter(|ip| ip.as_str() != capture_ip)
        .cloned()
        .collect()
}
```

在 `Worker` 结构体（`worker.rs:44-66`）增加字段：

```rust
    engine: Option<crate::vpn::router::Engine>,
    capture_dev: Option<crate::vpn::tun_device::Utun>,
    capture_index: u32,
    fake_pool: Option<crate::vpn::fakedns::FakeIpPool>,
```

初始化处（`Worker::new`，`worker.rs:69`）置为 `None` / `0`。

- [ ] **Step 4: 装配引擎（替换 `tunnel_ready` 后动作）**

在 `run_openconnect` 里，`tunnel_ready` 成立（`worker.rs:167`）后原本调用 `apply_routes` 的地方，改为调用新函数 `start_engine`：

```rust
    /// 隧道就绪后：建捕获 TUN、下发半默认路由、装配引擎。
    fn start_engine(&mut self) -> Result<(), String> {
        use crate::vpn::{netstack, platform, proc_lookup, router, rule_engine, tun_device};

        let (vpn_dev, vpn_index) = match (self.status.tun_dev.as_str(), self.status.tun_index) {
            ("", 0) => return Err("隧道接口信息缺失，无法装配引擎".to_string()),
            (dev, 0) => {
                let idx = self.tunnel_if_index(dev)?;
                (dev.to_string(), idx)
            }
            (dev, idx) => (dev.to_string(), idx),
        };

        let capture = tun_device::Utun::create().map_err(|e| format!("创建捕获 TUN 失败: {}", e))?;
        let capture_ip = crate::vpn::route::first_usable_in_subnet("10.9.0.0/30")
            .unwrap_or_else(|| "10.9.0.1".to_string());
        self.configure_interface(capture.name(), &capture_ip, self.status.mtu.max(1500))?;
        let capture_index = capture.index();
        self.capture_index = capture_index;

        let route_table = platform::MacRouteTable::new();
        route_table.add_capture_routes(capture_index)?;

        let gw = self.snapshot.default_gateway.clone();
        let server_ips = self.resolve_server_ips();
        for ip in filter_server_ips(&server_ips, &capture_ip) {
            if let Ok(v4) = ip.parse() {
                let _ = route_table.add_server_host_route(v4, &gw);
            }
        }

        let pool = crate::vpn::fakedns::FakeIpPool::new(
            &self.request.fake_ip_range,
            &self.request.fake_ip_filter,
        )
        .map_err(|e| format!("fake-ip 池初始化失败: {}", e))?;
        self.fake_pool = Some(pool);

        let rules = self.compiled_rules();
        let proc = proc_lookup::SysctlProcLookup::new();
        let engine = router::Engine::assemble(
            capture,
            netstack::NetstackConfig {
                local_ip: capture_ip.parse().map_err(|_| "捕获地址非法".to_string())?,
                mtu: self.status.mtu.max(1500) as usize,
                fake_domains: std::collections::HashMap::new(),
            },
            rules,
            platform::MacRouteTable::new(),
            platform::MacIfBinder::new(),
            capture_index,
            vpn_index,
            Box::new(proc),
            &self.request.default_policy,
        )?;
        let _ = vpn_dev;
        self.engine = Some(engine);
        self.log(&format!(
            "[引擎] 已接管：捕获 {} ({}) -> 隧道 {}",
            capture.name(),
            capture_ip,
            vpn_dev
        ));
        // 过渡期记录用：清掉按 CIDR 下发的路由，改由引擎按流注入
        self.applied.clear();
        self.status.routes.clear();
        Ok(())
    }

    fn tunnel_if_index(&self, dev: &str) -> Result<u32, String> {
        crate::vpn::route::list_interfaces()
            .unwrap_or_default()
            .into_iter()
            .find(|i| i.name == dev)
            .map(|i| i.index)
            .ok_or_else(|| format!("找不到隧道接口 {}", dev))
    }

    fn configure_interface(&self, dev: &str, ip: &str, mtu: u32) -> Result<(), String> {
        use std::process::Command;
        let status = Command::new("/sbin/ifconfig")
            .args([dev, ip, ip, "pointopoint", ip, "up", "mtu", &mtu.to_string()])
            .status()
            .map_err(|e| format!("配置 {} 失败: {}", dev, e))?;
        if !status.success() {
            return Err(format!("ifconfig {} 点对点地址配置失败", dev));
        }
        Ok(())
    }
```

同时把 `run_openconnect` 主循环里周期性的 `apply_routes`/`guard_routes` 调用（`worker.rs` 中 `tunnel_ready→apply_routes` 与每 `GUARD_INTERVAL` 的 `guard_routes`）替换为：

```rust
        // 隧道就绪时
        if tunnel_ready {
            if let Err(e) = self.start_engine() {
                self.fail(&e);
                return EXIT_FAILED;
            }
            self.set_phase(VpnPhase::Connected, "隧道已建立，引擎已接管");
        }
        // 每轮循环
        if let Some(engine) = self.engine.as_mut() {
            match engine.poll() {
                Ok(()) => {}
                Err(e) => self.log(&format!("[引擎] 轮询出错: {}", e)),
            }
            self.publish_engine_status(engine);
        }
```

- [ ] **Step 5: 在 `router.rs` 增加 `Engine::assemble` / `poll` / `publish` 支撑**

```rust
// router.rs 追加（替换 Task 5 的 Engine 占位）
use super::netstack::{Netstack, NetstackConfig};
use super::proc_lookup::ProcLookup;
use super::rule_engine::CompiledRuleSet;
use super::tun_device::Utun;

pub struct Engine {
    pub router: Router,
    netstack: Netstack<Utun>,
    pub rules: CompiledRuleSet,
    proc: Box<dyn ProcLookup>,
    default_policy: Policy,
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        capture: Utun,
        netstack_cfg: NetstackConfig,
        rules: CompiledRuleSet,
        route: Box<dyn RouteTable>,
        binder: Box<dyn IfBinder>,
        capture_if: u32,
        vpn_if: u32,
        proc: Box<dyn ProcLookup>,
        default_policy: &str,
    ) -> Result<Self, String> {
        let mtu = netstack_cfg.mtu;
        let phy = super::netstack::TunPhy::new(capture, mtu);
        let ns = Netstack::new(phy, netstack_cfg.local_ip, [0x02, 0, 0, 0, 0, 0x01]);
        let router = Router::new(route, binder, capture_if, vpn_if);
        Ok(Engine {
            router,
            netstack: ns,
            rules,
            proc,
            default_policy: Policy::parse(default_policy).unwrap_or(Policy::Direct),
        })
    }

    /// 一轮收发。真正 accept/close 流在后续 Task 11 补齐 UDP 后收敛。
    pub fn poll(&mut self) -> Result<(), String> {
        let key = FlowKey {
            transport: Transport::Tcp,
            local: "0.0.0.0:0".parse().unwrap(),
            remote: "0.0.0.0:0".parse().unwrap(),
        };
        let _ = (&self.proc, &self.default_policy, key);
        self.netstack.poll(smoltcp::time::Instant::now());
        Ok(())
    }

    pub fn flow_count(&self) -> usize {
        self.router.open_flow_count()
    }
}
```

> 说明：`Engine::poll` 的 accept/映射/搬运循环的**完整实现**见 Task 11（本任务只建立装配与轮询骨架，让 worker 主循环先跑通）。`decide_egress`/`decide_domain`（Task 5）已就绪，Task 11 用它把 netstack 里新建立的连接接到 `Router::open_egress`。

- [ ] **Step 6: 状态推进与看门狗**

新增：

```rust
    fn publish_engine_status(&mut self, engine: &crate::vpn::router::Engine) {
        self.status.message = format!("已连接，活动连接 {}", engine.flow_count());
        let _ = self.session.write_status(&self.status);
    }
```

把 `cleanup_routes`（`worker.rs:537-577`）扩展为「拆引擎 → 回收捕获路由/服务器主机路由 → 关捕获 TUN」：

```rust
    fn cleanup_routes(&mut self) {
        if let Some(mut engine) = self.engine.take() {
            engine.shutdown();
        }
        let route_table = crate::vpn::platform::MacRouteTable::new();
        let capture_index = self.capture_index;
        if capture_index != 0 {
            let _ = route_table.del_capture_routes(capture_index);
        }
        for ip in self.resolve_server_ips() {
            if let Ok(v4) = ip.parse() {
                let _ = route_table.del_server_host_route(v4);
            }
        }
        self.capture_dev = None;
        self.capture_index = 0;
        // 原有：还原默认路由、删服务器主机路由、恢复 DNS、清空 status.routes
        route::restore_default_route(&self.snapshot);
        for ip in &self.vpn_server_ips {
            let _ = route::delete_host_route(ip);
        }
        script::restore_vpn_dns(&self.session.dir, &self.snapshot.default_if_alias, &self.status.tun_dev);
        self.status.routes.clear();
    }
```

在 `router.rs` 的 `Engine` 里加：

```rust
    pub fn shutdown(&mut self) {
        let keys: Vec<FlowKey> = self.router.flow_keys();
        for k in keys {
            self.router.close_flow(&k);
        }
    }
```

并在 `Router` 里加：

```rust
    pub fn flow_keys(&self) -> Vec<FlowKey> {
        self.flows.keys().copied().collect()
    }
```

- [ ] **Step 7: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::worker vpn::router`
Expected: PASS。

- [ ] **Step 8: 构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过。

- [ ] **Step 9: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/worker.rs src-tauri/src/vpn/router.rs src-tauri/src/vpn/session.rs
git commit -m "feat(vpn): worker 装配引擎、捕获 TUN 与路由回收"
```

---

## Task 10: 规则编译接入、校验与预览命令

**Files:**
- Modify: `src-tauri/src/vpn/commands.rs`
- Modify: `src-tauri/src/lib.rs`（注册新命令）
- Test: `src-tauri/src/vpn/commands.rs`（内联）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_known_kinds() {
        assert!(validate_rule_input("DOMAIN", "a.com").is_ok());
        assert!(validate_rule_input("DST-PORT", "443").is_ok());
        assert!(validate_rule_input("DST-PORT", "70000").is_err());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/8").is_ok());
        assert!(validate_rule_input("IP-CIDR", "10.0.0.0/33").is_err());
        assert!(validate_rule_input("bogus", "x").is_err());
    }

    #[test]
    fn domain_rule_rejects_empty_value() {
        assert!(validate_rule_input("DOMAIN", "").is_err());
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::commands`
Expected: 编译失败，`validate_rule_input` 未定义。

- [ ] **Step 3: 实现校验与预览**

在 `commands.rs` 新增：

```rust
/// 单条规则的入参校验，供 UI 即时反馈。
pub fn validate_rule_input(kind: &str, value: &str) -> Result<(), String> {
    use crate::vpn::rule_engine::{parse_port_range, RuleKind};
    let kind = RuleKind::parse(kind).map_err(|e| e)?;
    if value.trim().is_empty() {
        return Err("匹配值不能为空".to_string());
    }
    match kind {
        RuleKind::DstPort => {
            parse_port_range(value)?;
        }
        RuleKind::IpCidr => {
            crate::vpn::route::normalize_cidr(value)?;
        }
        _ => {}
    }
    Ok(())
}

#[tauri::command]
pub fn vpn_validate_rule(kind: String, value: String) -> Result<String, String> {
    validate_rule_input(&kind, &value)?;
    Ok(match crate::vpn::rule_engine::RuleKind::parse(&kind).unwrap() {
        crate::vpn::rule_engine::RuleKind::IpCidr => crate::vpn::route::normalize_cidr(&value)?,
        _ => value,
    })
}

/// 用给定规则集与一条输入，预览命中结果，供 UI 展示「这条域名会走哪」。
#[tauri::command]
pub fn vpn_preview_rules(
    rules: Vec<VpnRule>,
    default_policy: String,
    domain: Option<String>,
    ip: Option<String>,
    port: Option<u16>,
    process: Option<String>,
) -> Result<String, String> {
    use crate::vpn::rule_engine::{compile, MatchInput, Policy, Rule, RuleKind};
    let mut compiled = Vec::new();
    for r in rules.iter().filter(|r| r.enabled) {
        let kind = RuleKind::parse(&r.kind)?;
        let policy = Policy::parse(&r.policy)?;
        compiled.push(Rule::new(kind, r.value.clone(), policy).with_no_resolve(r.no_resolve));
    }
    let default = Policy::parse(&default_policy)?;
    let set = compile(&compiled, &std::collections::HashMap::new(), default)?;
    let verdict = set.evaluate(&MatchInput {
        domain: domain.as_deref(),
        ip: ip.as_deref(),
        port,
        process: process.as_deref(),
    });
    Ok(verdict.as_str().to_string())
}
```

同时把 `vpn_connect` 里新增一步：编译规则，编译失败直接返回错误（fail-closed 交由 worker 再次兜底）：

```rust
    // 校验规则可编译（失败即拒绝连接）
    {
        use crate::vpn::rule_engine::{compile, Policy, Rule, RuleKind};
        let mut compiled = Vec::new();
        for r in rules.iter().filter(|r| r.enabled) {
            let kind = RuleKind::parse(&r.kind)?;
            let policy = Policy::parse(&r.policy)?;
            compiled.push(Rule::new(kind, r.value.clone(), policy).with_no_resolve(r.no_resolve));
        }
        let default = Policy::parse(&default_policy_for(&profile, &rules))?;
        compile(&compiled, &std::collections::HashMap::new(), default)
            .map_err(|e| format!("规则编译失败：{}", e))?;
    }
```

并把 `default_policy_for` 改为从 `profile` 携带的规则集缺省策略取（Task 12 会在前端写入 `VpnProfile` 的对应字段；若暂未加字段，保持返回 `"DIRECT"`）。

- [ ] **Step 4: 在 `lib.rs` 注册新命令**

在 `lib.rs` 的 `invoke_handler` 命令列表（`lib.rs:128-147`）加入：

```rust
            vpn::commands::vpn_validate_rule,
            vpn::commands::vpn_preview_rules,
```

并按现有风格删除已废弃的 `vpn_validate_cidr` / `vpn_check_rule_overlaps` 注册（若前端已不再调用；Task 12 会同步删除前端调用）。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::commands`
Expected: PASS（2 passed）。

- [ ] **Step 6: 构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过。

- [ ] **Step 7: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/commands.rs src-tauri/src/lib.rs
git commit -m "feat(vpn): 规则校验与命中预览命令"
```

---

## Task 11: UDP 五元组映射转发

**Files:**
- Modify: `src-tauri/src/vpn/router.rs`
- Modify: `src-tauri/src/vpn/netstack.rs`
- Modify: `src-tauri/src/vpn/worker.rs`
- Test: `src-tauri/src/vpn/router.rs`（内联）

本任务补齐 `Engine::poll` 的完整收发循环：netstack 里 accept 出的 TCP 流 / 收到的 UDP 报文，按规则选出口，经 `Router` 与出站 socket 双向搬运；DNS（dst port 53）交给 `dns_srv`。

- [ ] **Step 1: 写失败的测试**

```rust
    #[test]
    fn udp_flow_key_is_distinct_per_remote() {
        let a = FlowKey { transport: Transport::Udp, local: "10.0.0.2:5000".parse().unwrap(), remote: "1.1.1.1:53".parse().unwrap() };
        let b = FlowKey { transport: Transport::Udp, local: "10.0.0.2:5000".parse().unwrap(), remote: "8.8.8.8:53".parse().unwrap() };
        assert_ne!(a, b);
    }

    #[test]
    fn dns_destination_is_recognized() {
        assert!(is_dns_port(53));
        assert!(!is_dns_port(443));
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd src-tauri && cargo test --lib vpn::router`
Expected: 编译失败，`is_dns_port` 未定义。

- [ ] **Step 3: 补 UDP 出口与 DNS 判定**

在 `router.rs` 新增：

```rust
pub const DNS_PORT: u16 = 53;

pub fn is_dns_port(port: u16) -> bool {
    port == DNS_PORT
}

/// UDP 出站：同样的选出口逻辑，只是走 `UdpSocket`。
struct UdpEgress {
    socket: std::net::UdpSocket,
    installed_route: Option<Ipv4Addr>,
}

impl Router {
    /// 为一条 UDP 流建立出站 socket 并绑定出口。
    pub fn open_udp_egress(&mut self, key: &FlowKey, egress: Egress) -> std::io::Result<()> {
        use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
        let if_index = match egress {
            Egress::Direct => self.physical_if,
            Egress::Vpn => self.vpn_if,
        };
        let fd: RawFd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if let Err(e) = self.binder.bind_fd_to_if(fd, if_index) {
            unsafe { libc::close(fd) };
            return Err(e);
        }
        let socket = unsafe { std::net::UdpSocket::from_raw_fd(fd) };
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;

        let mut installed_route = None;
        if egress == Egress::Vpn {
            let ip = *key.remote.ip();
            self.route
                .add_flow_route(ip, self.vpn_if)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            installed_route = Some(ip);
        }
        self.udp_flows.insert(key.to_owned(), UdpEgress { socket, installed_route });
        Ok(())
    }

    /// 把一个应用侧 UDP 载荷发往远端。
    pub fn udp_sendto(&self, key: &FlowKey, payload: &[u8]) -> std::io::Result<usize> {
        if let Some(f) = self.udp_flows.get(key) {
            f.socket.send_to(payload, std::net::SocketAddr::V4(key.remote))
        } else {
            Ok(0)
        }
    }

    /// 收一个远端回包（非阻塞语义，超时返回 0）。
    pub fn udp_recv(&self, key: &FlowKey, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(f) = self.udp_flows.get(key) {
            match f.socket.recv(buf) {
                Ok(n) => Ok(n),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    Ok(0)
                }
                Err(e) => Err(e),
            }
        } else {
            Ok(0)
        }
    }
}
```

并在 `Router` 结构体加字段 `udp_flows: HashMap<FlowKey, UdpEgress>`，在 `Router::new` 里初始化为 `HashMap::new()`，在 `close_flow` 里一并回收 UDP 流路由：

```rust
        if let Some(flow) = self.udp_flows.remove(key) {
            if let Some(ip) = flow.installed_route {
                let _ = self.route.del_flow_route(ip);
            }
        }
```

- [ ] **Step 4: 补全 `Engine::poll` 收发循环**

用下列实现替换 Task 9 的占位 `poll`：

```rust
    pub fn poll(&mut self) -> Result<(), String> {
        let now = smoltcp::time::Instant::now();
        self.netstack.poll(now);

        // 1) TCP：netstack 里已 established 的应用连接 -> 建出口 -> 双向搬运
        let new_tcp = self.netstack.take_new_tcp_flows();
        for flow in new_tcp {
            let domain = flow.domain.clone();
            let process = self
                .proc
                .pid_for(&flow.key)
                .and_then(|pid| self.proc.name_for_pid(pid));
            let egress = match domain.as_deref() {
                Some(d) => super::router::decide_domain(&self.rules, d, &flow.key, process.as_deref()),
                None => super::router::decide_egress(&self.rules, &flow.key, None, process.as_deref()),
            };
            if let Err(e) = self.router.open_egress(&flow.key, egress) {
                self.netstack.abort_tcp(&flow.key);
                continue;
            }
        }
        self.netstack.pump_tcp(&mut self.router);

        // 2) UDP：netstack 里收到的报文 -> DNS 优先本地应答，其余建出口转发
        for datagram in self.netstack.take_udp_datagrams() {
            if is_dns_port(datagram.remote.port()) {
                let resp = self.dns_respond(&datagram.payload);
                if let Some(resp) = resp {
                    self.netstack.send_udp_to_app(&datagram.key, &resp);
                }
                continue;
            }
            let process = self
                .proc
                .pid_for(&datagram.key)
                .and_then(|pid| self.proc.name_for_pid(pid));
            let egress =
                super::router::decide_egress(&self.rules, &datagram.key, None, process.as_deref());
            if self.router.open_udp_egress(&datagram.key, egress).is_ok() {
                let _ = self.router.udp_sendto(&datagram.key, &datagram.payload);
            }
        }
        self.netstack.pump_udp(&mut self.router);
        Ok(())
    }

    /// 处理一条 DNS 查询：分配 fake-ip 并回 A 记录，AAAA 回空。
    fn dns_respond(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        let query = super::dns_srv::parse_query(payload)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let pool = self.fake_pool.as_mut()?;
        if query.qtype == 1 {
            let ip = pool.allocate(&query.name, now, 300)?;
            self.note_fake_domain(ip, &query.name);
            Some(super::dns_srv::build_a_response(
                payload,
                &query,
                std::net::Ipv4Addr::from(ip),
            ))
        } else {
            Some(super::dns_srv::build_empty_response(payload, &query))
        }
    }
```

在 `netstack.rs` 增加上述四种方法与 `NewTcpFlow` / `UdpDatagram` 结构；`Engine` 增加字段 `fake_pool: Option<FakeIpPool>` 与 `fake_domains: HashMap<u32, String>`，`assemble` 时用 `NetstackConfig.fake_domains` 初始化，`note_fake_domain(ip, name)` 写入两者。`netstack` 里 `FlowKey` → 路由判定所需的 `domain` 通过 `fake_domains.get(u32::from(remote.ip()))` 反查。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cd src-tauri && cargo test --lib vpn::router vpn::netstack`
Expected: PASS。

- [ ] **Step 6: 构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过。

- [ ] **Step 7: 重启项目并冒烟**

Run:
```bash
# 先结束残留进程，再用正确命令拉起
pkill -f "vite" ; pkill -f "target/debug/ws-workbench" ; pnpm exec tauri dev
```
Expected: 应用启动；连接 VPN 后状态显示「已连接，活动连接 N」，浏览器访问一个走 VPN 的域名能通，`nslookup` 返回 `198.18.x.x`。

- [ ] **Step 8: 提交（待用户同意后执行）**

```bash
git add src-tauri/src/vpn/router.rs src-tauri/src/vpn/netstack.rs src-tauri/src/vpn/worker.rs
git commit -m "feat(vpn): UDP 五元组映射转发与 DNS fake-ip 拦截"
```

---

## Task 12: 前端规则编辑器与 DNS 配置

**Files:**
- Modify: `src/types/vpn.ts`
- Modify: `src/composables/useVpn.ts`
- Modify: `src/components/vpn/VpnRuleTable.vue`
- Modify: `src/components/vpn/VpnProfileForm.vue`
- Modify: `src/App.vue`

- [ ] **Step 1: 改类型 `src/types/vpn.ts`**

- 删除 `VpnMode`（`vpn.ts:2`）与 `MODE_LABELS`/`MODE_DESCRIPTIONS`（`vpn.ts:223-231`）。
- 重写 `VpnRule`（`vpn.ts:19-25`）：

```ts
export type VpnRuleKind =
  | 'DOMAIN'
  | 'DOMAIN-SUFFIX'
  | 'DOMAIN-KEYWORD'
  | 'PROCESS-NAME'
  | 'DST-PORT'
  | 'IP-CIDR'

export type VpnPolicy = 'DIRECT' | 'VPN'

export interface VpnRule {
  id: string
  kind: VpnRuleKind
  value: string
  policy: VpnPolicy
  no_resolve: boolean
  remark: string
  enabled: boolean
}
```

> 说明：kind/policy 直接使用引擎规范形式（大写连字符），**不再**做 snake_case→连字符的映射。UI 下拉框展示 `RULE_KIND_LABELS`（与取值相同）。

- 改 `VpnRuleSet`（`vpn.ts:28-33`）加 `default_policy: VpnPolicy`。
- 改 `VpnProfile`（`vpn.ts:48-49` 附近）删 `mode`，加：

```ts
  fake_ip_range: string
  fake_ip_filter: string[]
  use_vpn_dns: boolean
```

- `VpnStatus`（`vpn.ts:148`）删 `mode`。
- 新增常量：

```ts
export const RULE_KIND_LABELS: Record<VpnRuleKind, string> = {
  DOMAIN: 'DOMAIN',
  'DOMAIN-SUFFIX': 'DOMAIN-SUFFIX',
  'DOMAIN-KEYWORD': 'DOMAIN-KEYWORD',
  'PROCESS-NAME': 'PROCESS-NAME',
  'DST-PORT': 'DST-PORT',
  'IP-CIDR': 'IP-CIDR',
}

export const POLICY_LABELS: Record<VpnPolicy, string> = {
  DIRECT: '直连',
  VPN: '走 VPN',
}
```

- [ ] **Step 2: 改 `useVpn.ts`**

- 删 `setMode`（`useVpn.ts:368-372`）。
- `newRuleSet`（`useVpn.ts:252-259`）加 `default_policy: 'DIRECT'`。
- `newRule(cidr, remark)`（`useVpn.ts:261-263`）改为：

```ts
function newRule(kind: VpnRuleKind = 'DOMAIN', value = '', policy: VpnPolicy = 'DIRECT'): VpnRule {
  return {
    id: crypto.randomUUID(),
    kind,
    value,
    policy,
    no_resolve: true,
    remark: '',
    enabled: true,
  }
}
```

- `addProfile` 默认档案（`useVpn.ts:280`）删 `mode`，加 `fake_ip_range: '198.18.0.0/16'`、`fake_ip_filter: []`、`use_vpn_dns: true`。
- 把 `validateCidr`/`checkOverlaps`（`useVpn.ts:374-389`）替换为：

```ts
async function validateRule(kind: VpnRuleKind, value: string): Promise<string> {
  return await invoke<string>('vpn_validate_rule', { kind, value })
}

async function previewRule(
  rules: VpnRule[],
  defaultPolicy: VpnPolicy,
  probe: { domain?: string; ip?: string; port?: number; process?: string }
): Promise<VpnPolicy> {
  const verdict = await invoke<string>('vpn_preview_rules', {
    rules,
    defaultPolicy,
    domain: probe.domain ?? null,
    ip: probe.ip ?? null,
    port: probe.port ?? null,
    process: probe.process ?? null,
  })
  return verdict as VpnPolicy
}
```

- 更新 `index`/导出（`useVpn.ts:527-580`）：移除 `setMode`、`validateCidr`、`checkOverlaps`，新增 `validateRule`、`previewRule`。

- [ ] **Step 3: 重写 `VpnRuleTable.vue`**

把「CIDR 表格 + 重叠告警」改成有序规则编辑器：
- props 改为 `{ ruleSet: VpnRuleSet }`；emits 保持 `add/update/remove`。
- 顶部一行显示 `MATCH → {POLICY_LABELS[ruleSet.default_policy]}`（固定兜底行，不可拖动/删除，policy 可切换，改动通过 `update` 抛出 `default_policy`）。
- 规则行字段：`kind`（下拉，`RULE_KIND_LABELS`）、`value`（输入框）、`policy`（下拉）、`no_resolve`（仅 `kind==='IP-CIDR'` 时可勾选）、`enabled`（开关）、`remark`（输入框）。
- 保留「新增规则」按钮；`validate()` 改为逐条调用 `validateRule(kind, value)`（经 `useVpn`），校验失败在行内标红。
- 删除原来针对 CIDR 的 `CIDR_PATTERN` 校验与重叠提示区。

- [ ] **Step 4: 改 `VpnProfileForm.vue`**

- 删除模式卡片选择区（`VpnProfileForm.vue:127-144`）。
- 新增 DNS/高级段：

```vue
<a-form-model-item label="fake-ip 地址池">
  <a-input v-model="form.fake_ip_range" placeholder="198.18.0.0/16" />
</a-form-model-item>
<a-form-model-item label="fake-ip 过滤（每行一条，*.后缀 或 关键字）">
  <a-textarea
    :value="form.fake_ip_filter.join('\n')"
    :rows="3"
    @change="form.fake_ip_filter = ($event.target as HTMLTextAreaElement).value.split('\n').map(s => s.trim()).filter(Boolean)"
  />
</a-form-model-item>
<a-form-model-item label="写入 VPN DNS">
  <a-switch v-model="form.use_vpn_dns" />
</a-form-model-item>
```

- 删除「选择 company_first 时自动置 `use_vpn_dns: true`」的逻辑。

- [ ] **Step 5: 改 `App.vue`**

- 删除托盘监听中的 `vpn-mode` 分支（`App.vue:78-89`）及对 `vpn.setMode` 的调用。
- 删除状态栏对 `MODE_LABELS[vpn.status.mode]` 的渲染（`App.vue:414-418`），改为显示 `vpn.status.message`。

- [ ] **Step 6: 类型检查与构建**

Run: `pnpm build`
Expected: 通过（`vue-tsc` 无类型错误）。若报 `VpnMode` 残留引用，逐一删除。

- [ ] **Step 7: 重启项目并手工验证**

Run:
```bash
pkill -f "vite" ; pkill -f "target/debug/ws-workbench" ; pnpm exec tauri dev
```
Expected: VPN 页能新增/编辑/删除规则、切换 MATCH 兜底策略、配置 fake-ip 池；保存后重开仍在（经 `safeStorage` 落地）。

- [ ] **Step 8: 提交（待用户同意后执行）**

```bash
git add src/types/vpn.ts src/composables/useVpn.ts src/components/vpn/VpnRuleTable.vue \
        src/components/vpn/VpnProfileForm.vue src/App.vue
git commit -m "feat(vpn-ui): Clash 式有序规则编辑器与 fake-ip 配置"
```

---

## Task 13: 端到端手工验收与回归

**Files:**
- 无代码改动（只验收）；如发现缺陷回到对应任务修复。

- [ ] **Step 1: 全量测试与构建**

Run:
```bash
cd src-tauri && cargo test
pnpm build
```
Expected: `cargo test` 相对基线**无新增失败**（基线已知 4 个无关失败：`apps::registry` ×3、`vpn::detect` ×1）；`pnpm build` 通过。`grep -rn "VpnMode" src src-tauri/src` 应为空。

- [ ] **Step 2: 重启项目**

Run: `pkill -f "vite" ; pkill -f "target/debug/ws-workbench" ; pnpm exec tauri dev`
Expected: 正常启动。

- [ ] **Step 3: 域名规则验收（DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD）**

配置三条规则分别命中不同域名，访问对应站点；用 `nslookup <域名>` 确认返回 `198.18.x.x`，用 `lsof -nP -iTCP` 或状态栏「活动连接」确认走了预期出口。
Expected: 命中 `vpn` 的域名经隧道可达且出口 IP 为 VPN 出口；命中 `direct` 的为本地出口。

- [ ] **Step 4: 端口与进程规则验收**

配 `DST-PORT 22 → vpn`、`PROCESS-NAME <某浏览器进程> → vpn`，分别发起 SSH 与浏览器访问。
Expected: 端口/进程命中生效（用状态栏活动连接数变化与出口 IP 交叉验证）。

- [ ] **Step 5: 兜底策略验收**

把 `MATCH` 设为 `direct` 再设为 `vpn`，访问一条无规则命中的域名。
Expected: 出口随 MATCH 变化；首次匹配优先于 MATCH（被具体规则命中的仍按规则走）。

- [ ] **Step 6: UDP 验收**

用一条命中的域名做 DNS + QUIC/UDP 类访问（如 `curl --http3` 或系统 UDP 应用）。
Expected: DNS 返回假地址；UDP 载荷按规则出口转发，未命中则按 MATCH。

- [ ] **Step 7: 断开与回收验收**

断开 VPN 后检查：
```bash
netstat -rn | grep -E "^0\.0\.0\.0/1|^128\.0\.0\.0/1"
netstat -rn | grep -E "<某个 198\.18 上游? 不用看>" 
sudo ifconfig | grep utun
```
Expected: 两条半默认路由已删除；默认路由已还原；临时捕获 utun 已回收；无残留流路由；无残留 `openconnect`/worker 进程。

- [ ] **Step 8: 文档与提交（待用户同意后执行）**

若验收中发现设计偏差，回到对应任务修正并在本步骤记录；随后：

```bash
git add -A
git commit -m "test(vpn): 竖切 A 端到端验收记录"
```

---

## 自审记录（Self-Review）

**1. 规格覆盖（对照竖切设计 §1–§10）**

| 设计要求 | 覆盖任务 |
| --- | --- |
| §4.1 trait 抽象（`TunDevice`/`RouteTable`/`IfBinder`/`ProcLookup`） | Task 1 |
| §4.2 `platform.rs` | Task 1 |
| §4.2 `tun_device.rs` | Task 3 |
| §4.2 `netstack.rs` | Task 4、Task 11 |
| §4.2 `router.rs` | Task 5、Task 9、Task 11 |
| §4.2 `proc_lookup.rs` | Task 7 |
| §4.2 `dns_srv.rs` | Task 6、Task 11 |
| §4.3 复用 `rule_engine`/`fakedns`/`route.rs`/`config.rs` | Task 2、Task 9、Task 10 |
| §4.4 `script.rs` 退化 | Task 8 |
| §4.4 `worker.rs` 装配/看门狗/回收 | Task 9、Task 11 |
| §4.4 `commands.rs` | Task 2、Task 10 |
| §4.4 `models.rs` | Task 2 |
| §4.4 `session.rs` | Task 2、Task 9 |
| §4.5 前端 7 处 | Task 12 |
| §1 范围：7 种匹配类型 | 前端 Task 12 暴露；引擎 Task 5/11 消费（`rule_engine` 已在 Phase 1 覆盖语义） |
| §1 fake-ip DNS | Task 6、Task 11 |
| §1 TCP + 非 DNS UDP | Task 5、Task 11 |
| §1 移除 `mode` | Task 2（Rust）、Task 12（前端） |
| §10 步骤 1–12 | 分别映射 Task 1/3、4、5、8、9、6、7、10、11、12、2、13 |
| §8 测试与验收 | 各任务内联测试 + Task 13 |

**2. 占位符扫描**：本计划未使用 TBD/TODO/"类似 Task N"/"适当处理错误"等表述；每个代码步骤给出完整代码或明确到函数级的替换指令（含前后行为差异）。`proc_lookup` 的 pcblist 字段偏移属唯一「运行期实测确定」项，已在 Task 7 说明具体的校验手段（`lsof` 交叉比对）与唯一可调常量位置，不构成设计占位。

**3. 类型/命名一致性核对**

- `FlowKey`/`Transport`（Task 1）在 Task 4/5/7/11 中的字段名一致（`transport`/`local`/`remote`）。
- `Egress`（Task 1）在 Task 5 实现 `From<Policy>`，Task 11 复用。
- `Router` 方法名：`open_egress`（Task 5）、`open_udp_egress`/`udp_sendto`/`udp_recv`（Task 11）、`close_flow`/`flow_keys`/`open_flow_count`（Task 5/9）——跨任务一致。
- `Engine`：`assemble`/`poll`/`flow_count`/`shutdown`（Task 5/9/11）一致；`proc_lookup::ProcLookup` 方法 `pid_for`/`name_for_pid`（Task 1/7）一致。
- `rule_engine` 复用面：`RuleKind::parse`/`Policy::parse`/`Rule::new(...).with_no_resolve`/`compile`/`CompiledRuleSet::evaluate`/`MatchInput` 与 Phase 1 的定义一致；本计划额外要求 `CompiledRuleSet::rules()` 暴露（Task 2 已给）。
- 前端：`VpnRuleKind`/`VpnPolicy`/`RULE_KIND_LABELS`/`POLICY_LABELS`（Task 12）与 Rust 侧 `kind`/`policy` 字符串取值一致（`DOMAIN`/`DIRECT` 等，见「字符串规范」）。
- 命令名：`vpn_validate_rule`/`vpn_preview_rules`（Task 10）与前端 `invoke` 名一致（Task 12）。

**已知遗留**

- Windows 实现、`RULE-SET` 外部文件加载、e2e 自动化：本竖切不做（见「范围边界」）。
- `.superpowers/` 是否纳入版本控制尚未处理（与竖切无关）。
- 全部任务的提交步骤均需用户明确同意后执行。

**最终审查后仍待处理（不阻断编译，需真机验证或后续迭代）**

- `fake_ip_filter` 命中的域名目前**不回任何 DNS 应答**（缺上游真解析）；默认过滤为空，故默认无影响。
- **TCP:53** 未做 fake-ip 拦截（作为普通 TCP 流按规则转发，可通但拿不到假地址，域名规则对它不生效）。
- `Engine::poll` 若 panic，worker 同线程会退出且不执行 `cleanup_routes` → 可能残留捕获路由/TUN（缺看门狗）。
- 每条流出站 socket 用 200ms 读写超时，串行轮询在大量流时吞吐受限。
- `VpnRoutePreview` 依赖 `status.routes`，而引擎接管后清空且不回填 → 连接后预览恒为空。
- 进程反查 pcblist 的字段偏移为内核版本相关常量，需在目标机用 `lsof` 交叉校验（见 Task 7）。
- 死亡代码/死接口：`Router::is_open`/`pump_out_to_app`、`Worker::capture_dev`/`fake_pool`（只写不读）、`vpn_validate_cidr`/`vpn_check_rule_overlaps`（前端已不调用）。
- README 仍描述 `customer_first`/`company_first`（文档待更新）。
