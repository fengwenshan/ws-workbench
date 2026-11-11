# 软件卸载：独立卸载窗口 + 强制退出 + 关联文件清理设计

- 日期：2026-11-10
- 分支：main（沿用现有 cleaner 模块）
- 状态：已实现（macOS 已验证；Windows 已编译并通过单测，待实机验收）
- 关联：`2026-11-06-cleaner-design.md`、`2026-11-09-cleaner-runtime-guard-and-deep-scan-design.md`

## 1. 背景与目标

现状：主窗口「系统 → 软件卸载」点「卸载」弹 antd 确认框，确认后执行系统卸载（macOS 把 `.app` 移入废纸篓 / Windows 跑 `UninstallString`），**卸载后**再弹残留扫描框让用户勾选删除。运行中的应用只能优雅退出（`terminate()`），退不掉就中止。

目标（按 2026-11-10 用户确认的新交互）：

1. 点「卸载」→ **独立卸载窗口**（截图 1）：显示应用本体与所有关联文件，分为「程序文件 / 支持文件 / 缓存文件」三组，逐项勾选，组头可勾选整组、可折叠；底部「全选 / 返回 / 确认删除」。
2. 点「确认删除」→ 若应用仍在运行，弹**强制退出确认**（截图 2）：「`<应用名>` 正在运行中，是否要强制退出？强制退出可能会导致正在编辑的数据丢失」，按钮「取消 / 强制退出」。
3. 点「强制退出」→ 先正常退出，超时后 `kill -9`，**轮询确认所有相关进程全部消失**后才继续；然后执行卸载（macOS 移 `.app` 入废纸篓 / Windows 跑自带卸载程序），再删除用户勾选的关联文件（进废纸篓，需提权的合并成一次授权），窗口内展示结果。
4. **不能卸载自己**：已安装列表过滤掉当前运行的应用本体（同一 `.app` / 同一安装目录）；命令层再兜底拦截。

非目标：

- 不做 AppCleaner 式的「把任意 .app 拖进来」入口；入口仍是已安装列表。
- 不改 VPN / 剪贴板 / 打包等其它功能。
- 不删 `pkgutil` 安装收据（只读，本窗口不列）。
- Windows 端保持与 macOS 同一套交互；已在 Windows 主机编译并通过 `cargo test --lib`，但注册表 / UAC / 强杀链路仍需实机验收。

## 2. 交互流程

```text
主窗口「软件卸载」列表 ── 点「卸载」 ──▶ cleaner_open_uninstall_window(appId)
                                           │ 已存在 uninstall 窗口 → 聚焦 + 换目标
                                           ▼
                                    卸载窗口（独立 WebviewWindow，label="uninstall"）
                                    顶部：图标 / 名称 / 使用情况 / 合计大小
                                    清单：程序文件 ▾ / 支持文件 ▾ / 缓存文件 ▾
                                          逐项勾选；组头勾选整组；可折叠
                                    底部：☐ 全选          [返回] [确认删除]
                                           │
                                    点「确认删除」
                                           │ cleaner_app_running
                               ┌───────────┴────────────┐
                           未运行                    运行中
                               │                       │ 弹强制退出确认（窗口内模态）
                               │                  ┌────┴────┐
                               │                取消      强制退出
                               │                 │          │ cleaner_force_quit
                               │                 ▼          ▼
                               │              回清单   先 terminate，3s 不退 kill -9，
                               │                        轮询到相关进程归零（≤20s）
                               │                              │ 仍未退干净 → 错误条，留在窗口
                               ▼                              ▼
                        cleaner_uninstall_app(quitRunning=false)   ← 系统卸载
                               ▼
                        cleaner_delete_related(appId, selectedPaths) ← 关联文件进废纸篓
                               ▼
                        窗口展示结果：成功 N 项 / 跳过 M 项 / 失败明细；[关闭]
                        成功后后端广播 cleaner-apps-changed → 主窗口刷新列表
```

