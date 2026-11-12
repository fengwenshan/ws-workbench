# VPN 连接器平台层拆分 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development 或 superpowers:executing-plans 按任务执行。本轮由 Lead 在本会话内联完成（工作区有用户未提交改动，不自动 commit）。

**Goal:** 把 `vpn/detect.rs` 拆成 `vpn/platform/{mod,common,macos,windows}.rs`，macOS / Windows 各自实现检测、导入、候选来源，共享层只留 probe / 版本解析 / 模型；Windows 与 macOS 二进制互不编译对方平台实现。

**Architecture:** `platform/mod.rs` 用整模块 `#[cfg]` 选择 `macos` 或 `windows` 并再导出统一 API；`common.rs` 放跨平台一致的探测与解析；`commands.rs` 从 `vpn::detect` 改为 `vpn::platform`。资源用 Tauri 平台配置隔离，CI 加 macOS / Windows 双 job 跑各自 `cargo test --lib`。

**Tech Stack:** Rust 2021 / Tauri 2 / GitHub Actions / pnpm + Vite。

**前置状态：** 工作区有用户正在做的 VPN 检测改动（`detect.rs` 已比 HEAD 新），必须保留其行为与注释，不要回退。

---

## 文件结构

| 文件 | 责任 |
| --- | --- |
| `src-tauri/src/vpn/platform/mod.rs` | 唯一平台分派；导出 `detect_all` / `import_openconnect` / `remove_managed_openconnect` / `pick_connector` / `fetch_version` / `ImportedOpenconnect` |
| `src-tauri/src/vpn/platform/common.rs` | `PROBE_TIMEOUT`、`parse_openconnect_version`、`leading_version`、`source_suffix`、`describe_launch_failure`、`probe_openconnect`、`fetch_version`、`pick_connector`、`ImportedOpenconnect` + 纯逻辑单测 |
| `src-tauri/src/vpn/platform/macos.rs` | macOS 检测/导入/移除/记住的路径/旧副本清理 + macOS 用例 |
| `src-tauri/src/vpn/platform/windows.rs` | Windows 只认内置副本的检测、拒绝导入、无登记移除 + Windows 用例 |
| `src-tauri/src/vpn/mod.rs` | 去掉 `detect`，加 `platform` |
| `src-tauri/src/vpn/commands.rs` | 4 处调用改 `platform::`，注释同步 |
| `src-tauri/src/process.rs` | 顶部注释里的 `vpn::detect` 改为 `vpn::platform` |
| `.github/workflows/ci.yml` | 双平台 CI（teammate 负责） |
| `src-tauri/tauri.windows.conf.json` | Windows 包去掉 macOS 资源（teammate 负责） |
| `README.md` | 目录结构 / CI / 打包隔离说明（teammate 负责） |

---

## Task 1: 建 common.rs（共享层）

**Files:** Create `src-tauri/src/vpn/platform/common.rs`

- [ ] 从 `detect.rs` 原样迁移：`PROBE_TIMEOUT`、`parse_openconnect_version`、`leading_version`、
      `source_suffix`、`describe_launch_failure`、`probe_openconnect`、`fetch_version`、`pick_connector`、
      `ImportedOpenconnect`。可见性 `pub(super)`（对外导出的由 mod.rs `pub use`）。
- [ ] 迁移对应单测：版本解析 3 条、`source_suffix_labels_are_distinct`、`picks_openconnect_for_both_preferences`、
      `returns_none_when_nothing_usable`、`unavailable_candidate_with_path_is_not_picked`。
- [ ] 加 `#[cfg(test)] mod tests`，测试里用 `super::*`。

验证：`cd src-tauri && cargo test --lib vpn::platform::common`（此时 mod.rs 尚未接入，先做 Task 2 后再跑）。

## Task 2: 建 macos.rs（平台实现）

**Files:** Create `src-tauri/src/vpn/platform/macos.rs`

- [ ] 从 `detect.rs` 迁移并保留全部注释：`stored_connector_path_file`、`read_stored_connector_path`、
      `store_connector_path`、`clear_stored_connector_path`、`cleanup_legacy_registration`、
      `probe_app`、`probe_location`、`detect_all`、`import_openconnect`、`remove_managed_openconnect`。
