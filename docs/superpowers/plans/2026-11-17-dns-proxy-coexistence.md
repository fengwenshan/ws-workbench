# DNS 分流与本地代理共存（实施计划）

对应设计：[2026-11-17-dns-proxy-coexistence-design.md](../specs/2026-11-17-dns-proxy-coexistence-design.md)

## 1. 转发器与 53 端口

- [x] 新建 `src-tauri/src/vpn/local_dns/{mod.rs,windows.rs,macos.rs}`，`mod.rs` 只做平台分派。
- [x] `vpn/mod.rs` 注册模块；`split.rs` 删掉 `FORWARDER_ADDR` / `LOCAL_DNS_SERVER`，改用 `local_dns::*`。
- [x] macOS：直接绑 `127.0.0.1:0`（随机高位端口），不再「先试 53、失败再换端口」。
- [x] Windows 先按「专用回环 `127.0.0.2:53`」实现，实机证伪（能绑上、探针 2.5 秒零查询）；
      改回 `127.0.0.1:53` 再试，同样零查询。
- [x] 结论落地：Windows **不用转发器、不绑 53**，NRPT 直接指向 VPN DNS，子域靠
      「扫系统 DNS 缓存 + 主动解析」补齐；删掉 `start_windows_local_proxy` /
      `local_proxy_receives_queries` / `stop_local_forwarder` / `nrpt_servers` / `PROBE_TIMEOUT`。
- [x] 转发线程本身（`ForwarderCtx` / `forward_loop` / `install_dynamic_routes`）收为
      macOS + 单测可见，避免 Windows 侧死代码告警。
- [x] 更新 `split.rs` 顶部对照表与相关注释。

## 2. 系统代理检测

- [x] 新建 `src-tauri/src/vpn/proxy/{mod.rs,common.rs,windows.rs,macos.rs,unsupported.rs}`。
- [x] `common.rs`：`bypass_covers` + `warning_for`，纯函数 + 单测（9 个）。
- [x] `windows.rs`：从注册表读 WinINET 代理；`macos.rs`：解析 `scutil --proxy`。
- [x] `split.rs` 在布线完成后调用，警告写 worker 日志并进 `VpnStatus.warnings`。

## 3. 状态与界面

- [x] `models.rs`：`VpnStatus.warnings`（`#[serde(default)]` + Default）。
- [x] `worker.rs`：布线后 `status.warnings = split.warnings()`，全量模式与
      `routes.clear()` 处一并清空。
- [x] `src/types/vpn.ts`：状态类型加 `warnings`。
- [x] `VpnPanel.vue`：`warnings` 非空时在代理页顶部显示可关闭的提示条，
      连接身份变化时复位关闭状态。

## 4. NRPT 写入收敛

- [x] 命名空间 + `NameServers` 都没变时跳过重写（`installed_rules` + `rules_cover`）。
- [x] 写完校验，缺失时重写一次并把实际规则写日志。

## 5. 文档与验证

- [x] `docs/features.md`：改写两个平台的解析路径，补「系统代理会绕开」的提示与处理方式。
- [x] `docs/development.md` / `docs/architecture.md`：新增 `vpn/local_dns/`、`vpn/proxy/`。
- [x] `AGENTS.md` §7：把两个新模块登记为平台拆分范本。
- [x] 前端：`vue-tsc --noEmit` + `vite build` 绿。
- [x] Rust：`cargo check --lib --tests` 绿且无 warning。
- [ ] 实机（Windows）：连 VPN → 日志「不用本机转发器…不占用 53」、53 上没有我们的进程、
      `Resolve-DnsName oa.risencn.com` 返回内网地址；系统代理开着时日志与界面出现代理警告。
- [ ] macOS：`/etc/resolver` 里 `port` 为随机高位端口、53 未被占用（本机无法实跑，CI + 说明）。