- 「返回」= 关闭卸载窗口。
- 默认勾选：`high` / `medium` 为勾选，`low` 不勾；「全选」一键全勾（含 `low`），再次点取消全勾。
- 「使用情况」由 Spotlight 元数据推导（今天 / 7 天内 / 30 天内 / 最近未使用），取不到时不显示该行。
- 窗口打开期间应用被卸载/删除：扫描结果为空时显示空态 + 「重新扫描」。

## 3. 数据契约

Rust 结构体放 `cleaner/models.rs`，字段 `snake_case`；前端 `src/types/cleaner.ts` 同名镜像。`invoke` 参数用 camelCase（`appId`、`paths`、`quitRunning`）。

```rust
/// 关联文件分组：program / support / cache
pub struct RelatedFileGroup {
    pub id: String,     // "program" | "support" | "cache"
    pub label: String,  // "程序文件" | "支持文件" | "缓存文件"
    pub items: Vec<RelatedFileItem>,
}

pub struct RelatedFileItem {
    pub path: String,        // 文件/目录绝对路径；Windows 注册表项为 "win:{hive}|{key_path}"
    pub name: String,        // 展示名（应用名 / 目录名 / 注册表键名）
    pub kind: String,        // app_bundle/app_data/cache/preference/container/launch_agent/log/crash_report/install_dir/registry
    pub size_bytes: u64,     // 目录按 path_size 限深统计
    pub is_dir: bool,
    pub version: String,     // 仅 app_bundle；未知为空串
    pub confidence: String,  // high / medium / low
    pub requires_elevation: bool,
    pub exists: bool,
    pub selected: bool,      // 默认勾选：confidence != "low"
}

pub struct RelatedFiles {
    pub app_id: String,
    pub groups: Vec<RelatedFileGroup>,
    pub total_bytes: u64,    // 所有条目合计
    pub item_count: usize,
    pub last_used: String,   // "今天使用过" / "7天内使用过" / "30天内使用过" / "最近未使用" / ""
}

/// 强制退出结果
pub struct ForceQuitOutcome {
    pub ok: bool,            // true = 所有相关进程已退出
    pub processes: usize,    // 结束时仍在运行的进程数（ok 时为 0）
    pub names: Vec<String>,
    pub message: String,
}
```

`DeleteOutcome`（已有）增加 `skipped: usize`（serde 默认 0），语义：卸载程序已经删掉、或扫描后消失的路径不算失败。

### 3.1 新增命令

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `cleaner_open_uninstall_window` | `appId` | `()` | 记录目标、创建/聚焦 `uninstall` 窗口；目标是自身时返回错误 |
| `cleaner_uninstall_target` | — | `InstalledApp` | 卸载窗口读取当前目标；没有目标时报错 |
| `cleaner_related_files` | `appId` | `RelatedFiles` | 扫描三组关联文件，同时把本次扫描到的路径写入后端白名单缓存 |
| `cleaner_force_quit` | `appId` | `ForceQuitOutcome` | 强制退出全部相关进程并等它们消失 |
| `cleaner_uninstall_app` | `appId`, `quitRunning` | `UninstallOutcome` | 已有命令；新流程传 `quitRunning=false`（退出已单独完成），加自身拦截 |
| `cleaner_delete_related` | `appId`, `paths` | `DeleteOutcome` | 只允许删本次 `cleaner_related_files` 缓存的路径；已不存在算 skipped |

事件：

- `cleaner-progress`（已有）：新增 `task = "related_files" | "force_quit"`，`phase` 复用 `scanning/running/waiting/done`。
- `cleaner-apps-changed`：卸载/删除成功后广播，主窗口收到后 `loadApps(true)`。
- `cleaner-uninstall-target-changed`：窗口复用时广播，卸载窗口重新拉目标与清单。

## 4. 后端设计

### 4.1 模块划分

