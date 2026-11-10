# 软件卸载：独立卸载窗口 + 强制退出 + 关联文件清理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把「点卸载 → 弹确认框」改造成「点卸载 → 独立卸载窗口展示三组关联文件 → 勾选/全选 → 确认删除 → 运行中则强制退出并等所有进程消失 → 系统卸载 → 删除关联文件」；并保证不能卸载当前运行的应用本体。

**Architecture:** 新增 `uninstall.html` 页面与 `uninstall` WebviewWindow；后端新增 `cleaner/related.rs`（清单组装）+ 关联文件扫描（macOS/Windows 平台实现）+ `running::force_quit` + 白名单删除；命令层新增 5 个命令；主窗口列表改为打开卸载窗口，监听 `cleaner-apps-changed` 刷新。

**Tech Stack:** Rust（Tauri 2、sysinfo 0.40、objc2-app-kit、trash_guard、walkdir）、Vue 3 + TypeScript、手写窗口内 UI（不引入新的 UI 库）、Chrome CDP 预览截图。

**Spec:** `docs/superpowers/specs/2026-11-10-uninstall-window-design.md`

**当前状态（2026-11-10）：** 已实现并合入 main（提交 `ca82611`）。下方任务 checkbox 保留为实施时的历史记录，未逐项回勾；Windows 侧已编译并通过 `cargo test --lib`，注册表 / UAC / 强杀链路待实机验收。

**并行分工（两个 teammate 写不同的文件，禁止越界）：**

- Rust 后端：Task 1–7、Task 12 的后端部分（`src-tauri/**`）
- 前端：Task 8–11、Task 12 的预览部分（`src/**`、`uninstall.html`、`vite.config.ts`）
- Lead：Task 13（集成、全量验证、README、最终 diff）

---

## Task 1: 数据契约（models.rs）

**Files:**
- Modify: `src-tauri/src/cleaner/models.rs`
- Test: 同文件 `#[cfg(test)]`

- [ ] **Step 1:** 追加 `RelatedFileItem`、`RelatedFileGroup`、`RelatedFiles`、`ForceQuitOutcome`（字段见 spec 第 3 节，全部 `Serialize + Deserialize + Clone + Debug + Default`；`RelatedFileItem` 的 `selected` 默认 false，由扫描逻辑赋值）。
- [ ] **Step 2:** `DeleteOutcome` 增加 `#[serde(default)] pub skipped: usize`，并在两处构造点补 `..Default::default()` 或显式赋值（`installer.rs`）。
- [ ] **Step 3:** 写单测：`related_files_serializes_snake_case`（serde_json 序列化包含 `size_bytes`、`last_used`，不出现 camelCase）。
- [ ] **Step 4:** `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::models` 通过。

## Task 2: 自身识别与列表过滤

**Files:**
- Modify: `src-tauri/src/cleaner/installer_macos.rs`、`src-tauri/src/cleaner/installer_windows.rs`、`src-tauri/src/cleaner/installer.rs`
- Test: `installer_macos.rs` 单测

- [ ] **Step 1:** macOS 增加 `pub(crate) fn current_app_bundle() -> Option<PathBuf>`：`std::env::current_exe()` 向上找第一个后缀为 `.app` 的祖先（`to_string_lossy().ends_with(".app")`），canonicalize；裸二进制（dev 构建）返回 `None`。
- [ ] **Step 2:** macOS 增加 `pub(crate) fn is_self_bundle(bundle: &Path) -> bool`（`current_app_bundle()` 存在且 canonical 相等）；`list_apps` 的 `parallel_map` 结果里 `retain(|app| !is_self_bundle(bundle_of(app)))`。
- [ ] **Step 3:** Windows 增加 `is_self_install(current_exe: &Path, install_location: &str) -> bool`：`install_location` 归一化后 `win_path_within(current_exe, root)`；`list_apps` 过滤命中项。
- [ ] **Step 4:** `installer.rs` 增加 `pub fn is_self_app(app: &InstalledApp) -> bool` 平台分派（macOS 用 `is_self_bundle`，Windows 用 `is_self_install`，其它平台 false）。
- [ ] **Step 5:** 单测：`current_app_bundle` 在测试二进制（不在 .app 内）返回 None；`is_self_install` 大小写/路径边界；`list_apps` 过滤自身（可只测纯函数，不依赖真实列表）。
- [ ] **Step 6:** `cargo test` 相关测试通过。

## Task 3: 关联文件扫描（macOS）