- [ ] 文件内不写 `#[cfg(target_os)]`（整个文件只会在 macOS 编译）。
- [ ] 迁移 macOS 用例（去掉原 `#[cfg(target_os = "macos")]` / `#[cfg(unix)]` 条件）：
      `detect_all_returns_openconnect_channel`、`missing_custom_path_is_reported_for_openconnect`、
      `macos_never_probes_bundled_copy`、`standard_homebrew_openconnect_is_detected_without_locale_env`、
      `probe_runs_openconnect_under_c_locale`、`import_rejects_directory_without_binary`、
      `import_stores_path_without_copying_binary`、`import_accepts_homebrew_version_dir`、
      `import_reports_unrunnable_binary_and_leaves_nothing`、`import_accepts_localized_version_line`、
      `import_reports_unparseable_version_instead_of_missing_deps`、
      `probe_note_separates_parse_failure_from_launch_failure`、
      `detection_prefers_standard_install_over_registration`、
      `legacy_symlink_is_converted_to_stored_path`、`legacy_copied_binary_is_removed_not_reused`。

## Task 3: 建 windows.rs（平台实现）

**Files:** Create `src-tauri/src/vpn/platform/windows.rs`

- [ ] `detect_all`：只 `apps::bundled_binary(AppId::Openconnect, ctx)`，有则 `probe_openconnect(&path, "bundled")`，
      没有则返回 `source = "missing"` 的占位（说明 `resources/apps/openconnect/openconnect.exe` 缺失）。
- [ ] `import_openconnect`：`Err("Windows 版只使用安装包内置的 openconnect，不支持导入外部副本")`。
- [ ] `remove_managed_openconnect`：Windows 没有登记概念，`Ok(())`。
- [ ] 迁移 Windows 用例：`bundled_openconnect_is_probed_not_skipped`、
      `windows_never_falls_back_to_external_openconnect`、`import_rejects_directory_without_binary`（Windows 分支文案）。

## Task 4: 接入 mod.rs 与调用方

**Files:** Create `src-tauri/src/vpn/platform/mod.rs`；Modify `vpn/mod.rs`、`vpn/commands.rs`、`process.rs`

- [ ] `mod.rs`：`mod common;` + `#[cfg(target_os = "macos")] mod macos;` /
      `#[cfg(target_os = "windows")] mod windows;` + 对应 `pub use`；非 mac/win 给
      `compile_error!("VPN 连接器平台层目前只实现 macOS 与 Windows")`。
- [ ] `vpn/mod.rs`：`pub mod platform;` 替换 `pub mod detect;`，模块说明同步。
- [ ] `commands.rs`：`use crate::vpn::{elevate, notify, platform};` 与 4 处 `detect::` → `platform::`。
- [ ] 删除 `src-tauri/src/vpn/detect.rs`。

## Task 5: 打包隔离 + CI（teammate `ci-docs`，共享任务 task-1）

- [ ] `.github/workflows/ci.yml`：双 job；`pnpm build` 后 `cargo test --lib`。
- [ ] `src-tauri/tauri.windows.conf.json` 去掉 `resources/zh-Hans.lproj`。
- [ ] README 更新目录结构 / CI / 打包配置 / 本地测试命令。

## Task 6: 验证

- [ ] `cd src-tauri && cargo test --lib`：全绿（含新 platform 用例）。
- [ ] Windows 平台文件 stub 交叉编译：在 `.tmp/platform-win-check/` 建最小宿主工程，
      `#[path]` 引入真实 `common.rs` + `windows.rs`，按 `--target x86_64-pc-windows-msvc --emit=metadata` 编译通过。
- [ ] `pnpm build`（前端未改，确认整体没被波及）。
- [ ] `git status --short` 确认改动范围符合 write scopes；检查 `tauri.macos.conf.json` / `tauri.windows.conf.json` JSON 合法。
- [ ] 读一遍最终 diff，确认 detect.rs 的行为（messages / source / note / 清理逻辑）没有悄悄变化。

## 备注

- 不在本轮 commit：工作区已有用户未提交改动（detect.rs 改进、release.mjs、macOS dmg 辅助文件等），
  由用户统一决定提交粒度。
- 主界面代码分割不在本轮（用户已确认下一轮再做）。