- `cleaner/models.rs`：新增第 3 节类型；`DeleteOutcome.skipped`。
- `cleaner/related.rs`（新）：平台无关的清单组装（分组映射、去重、默认勾选、`last_used` 文案）与平台分派；macOS / Windows 的具体候选生成放在 `installer_macos.rs` / `installer_windows.rs` 里（沿用现有平台文件，避免跨文件调用私有逻辑）。
- `cleaner/running.rs`：新增 `force_quit` 与可注入的 `force_quit_with_limits`（纯逻辑 + 单测：先杀后轮询、进程重启后再杀、超时失败）。
- `cleaner/installer.rs`：新增 `related_files` / `delete_related` 分派；把「给定扫描结果 + 勾选路径 → 按普通/提权/注册表分组删除」抽成内部函数，`delete_leftovers` 与 `delete_related` 共用（区别：前者要求路径仍在扫描结果里且必须存在；后者允许「扫描后消失」记为 skipped）。新增 `is_self_app` 自身判定。
- `cleaner/installer_macos.rs`：`current_app_bundle()`（从 `std::env::current_exe()` 向上找 `.app`）、列表过滤自身、关联文件候选（程序文件＝安装位置 + Spotlight/mdfind 按 bundle id 找到的其它 `.app` 副本，排除 `~/.Trash`；支持/缓存复用 `mac_leftover_candidates` 的 kind 映射）。
- `cleaner/installer_windows.rs`：`current_exe` 是否位于目标 `install_location` → 过滤/拦截自身；关联文件候选＝安装目录（程序文件）+ 现有残留条目（app_data/preference/registry/shortcut → 支持文件；cache/log → 缓存文件）。安装目录候选会过滤整个 `%WINDIR%` 与公共目录根（否则 MSI 的 `msiexec.exe` 会把 `System32` 当成程序文件）；只有拿不到 `InstallLocation`（或目录已不存在）时才用卸载程序所在目录兜底。
- `commands/cleaner.rs`：新增命令与窗口创建；`CleanerState` 增加 `pending_uninstall: Mutex<Option<InstalledApp>>` 与 `related_paths: Mutex<Option<(String, Vec<String>)>>`（app_id + 白名单）。

### 4.2 关联文件扫描规则（macOS）

程序文件：

1. 主 bundle（`app.id` 的 `mac:` 路径）；
2. 其它副本：`mdfind "kMDItemCFBundleIdentifier == <bundle id>"` 的结果里只保留以 `.app` 结尾且存在的路径，排除主 bundle、`~/.Trash`、`/System`；`mdfind` 不可用/无结果时兜底扫描 `/Applications`、`~/Applications`、`~/Desktop`、`~/Downloads`、`/Users/Shared` 一层。
3. 版本取副本 `Info.plist` 的 `CFBundleShortVersionString`（复用 `read_bundle_info`）。

支持文件：`app_data`（Application Support / Application Scripts / Saved State）、`preference`（Preferences / ByHost）、`container`（Containers / Group Containers）、`launch_agent`。
缓存文件：`cache`（Caches / HTTPStorages / WebKit / Cookies）、`log`（Logs / DiagnosticReports）、`crash_report`。

复用 `mac_leftover_candidates(bundle_id, name)` 后映射 kind → 组；`low` 置信条目保留但默认不勾。`pkgutil` 收据不列。

### 4.3 强制退出

```
detect 进程（复用 running::detect 的匹配规则：exe 在 bundle 内 + NSRunningApplication bundle id）
  空 → 直接返回 ok
正常退出（macOS NSRunningApplication.terminate；Windows CloseMainWindow）
轮询 3s（250ms 一次）
仍有 → 逐 pid kill -9（macOS libc::kill；Windows taskkill /PID <pid> /T /F）
轮询 ≤17s，期间每次重新 detect，对新出现的相关进程继续 kill（"所有进程"）
归零 → ok=true；超时 → ok=false + message「仍有 N 个进程未退出」+ names
```

自身拦截：`cleaner_force_quit` / `cleaner_uninstall_app` / `cleaner_delete_related` 都先检查 `is_self_app`，命中直接返回错误「不能卸载正在运行的应用」。

### 4.4 删除