**Files:**
- Create: `src-tauri/src/cleaner/related.rs`
- Modify: `src-tauri/src/cleaner/mod.rs`、`src-tauri/src/cleaner/installer_macos.rs`
- Test: `related.rs`、`installer_macos.rs`

- [ ] **Step 1:** `installer_macos.rs` 增加：
  - `pub(crate) fn app_bundle_copies(bundle: &Path, bundle_id: &str) -> Vec<PathBuf>`：`mdfind "kMDItemCFBundleIdentifier == <id>"`，逐行过滤 `.app` 结尾、存在、非主 bundle、非 `~/.Trash`、非 `/System`；`mdfind` 失败或空结果时兜底扫 `/Applications`、`~/Applications`、`~/Desktop`、`~/Downloads`、`/Users/Shared` 的一层 `.app`，读 `Info.plist` 比对 bundle id；去重排序。
  - `pub(crate) fn last_used_text(bundle: &Path) -> String`：`mdls -raw -name kMDItemLastUsedDate -- <path>` 解析 `%Y-%m-%d %H:%M:%S %z`，按天数返回「今天使用过 / 7天内使用过 / 30天内使用过 / 最近未使用」；失败或 `(null)` 返回空串。
  - 允许 `related.rs` 复用的 `mac_leftover_candidates`（已 pub）与 `path_size`（`mod.rs` 已 `pub(crate)`）。
- [ ] **Step 2:** `related.rs` 平台无关部分：
  - `pub const GROUP_PROGRAM/SUPPORT/CACHE` 与 `label_for_group`；
  - `fn group_of_kind(kind: &str) -> &'static str`：app_bundle/install_dir → program；app_data/preference/container/launch_agent/registry → support；cache/log/crash_report → cache；
  - `pub(crate) fn build_related_files(app: &InstalledApp, items: Vec<RelatedFileItem>, last_used: String) -> RelatedFiles`：按组稳定排序、空组剔除、`total_bytes` 求和、`item_count`、`selected = confidence != "low"`；
  - `pub fn related_files(app_id, hint, emit) -> Result<RelatedFiles, String>` 平台分派（macOS 调 `related_files_macos`）。
- [ ] **Step 3:** `installer_macos.rs::related_files_macos`：主 bundle + `app_bundle_copies` 生成 program 条目（kind `app_bundle`、版本、大小、confidence high）；复用 `mac_leftover_candidates(bundle_id, name)` 过滤 `exists` 后映射到 support/cache；不包含 `receipt_candidates`；`last_used_text`；进度事件 `task="related_files"`。
- [ ] **Step 4:** 单测（用 tempdir 造假 bundle 与 `~/Library` 结构，参考 `macos_smoke.rs` 的做法；`mdfind`/`mdls` 用注入的目录扫描版本 `app_bundle_copies_in` / `last_used_from` 便于测试，外部命令路径保持薄封装）：
  - `build_related_files_groups_and_defaults`
  - `app_bundle_copies_in_finds_copies_and_skips_trash`
  - `last_used_text_formats_days`
- [ ] **Step 5:** `cargo test --lib cleaner::related` 通过。

## Task 4: 关联文件扫描（Windows + 分派）

**Files:**
- Modify: `src-tauri/src/cleaner/installer_windows.rs`、`related.rs`

- [ ] **Step 1:** `installer_windows.rs::related_files_windows`：program 条目 = 安装目录（kind `install_dir`，存在时 conf high，大小 `path_size`）+ 安装目录为空时用 `UninstallString`/exe 所在目录兜底；support/cache 复用 `scan_leftovers` 结果映射；不含 `receipt`；`last_used` 返回空串（Windows 不查）。
- [ ] **Step 2:** `related.rs` 的平台分派补 Windows；非 macOS/Windows 返回错误。
- [ ] **Step 3:** `cargo test` 通过；`cargo check --target x86_64-pc-windows-msvc` 若本机装了 target 则跑（没有就跳过，记在报告里）。

## Task 5: 强制退出（running.rs）

**Files:**
- Modify: `src-tauri/src/cleaner/running.rs`
- Test: 同文件单测 + `macos_smoke.rs`

