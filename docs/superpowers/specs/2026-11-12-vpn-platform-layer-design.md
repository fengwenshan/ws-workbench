# VPN 连接器平台层拆分设计

**日期：** 2026-11-12（2026-11-13 修订：macOS 改为只认安装包内置的静态 openconnect）
**状态：** 已经用户确认（方案 A：删除 detect.rs，vpn::platform 成为唯一入口；按平台配置各自管理打包资源）

## 目标

把 `src-tauri/src/vpn/detect.rs` 里「共享逻辑 + `#[cfg(target_os)]` 分支」混在一起的结构，
拆成按平台分文件的连接器平台层：

```
src-tauri/src/vpn/platform/
  mod.rs      唯一做平台分派的地方（整文件 cfg 选择），并对外导出公共 API
  common.rs   共享层：probe、--version 解析、来源展示名、错误描述、pick/fetch、模型
  macos.rs    macOS：检测（只认安装包内置副本）、导入（明确拒绝）、移除、旧登记/副本清理
  windows.rs  Windows：检测（只认安装包内置副本）、导入（明确拒绝）、移除（无登记可移）
```

拆分后：

- 改 macOS 的检测/导入只会碰 `macos.rs`，不可能影响 Windows 编出来的代码；
- 改 Windows 同理；
- 共享层只保留真正跨平台一致的东西：探测子进程、版本号解析、`ConnectorInfo` 选择逻辑、
  `ImportedOpenconnect` 模型；
- `#[cfg]` 只出现在 `platform/mod.rs` 的模块选择与再导出上，平台文件内部不再写平台分支。

## 为什么不是保留 detect.rs 门面

保留门面只是把文件挪走，调用方仍经过一层空壳，且 README / 注释里会同时存在两个概念。
用户明确要「三个部分」的平台层，因此采用方案 A：删除 `detect.rs`，`vpn::platform` 直接对外。

## 对外 API

`vpn::platform` 导出：

- `detect_all(ctx: &AppContext, custom_openconnect: &str) -> Vec<ConnectorInfo>`（平台实现）
- `import_openconnect(data_dir: &Path, source: &Path) -> Result<ImportedOpenconnect, String>`（平台实现）
- `remove_managed_openconnect(data_dir: &Path) -> Result<(), String>`（平台实现）
- `pick_connector(candidates: &[ConnectorInfo], want: ConnectorKind) -> Option<&ConnectorInfo>`（common）
- `fetch_version(info: &ConnectorInfo) -> String`（common）
- `ImportedOpenconnect`（common）

调用方只有 `vpn/commands.rs` 的 4 处：`detect_all`、`import_openconnect`、
`remove_managed_openconnect`、`pick_connector`。

## 各平台行为（保持现状，不趁机改产品行为）

### macOS

2026-11-13 起与 Windows 一致：**只认安装包内置副本**，不再读 Homebrew / 标准前缀 / PATH /
用户指定的路径，也不提供「选择安装目录」入口。

- 唯一候选：`<资源目录>/apps/openconnect/openconnect`（源码在
  `src-tauri/resources/openconnect-macos/`，`tauri.macos.conf.json` 负责映射；调试构建在资源
  未拷进 target 时回退到源码树）。只认无后缀的 `openconnect`，不会把 Windows 的
  `openconnect.exe` 当成候选。
- 探测仍然真跑一次 `--version`，跑不起来就报「安装包缺少内置 openconnect」并给出资源路径。
- 旧版本留下的 `<data>/vpn/openconnect-path.txt`、`<data>/apps/openconnect/`、
  `<data>/connectors/openconnect/` 在每次检测时清理；内置副本缺失就回到「重新安装」引导，
  不会偷偷退回外置那份。
- 导入：直接返回「只使用安装包内置的 openconnect，不支持导入外部副本」。
- 移除：没有登记可移；调用只做一次旧数据清理，返回成功。
- 内置二进制是静态链接 GnuTLS 的通用二进制（arm64 + x86_64，`minos 11.0`），构建与许可
  见 `src-tauri/resources/openconnect-macos/README.md`。

### Windows

- 只探测安装包内置 `resources/apps/openconnect/openconnect.exe`，不读注册表 / 常见目录 / PATH。
- 导入：直接返回「只使用安装包内置副本，不支持导入外部副本」。
- 移除：没有「登记」这个概念，返回成功即可（与现状等价：Windows 从未产生 managed 记录）。

### 其他平台

当前产品只支持 macOS / Windows（README「平台与限制」）。`platform/mod.rs` 对其它目标给出
`compile_error!` 的明确信息，而不是让 `detect.rs` 那样的隐式缺失函数在各个 cfg 组合里报错。

## 打包隔离

Rust 侧靠 `#[cfg]` 整文件选择：macOS 二进制不编译 `windows.rs`，Windows 二进制不编译 `macos.rs`。

资源侧用 Tauri 2 的平台配置文件（自动与 `tauri.conf.json` 做 JSON merge patch）：

- `tauri.macos.conf.json`：`bundle.resources."resources/apps": null` —— mac 包不再带
  Windows 的 openconnect.exe + DLL（约 9MB）；同时把 `resources/openconnect-macos` 映射到
  `apps/openconnect`，并设置 `bundle.macOS.minimumSystemVersion = "11.0"`（内置二进制及其
  arm64 切片的最低系统要求）。
- `tauri.windows.conf.json`（新增）：`bundle.resources."resources/zh-Hans.lproj": null` —— Windows
  包不再带 macOS 的本地化资源。

## CI 护栏

新增 `.github/workflows/ci.yml`，push main / PR / 手动触发：

- `test-macos`（macos-latest）与 `test-windows`（windows-latest）两个 job；
- 各自 `pnpm install --frozen-lockfile` → `pnpm build`（`generate_context!` 需要 `../dist`）→
  `cargo test --lib`（在 `src-tauri` 下）；
- 两个 job 都原生编译各自平台代码并跑本平台用例；谁把对面平台的代码改坏，对应 job 直接红。

发版仍只由 tag 触发 `release.yml`，CI 不打包、不发布。

## 验证

- 本机 macOS：`cd src-tauri && cargo test --lib`（跑全部 macOS 用例）。
- Windows 代码：本机交叉 `cargo check --target x86_64-pc-windows-msvc` 会因 ring 的 C 依赖缺少
  MSVC 头文件失败，因此用 stub 宿主工程把 `common.rs` + `windows.rs` 按 Windows 目标
  `--emit=metadata` 编译，专门验证 Windows 平台文件能编译；真实编译由 CI 的 windows job 兜底。
- 打包配置：构建一次 debug app，确认 `Contents/Resources/apps/openconnect/openconnect`
  存在、能跑 `--version`，且没有混进 Windows 的 `openconnect.exe` / `wintun.dll`；
  确认 `Info.plist` 的 `LSMinimumSystemVersion` 是 11.0。

## 不在这一轮

- 主界面代码分割（项目 / 系统 / 托盘拆成独立组件 + 异步加载）。
- `credential.rs` / `elevate.rs` / `route.rs` / `split.rs` / `fulltunnel.rs` 里既有的平台分支整理。
- Linux 支持。