- 白名单：`cleaner_related_files` 扫描时把「本次扫描存在的路径」存入 `CleanerState.related_paths`（含 app_id）；`cleaner_delete_related` 只接受该集合内的路径，其它路径记失败「不在本次扫描结果中」。
- 删除动作沿用 `trash_guard`：普通路径进废纸篓，`/Library` 等需提权路径与注册表键合并成一次系统授权；`/`、主目录、`/System`、`/usr` 等仍硬拒绝。
- 顺序：先 `cleaner_uninstall_app`（系统卸载），再 `cleaner_delete_related`。macOS 主 `.app` 已被移走 → skipped；Windows 卸载程序已删的路径同理。
- 卸载成功后前端自动再调一次 `cleaner_related_files`：`cleaner_uninstall_app` 只清应用列表缓存，`pending_uninstall` 保留到删除结束，所以二次扫描仍能取到应用信息。有残留 → 回到勾选页（提示「卸载已完成。重新扫描到 N 项残留」，主按钮变「清理残留」）确认后清理；无残留 → 直接进结果页。
- Windows 无参数卸载：`start_process_script` 在参数为空时不生成 `-ArgumentList`（PowerShell `Start-Process` 不接受空数组），避免「UninstallString 只有 exe 路径」的应用启动失败。
- 删除后广播 `cleaner-apps-changed`；主窗口 `loadApps(true)`。

### 4.5 卸载窗口

- label：`uninstall`，尺寸 820×560（min 760×480），居中，不可最大化；macOS `hiddenTitle + titleBarStyle Overlay`（红绿灯浮在内容上，截图 1 的样子）；Windows `decorations(false)` + 前端自绘 38px 标题栏（与主窗口一致：品牌、最小化/关闭），不显示系统标题栏。
- 单例：已存在则 `show + set_focus + set_title`，更新 `pending_uninstall` 并广播 `cleaner-uninstall-target-changed`；关闭后再次打开重新创建。
- 目标应用不存在（缓存过期）时命令返回「应用信息不存在，请重新扫描列表」。
- 能力：`src-tauri/capabilities/default.json` 的 `windows` 增加 `uninstall`（复用已有 `core:window:allow-close`、`core:window:allow-minimize`、`core:window:allow-start-dragging`）。

## 5. 前端设计

### 5.1 卸载窗口页面

- 入口：`uninstall.html` + `src/uninstall/main.ts` + `src/uninstall/UninstallWindow.vue`；`vite.config.ts` 的 `rollupOptions.input` 增加 `uninstall`。
- `UninstallWindow.vue` 自己管理状态机：

```text
loading → ready → (confirm-running) → quitting → uninstalling → rescanning
        → ready(postScan) → deleting → done | error
```

- 组件拆分：
  - `UninstallWindow.vue`：顶部标题区、状态机、请求编排、结果态；
  - `RelatedList.vue`：三组折叠清单 + 勾选/全选/组级勾选；
  - `ForceQuitDialog.vue`：截图 2 的模态（应用图标、标题、说明、取消 / 强制退出），不依赖 antd Modal（窗口是独立页面，样式自己画）；
  - `main.ts`：挂载 + 全局样式。