- [ ] **Step 1:** 新增 `pub(crate) fn force_quit_with_limits<D, K>(mut detect: D, mut terminate: impl FnMut(), mut kill: K, ...) -> (usize, String)` 纯逻辑：先 terminate → 3s 轮询 → 仍有 → 逐轮 `kill` + 250ms 轮询 ≤17s（每轮重新 detect，对新 pid 继续 kill）→ 返回剩余进程数/文案。测试用闭包模拟（正常退出、超时强杀、重启后再杀、始终不退）。
- [ ] **Step 2:** 平台实现 `platform_kill(processes)`：macOS `libc::kill(pid, libc::SIGKILL)`；Windows `Command::new("taskkill").args(["/PID", pid, "/T", "/F"])`。
- [ ] **Step 3:** `pub fn force_quit(app: &InstalledApp, emit: &dyn Fn(CleanerProgress)) -> Result<ForceQuitOutcome, String>`：`detect` 空直接 ok；发进度 `task="force_quit"`、`phase="waiting"`（含进程数）；走 `force_quit_with_limits`；返回 `ForceQuitOutcome`。自身命中时返回 Err（由命令层先判）。
- [ ] **Step 4:** 单测：`force_quit_succeeds_after_kill`、`force_quit_times_out`；实机冒烟加「起一个 bundle 内进程 → force_quit → 进程消失」。

## Task 6: 白名单删除与 skipped

**Files:**
- Modify: `src-tauri/src/cleaner/installer.rs`
- Test: `installer.rs` 单测

- [ ] **Step 1:** 把 `delete_leftovers` 里的「扫描结果 + 勾选路径 → 普通文件 / 提权文件 / 普通注册表 / 提权注册表 → 执行」抽成 `fn delete_selected(items: &[LeftoverItem], paths: &[String], missing: MissingPolicy, emit) -> Result<DeleteOutcome, String>`；`MissingPolicy::{Fail, Skip}`。
- [ ] **Step 2:** 新增 `pub fn related_files(app_id, hint, emit) -> Result<RelatedFiles, String>` 分派（若 Task 3 已放在 related.rs，这里只保留同名薄封装，避免两个入口）。
- [ ] **Step 3:** 新增 `pub fn delete_related(app_id, hint, allowed: &[String], paths: &[String], emit) -> Result<DeleteOutcome, String>`：`allowed` 为白名单（扫描缓存的路径），调用方（命令层）从 `CleanerState` 传入；不在白名单 → failed「不在本次扫描结果中」；存在性检查：不存在 → `skipped += 1`（`MissingPolicy::Skip`）；先重扫 related 得到 `RelatedFileItem` 元数据（`requires_elevation`/`kind`），再走 `delete_selected`。
- [ ] **Step 4:** 单测：`delete_related_skips_missing_paths`、`delete_related_rejects_paths_not_in_scan`。
- [ ] **Step 5:** `cargo test --lib cleaner::installer` 通过。

## Task 7: 命令层 + 窗口 + 能力

**Files:**
- Modify: `src-tauri/src/commands/cleaner.rs`、`src-tauri/src/lib.rs`、`src-tauri/capabilities/default.json`
- Test: 现有 commands 无单测，靠编译 + 集成冒烟

- [ ] **Step 1:** `CleanerState`（`cleaner/mod.rs`）增加 `pending_uninstall: Mutex<Option<InstalledApp>>`、`related_paths: Mutex<Option<(String, Vec<String>)>>`。
- [ ] **Step 2:** 命令实现（都在 `spawn_blocking` 里跑扫描/删除，避免卡 UI）：
  - `cleaner_open_uninstall_window(app_id)`：缓存取 app（没有 → Err「应用信息不存在，请重新扫描列表」）；`installer::is_self_app` → Err「不能卸载正在运行的应用」；写 `pending_uninstall`；已有 `uninstall` 窗口 → `show/set_focus/set_title` + emit `cleaner-uninstall-target-changed`，否则 `WebviewWindowBuilder` 创建（820×560、min 760×480、center、`WebviewUrl::App("uninstall.html".into())`；macOS `hidden_title(true)` + `title_bar_style(Overlay)`）。
  - `cleaner_uninstall_target()`：返回 `pending_uninstall` 里的 app（没有 → Err「没有待卸载的应用」）。
  - `cleaner_related_files(app_id)`：自身拦截；`installer::related_files`；成功时把存在的路径写入 `related_paths = (app_id, paths)`；返回清单。
  - `cleaner_force_quit(app_id)`：自身拦截；`running::force_quit`。
  - `cleaner_delete_related(app_id, paths)`：自身拦截；取 `related_paths`（app_id 不匹配或为空 → Err「清单已过期，请重新扫描」）；`installer::delete_related`；成功后清列表缓存并 emit `cleaner-apps-changed`。
  - `cleaner_uninstall_app`：加自身拦截；成功后清缓存并 emit `cleaner-apps-changed`、清 `pending_uninstall`（窗口随后关闭）。