- 视觉：白底、系统字体（`-apple-system`）、蓝色勾选框、绿色「确认删除」(#07c160 附近)、灰色次按钮、圆角 8px；macOS 顶部留 36px 给红绿灯。图标复用 `src/components/cleaner/AppIcon.vue`（`webview` 下可用 `convertFileSrc`? 现有组件已处理 `data:` base64 图标，直接复用）。
- 交互细节：
  - 组头勾选框三态（全选 / 部分 / 未选），点击只改该组；点组名/箭头折叠。
  - 底部「全选」三态，反映当前全部条目。
  - 「确认删除」在无勾选时禁用；操作期间按钮转 loading 并禁用。
  - 失败：窗口内错误条 + 「重试 / 关闭」，不静默关闭。
  - 结果：`已卸载「X」：删除 N 项，跳过 M 项，失败 K 项`，失败逐条列出路径 + 原因，底部「关闭」；有失败时额外给「重试清理」，白名单保留到全部成功（或用户关闭窗口）。
  - 卸载成功后自动二次扫描：有残留时不直接进结果页，而是回到清单并提示「卸载已完成。重新扫描到 N 项残留，确认后会一并清理。」，按钮变「清理残留」；卸载前已勾选的路径保持勾选，新出现的条目按后端默认置信度勾选；没有残留直接进结果页。
- 关闭：`getCurrentWindow().close()`；主窗口不跟随关闭。

### 5.2 主窗口改造

- `InstalledAppsPanel.vue`：
  - 行内「卸载」→ `openUninstallWindow(app.id)`，直接打开独立窗口；
  - 删除旧的卸载确认弹窗、运行中二次确认弹窗、卸载后残留扫描弹窗及其状态/样式（约 600 行），保留列表、搜索、排序、进度、错误条；
  - `onMounted` 里监听 `cleaner-apps-changed`（`onUnmounted` 取消监听），收到后 `loadApps(true)`；
  - 卸载窗口正在操作时重复点「卸载」直接聚焦已有窗口（后端保证单例）。
- `src/api/cleaner.ts` 增加四个命令封装；`src/types/cleaner.ts` 增加类型、`DeleteOutcome.skipped`、`CleanerTask` 扩 `related_files | force_quit`。
- 旧的 `scanLeftovers` / `deleteLeftovers` 前端封装保留（后端仍保留命令），但主面板不再使用。

## 6. 安全与边界

| 场景 | 行为 |
| --- | --- |
| 目标是当前运行的应用本体 | 列表过滤掉；命令层再拦截，报「不能卸载正在运行的应用」 |
| 目标列表缓存过期 / 已删除 | `cleaner_open_uninstall_window` 报错，主窗口提示重新扫描 |
| 卸载程序弹自己的窗口 | 窗口进入「正在卸载…」，等命令返回（最长 15 分钟超时沿用现有实现） |
| 强杀后应用自行重启 | 轮询期间重新 detect 并继续 kill，直到归零或超时 |
| 关联文件扫描失败 | 窗口错误态 + 「重试」；不进入确认流程 |
| 删除部分失败 | 结果态列出失败路径与原因，成功项仍生效 |
| 权限不足的路径 | 沿用一次提权批处理；用户取消授权 → 失败明细展示 |
| `mdfind` 不可用 | 兜底目录扫描，不阻断流程 |
| 路径白名单 | 只删 `cleaner_related_files` 缓存里出现过的路径；`trash_guard` 的硬拒绝规则不变 |

## 7. 测试与验收

1. `cargo test --manifest-path src-tauri/Cargo.toml` 全绿；新增单测：
   - `related.rs`：kind → 分组映射、去重、默认勾选、`last_used` 文案、组排序；
   - `running.rs`：`force_quit_with_limits`（先正常杀、超时强杀、重启后再杀、超时失败）、自身判定；
   - `installer_macos.rs`：`current_app_bundle` 解析（含非 bundle 二进制返回 None）、副本扫描兜底、自身过滤；
   - `installer.rs`：`delete_related` 白名单校验（不在扫描结果里记失败、消失路径记 skipped）。
2. macOS 实机冒烟（`CLEANER_SMOKE=1 ... --ignored`）：在 `~/Applications` 造测试 app + `~/Library` 三份残留，跑通「关联扫描分组 → 起一个 bundle 内进程 → 强制退出 → 卸载 → 删除关联文件 → 废纸篓」，结束清理。
3. 前端 `pnpm build`（`vue-tsc --noEmit` + vite）通过。
4. 浏览器预览：`.tmp/preview-uninstall.html` 用 mock `__TAURI_INTERNALS__` 渲染 `UninstallWindow`，Chrome CDP 截图 820×560，对比截图 1；mock 运行中状态 → 点「确认删除」截图强制退出模态，对比截图 2。
5. README 的「软件卸载」段落更新为新流程说明。

## 8. 已知限制

- macOS 的窗口截图受 TCC 限制（本机 `screencapture` 被拒），GUI 视觉验收用浏览器预览 + 真机启动冒烟替代。
- Windows 的注册表 / UAC / 强杀链路只做交叉编译检查，未实机验收。
- Spotlight 未索引的磁盘/目录可能漏掉 `.app` 副本；兜底扫描只覆盖常见目录。
- 关联文件是尽力而为：随应用变化的运行期缓存（如 WebKit 在下次启动重建）删掉后应用会重新生成。