- [ ] **Step 3:** `lib.rs` 的 `invoke_handler` 注册 5 个新命令；`capabilities/default.json` 的 `windows` 加 `"uninstall"`。
- [ ] **Step 4:** `cargo check --manifest-path src-tauri/Cargo.toml` 通过。

## Task 8: 前端契约与 API

**Files:**
- Modify: `src/types/cleaner.ts`、`src/api/cleaner.ts`

- [ ] **Step 1:** 增加 `RelatedFileItem`、`RelatedFileGroup`、`RelatedFiles`、`ForceQuitOutcome` 类型；`DeleteOutcome` 加 `skipped: number`；`CleanerTask` 扩 `'related_files' | 'force_quit'`。
- [ ] **Step 2:** `api/cleaner.ts` 增加 `openUninstallWindow(appId)`、`uninstallTarget()`、`relatedFiles(appId)`、`forceQuitApp(appId)`、`deleteRelated(appId, paths)`；导出事件名常量 `CLEANER_APPS_CHANGED_EVENT = 'cleaner-apps-changed'`、`CLEANER_TARGET_CHANGED_EVENT = 'cleaner-uninstall-target-changed'`。
- [ ] **Step 3:** `pnpm exec vue-tsc --noEmit`（或 `pnpm build`）无新增类型错误（此时其它文件可能尚未接完，允许旧错误不存在）。

## Task 9: 卸载窗口 UI

**Files:**
- Create: `uninstall.html`、`src/uninstall/main.ts`、`src/uninstall/UninstallWindow.vue`、`src/uninstall/RelatedList.vue`、`src/uninstall/ForceQuitDialog.vue`、`src/uninstall/styles.css`

- [ ] **Step 1:** `main.ts`：`createApp(UninstallWindow).mount('#uninstall-app')`，引入 `./styles.css`。
- [ ] **Step 2:** `UninstallWindow.vue`：
  - 状态机 `loading | ready | quitting | uninstalling | deleting | done | error`；
  - 挂载：`uninstallTarget()` → `relatedFiles(appId)`；监听 `cleaner-uninstall-target-changed` 重新加载；监听 `cleaner-progress` 更新状态文案；
  - 顶部：`AppIcon`（复用 `src/components/cleaner/AppIcon.vue`）+ 应用名 + `last_used` + 右侧合计大小（`formatBytes` 复用 `src/components/cleaner/format.ts`）；
  - 底部：全选（三态）、`返回`、`确认删除`（绿色，无勾选禁用）；
  - 「确认删除」→ `appRunning`：运行中 → `ForceQuitDialog`；未运行 → 直接执行 `runUninstall()`；
  - `runUninstall()`：`uninstallApp(appId, false)` → 失败留在 error 态；成功 → `deleteRelated(appId, selectedPaths)` → 结果态（成功/跳过/失败明细），底部「关闭」；
  - 「强制退出」→ `forceQuitApp(appId)`：`ok=false` → 窗口内错误条 + 「重试 / 取消」；`ok=true` → `runUninstall()`。
- [ ] **Step 3:** `RelatedList.vue`：三组可折叠（组头 checkbox 三态、箭头旋转）、条目 checkbox + 图标 + 名称 + 版本徽标（仅 app_bundle + 非空）+ 大小；`defineModel`/props+emit 管理勾选；空组不渲染；全空时显示空态。
- [ ] **Step 4:** `ForceQuitDialog.vue`：按截图 2 画（白色圆角卡片、应用图标、`<b>应用名</b> 正在运行中，是否要强制退出？`、说明文字、两个灰色按钮「取消 / 强制退出」）；`取消` emit cancel，`强制退出` emit confirm；遮罩半透明、Esc = 取消。
- [ ] **Step 5:** `styles.css`：白底、`-apple-system` 字体、蓝色 checkbox（`accent-color: #007aff`）、绿色主按钮、灰色次按钮、滚动区细滚动条；macOS 顶部 36px 红绿灯安全区（`body { padding-top: 36px }` 仅 macOS 需要——用 `platform.is_macos` 加 class）。
- [ ] **Step 6:** 关闭窗口用 `getCurrentWindow().close()`；`返回` 同。

## Task 10: 主窗口改造

**Files:**
- Modify: `src/components/cleaner/InstalledAppsPanel.vue`

- [ ] **Step 1:** 删除卸载确认 / 运行中确认 / 残留扫描三个 Modal 的模板与脚本（约 600 行）、相关 imports 与样式；保留列表、搜索、排序、扫描进度、错误条。
- [ ] **Step 2:** 行内「卸载」→ `await openUninstallWindow(app.id)`，失败 `message.error(errorText(err))`。
- [ ] **Step 3:** `onMounted` 里 `listen(CLEANER_APPS_CHANGED_EVENT, () => { void cleaner.loadApps(true) })`，`onUnmounted` unlisten。
- [ ] **Step 4:** `useCleaner.ts` 中不再使用的卸载/残留状态可以保留（后端命令仍保留），但面板不再引用；确认无未使用变量导致的 TS 报错。

## Task 11: 构建入口与预览 harness

**Files:**
- Modify: `vite.config.ts`（input 加 `uninstall`）
- Create: `.tmp/preview-uninstall.html`、`.tmp/preview-uninstall.ts`（仅验证用，不进 dist——`vite` 的 input 只加 `uninstall.html`）

- [ ] **Step 1:** `vite.config.ts` 的 `rollupOptions.input` 增加 `uninstall: path.resolve(__dirname, 'uninstall.html')`；`uninstall.html` 参照 `quick.html` 写法，body 放 `#uninstall-app`。
- [ ] **Step 2:** 预览页 mock `window.__TAURI_INTERNALS__`：`cleaner_uninstall_target` 返回假应用（名称 `ws-workbench`、版本 1.0.18、图标 null）、`cleaner_related_files` 返回三组假数据（程序文件 2 项 39.2 MB/38.9 MB、支持文件 2 项 4.1 KB/16.4 MB、缓存文件 2 项）、`plugin:event|listen` 返回 1；`cleaner_app_running` 先返回 `{ running: true, processes: 3, names: [...] }`；`cleaner_force_quit` 返回 ok；`cleaner_uninstall_app`/`cleaner_delete_related` 返回成功。
- [ ] **Step 3:** `pnpm dev`（8790）起前端，Chrome headless + CDP 截图 820×560：清单页（对比截图 1）、点「确认删除」后的强制退出模态（对比截图 2）、结果态。截图存 `.tmp/ui/uninstall-*.png`。

## Task 12: 冒烟测试

**Files:**
- Modify: `src-tauri/src/cleaner/macos_smoke.rs`

- [ ] **Step 1:** 新增 `#[ignore]` 测试 `macos_related_scan_force_quit_smoke`（`CLEANER_SMOKE=1` 才跑，所有写入限制在测试唯一名 + `~/Applications`，结束清理含废纸篓）：
  1. 造测试 `.app`（参考现有 smoke）+ `~/Library` 三份残留；
  2. `installer::related_files` → 断言三组、程序文件含测试 app、支持/缓存命中三份残留、默认勾选规则；
  3. 复制 `/bin/sleep` 为 bundle 内可执行文件并 spawn（`sleep 300`）→ `running::detect` 命中 → `force_quit` → 断言进程消失（`try_wait`）；
  4. `cleaner_uninstall_app` 等价路径（`installer_macos::uninstall`）→ `delete_related`（白名单=清单路径）→ 断言文件消失/跳过、废纸篓确认；
  5. 全部清理。
- [ ] **Step 2:** 本机执行：`CLEANER_SMOKE=1 CARGO_HOME="$PWD/.tmp/cargo-home" cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored cleaner::macos_smoke` 通过（若 TCC 限制废纸篓读取则按现有逻辑跳过断言）。

## Task 13: 集成验收（Lead）

- [ ] **Step 1:** 全量 `cargo test --manifest-path src-tauri/Cargo.toml`、`pnpm build` 通过。
- [ ] **Step 2:** 预览截图对比截图 1/2，调整样式（颜色、间距、字号、绿按钮、组头三态）。
- [ ] **Step 3:** 真机启动 dev 版冒烟（`pnpm exec tauri dev` 或已有二进制），确认 `uninstall` 窗口能打开、无 panic；TCC 不允许截图时记录限制。
- [ ] **Step 4:** README「软件卸载」段落改写成新流程（含「不能卸载自己」）。
- [ ] **Step 5:** `git diff` 全量 review，确认没有误改其它功能，最终报告。
