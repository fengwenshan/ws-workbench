# 「系统」工具页扩展设计（屏幕休眠 / 开机自启 / 弹窗拦截 / 默认应用）

> **2026-11-07 后续调整**：用户决定去掉「开机自启」「默认应用」两个页签，前后端代码已删除；
> 实际页签只剩 `软件卸载 | 弹窗拦截(仅 Windows) | 屏幕休眠`。本文 4.2、4.4 节与相关测试清单
> 仅作历史设计保留，不再对应代码。屏幕休眠页签也改成了会话式菜单（不限时 / 分钟 / 小时 /
> 自定义小时+分钟），与 Amphetamine「开启新会话」一致，4.1 节的固定四档时长已过时。

- 日期：2026-11-07
- 分支：feat/uninstaller
- 状态：设计已确认（方案 B：`cleaner` 模块内按 tab 拆子模块），实现中
- 关联：[2026-11-06-cleaner-design.md](./2026-11-06-cleaner-design.md)（软件卸载；内存占用部分已于 2026-11-07 移除）

## 1. 背景与目标

原「系统清理」工具页只有软件卸载 / 内存占用两个页签。这一版把它扩成「系统」工具页；**2026-11-07 按需求移除内存占用**（查看内存、结束进程、深度释放一并删除，卸载时也不再自动结束运行中的应用），现为软件卸载 + 四个新页签：

| 页签 | 能力 | 备注 |
| --- | --- | --- |
| 软件卸载 | 不变 | 已有 |
| 弹窗拦截 | 实时列出可疑广告弹窗（检测 + 可选拦截） | **仅 Windows**，紧跟软件卸载 |
| 屏幕休眠 | 阻止系统睡眠 / 屏幕关闭，可定时 | 参考 Amphetamine 的公开 API 做法 |
| 开机自启 | 列出开机启动项，启用 / 禁用，含本应用登录启动开关 | Windows 注册表/启动文件夹，macOS LaunchAgents + 登录项 |
| 默认应用 | 查看/修改常见文件类型的默认打开应用 | macOS 可改；Windows 只读 + 「选择默认应用」对话框 + 系统设置入口 |

非目标（本期不做）：
- 合盖不休眠（macOS `pmset disablesleep` 需 root，有残留风险）、away mode（仅 S3）
- 弹窗「自动拦截」、DLL 注入、驱动、结束进程；只做检测 + 用户逐条手动关闭/隐藏
- Windows 静默改写 `UserChoice`（Win8+ 有哈希保护，第三方写入会被系统重置，属于破坏行为）
- 写系统级启动项（HKLM / 公共启动文件夹）——只读展示，需管理员的操作不自动触发

## 2. 信息架构

- 页面标题「系统清理」→「系统」；侧栏按钮 title/aria「清理」→「系统」；托盘菜单项「清理」→「系统」；快捷键 `⌘/Ctrl+4` 不变；内部 view key 仍为 `cleaner`（命令前缀也保持 `cleaner_*`，避免大范围改名）。
- 页签顺序：`软件卸载 | 弹窗拦截(仅 Windows) | 屏幕休眠 | 开机自启 | 默认应用`
- 每个页签独立组件，容器 `CleanerPanel.vue` 只负责页签状态与懒加载（进入页签才拉数据 / 开监听）。

## 3. 模块划分（方案 B：一个 cleaner 模块，内部按 tab 拆）

后端：

```text
src-tauri/src/cleaner/
  sleep.rs              屏幕休眠（内部 cfg 分支 / 平台函数）
  startup.rs (+ startup_macos.rs / startup_windows.rs)
  popup.rs              Windows-only 检测线程；非 Windows 返回「不支持」
  defaultapps.rs (+ defaultapps_macos.rs / defaultapps_windows.rs)
  models.rs             现有数据契约；新增类型就近放各自模块文件，避免共享文件冲突
```

命令层（每个 tab 一个文件，只做参数/错误翻译，不含业务）：

```text
src-tauri/src/commands/cleaner_sleep.rs
src-tauri/src/commands/cleaner_startup.rs
src-tauri/src/commands/cleaner_popup.rs
src-tauri/src/commands/cleaner_defaultapps.rs
```

前端（每个 tab 一套：类型 / API / composable / 组件）：

```text
src/types/{sleep,startup,popup,defaultapps}.ts
src/api/{sleep,startup,popup,defaultapps}.ts
src/composables/use{KeepAwake,Startup,Popup,DefaultApps}.ts
src/components/cleaner/{SleepPanel,StartupPanel,PopupPanel,DefaultAppsPanel}.vue
```

共享文件（`cleaner/mod.rs`、`commands/mod.rs`、`lib.rs` 的 handler 注册、`CleanerPanel.vue`、`App.vue`）由 Lead 统一改，实现者不改，避免冲突。

## 4. 功能设计

### 4.1 屏幕休眠（防休眠）

交互：开关「保持唤醒」+ 时长（15/30/60/120 分钟 / 无限）+「允许屏幕关闭」勾选（默认关，即屏幕保持常亮）+ 状态卡（已保持时长、剩余时间、当前是否阻止屏幕关闭）+ 停止。

命令：`keepawake_status` / `keepawake_start(duration_secs: Option<u64>, allow_display_sleep: bool)` / `keepawake_stop`。

- macOS：IOKit `IOPMAssertionCreateWithName`，系统断言 `PreventUserIdleSystemSleep` 常建；`allow_display_sleep=false` 时加 `PreventUserIdleDisplaySleep`。断言名固定 `ws-workbench Keep Awake`，进程退出/崩溃由系统自动回收。不需要 root、不需要 TCC。
- Windows：首版用 `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED [| ES_DISPLAY_REQUIRED])`，由专用线程每 25 秒重新置位，停止时发信号让线程复位 `ES_CONTINUOUS` 后退出。API 面比 `PowerCreateRequest` 小，不需要管理员；代价是 `powercfg /requests` 里看不到命名请求（后续要可见性再换 `PowerCreateRequest`）。
- 定时：后端持有 `expires_at: Option<Instant>`，`std::thread` 定时器到期自动释放，`keepawake_status` 里还有一道惰性回收（定时器被系统挂起也不会残留断言）；`keepawake_status` 返回剩余秒数。
- 失败/边界：断言或电源请求创建失败返回中文错误；Windows 不拦屏保/合盖/主动睡眠，Modern Standby 下可能被挂起，UI 写明；macOS 合盖不在支持范围。
- 状态只在本进程内有效，重启应用即释放（不做登录级保活）。

**自动保持（Triggers，对齐 Amphetamine）**：页签第三张卡「自动保持唤醒」，总开关默认关；开启后可按规则自动保持：

- 规则类型（多条规则之间是「或」，任一满足即保持）：
  - `app_running`：指定进程在运行时（进程名忽略大小写、包含即匹配，可填 `Typora` / `Code`）；
  - `ac_power`：接通电源时（笔记本）；
  - `external_display`：连接外接显示器时。
- 命令：`sleep_triggers_state` / `sleep_triggers_set_armed(armed)` / `sleep_triggers_add(kind, value)` / `sleep_triggers_remove(id)` / `sleep_triggers_set_enabled(id, enabled)`。
- 实现：`cleaner/sleep/triggers.rs` 后台线程 5 秒一轮探测；进程列表用 `sysinfo`（跨平台），电源 macOS 用 `pmset -g batt`（`AC Power`）/ Windows 用 `GetSystemPowerStatus().ACLineStatus`，外接显示器 macOS 用 CoreGraphics `CGGetActiveDisplayList` 数量 > 1 / Windows 用 `GetSystemMetrics(SM_CMONITORS) > 1`。全部只读、不需要管理员权限。
- 优先级：手动开启优先——有手动会话时规则不接管；规则失效只释放「规则自己开」的会话（`Origin::Manual` / `Origin::Trigger(id)` 区分），不会误停手动会话。命中/失效时发事件 `sleep-triggers-changed`，状态卡显示「自动规则：…」。
- 持久化：`<data_dir>/sleep-triggers.json`（`armed` + 规则列表，最多 20 条），不动 AppConfig。
- 边界：规则只在应用运行期间生效（不做登录级保活、不装守护进程）；外接显示器按「活动屏幕数 > 1」判断，Sidecar/虚拟显示器也会算一块屏。

### 4.2 开机自启（启动项管理）

数据：`StartupItem { id, name, command, source, scope, enabled, can_toggle, detail }`
- `source`: `run` / `run_once` / `startup_folder` / `launch_agent` / `login_item`
- `scope`: `user` / `machine`

命令：`startup_list` / `startup_set_enabled(id, enabled)` / `startup_app_autostart_get` / `startup_app_autostart_set(enabled)`。

Windows：
- 枚举 `HKCU\...\Run`、`HKLM\...\Run`（含 `WOW6432Node` 32 位视图）、`RunOnce`（只读）、用户/公共启动文件夹。
- 启用状态读 `Explorer\StartupApproved\{Run,Run32,StartupFolder}` 的 12 字节 REG_BINARY：`02` = 启用；`03` + FILETIME = 禁用；缺省视为启用。写状态前把当前值备份到 `<data_dir>/startup-backup.json`（带时间戳，可回滚）。
- 禁用只写状态位、不删 Run 值；HKLM / 公共启动文件夹需要管理员 → `can_toggle=false` + 提示；`RunOnce` 只读。
- 本应用登录启动：写/删 `HKCU\...\Run\ws-workbench`，默认关。

macOS：
- 枚举 `~/Library/LaunchAgents/*.plist`（读 `Label`、`Program`/`ProgramArguments`、`RunAtLoad`），`/Library/LaunchAgents` 只读（需要 root 才能改）。
- 启用/禁用：`launchctl disable/enable gui/$UID/<label>` + `launchctl bootstrap/bootout gui/$UID <plist>`，只动用户域。
- 登录项：`osascript` 走 System Events 读取名称并启停；首次会触发「控制 System Events」自动化授权，被拒绝时降级为只列 LaunchAgents 并给提示（不阻塞页面）。
- 本应用登录启动：`SMAppService.mainApp`（macOS 13+）或 `~/Library/LaunchAgents/ws-workbench.plist`，默认关。

安全：只切状态不删除条目；系统范围只读；桌面文件系统只读不写。

### 4.3 弹窗拦截（仅 Windows，检测 + 可选拦截）

交互：页签顶部「检测」开关（默认关）+「拦截模式」开关（默认关，检测开着时才可开）+ 拦截参数（自动动作 `close`/`hide`、最低可疑分 6/8/10/12）+ 最近弹窗列表（时间 / 进程 / 标题 / 类名 / 尺寸 / 命中特征 / 操作）+「清空」+ 忽略名单（按进程名）。

命令：`popup_status` / `popup_start` / `popup_stop` / `popup_recent` / `popup_clear` / `popup_dismiss(id, action)` / `popup_ignore(process, ignored)` / `popup_settings_get` / `popup_settings_set(settings)`。

- 实现：后台线程每 800ms `EnumWindows` 轮询可见顶层窗口（和火绒的定时器 + 枚举思路一致）。首版不用 `SetWinEventHook`：API 面更小、不会被 UIPI 挡事件；代价同样是「先显示后记录」。同一窗口（类名 + 标题）只记一次，窗口标题变化会重新记录。
- 启发式打分（只用于「可疑」排序，不自动动作）：`WS_EX_TOOLWINDOW` / `WS_EX_NOACTIVATE` / `WS_EX_TOPMOST`、无标题、尺寸异常（< 200×120 或超过屏幕 80%）、`GetWindow(GW_OWNER)` 为空、所属进程不是前台进程、常见广告类名等；每项给分值 + 命中原因字符串。
- 检测模式（默认）：环形缓冲最多 200 条，新记录通过事件 `popup-detected` 推给前端；列表项操作由用户点击：`dismiss` 执行 `WM_CLOSE` 或 `SW_HIDE`（同进程 5 秒限流），`ignore` 把进程加入忽略名单（存 `<data_dir>/popup-ignore.json`，不动 AppConfig）。
- 拦截模式（可选，默认关）：命中后自动处理，判定是纯函数 `should_intercept`，全部条件都满足才动手：
  1. 设置里 `intercept=true`，且 `score >= min_score`；
  2. 命中原因至少两条（避免单一线索误杀）；
  3. 不在受保护进程表（`explorer.exe`/`dwm.exe`/`winlogon.exe`/`svchost.exe`/`SearchHost.exe`/本应用等 22 个）与受保护窗口类（`Progman`/`WorkerW`/`Shell_TrayWnd`/`Shell_SecondaryTrayWnd` 等外壳窗口）里；
  4. 进程不在忽略名单；
  5. 同进程 5 秒冷却；
  6. 每分钟自动处理数未达上限（默认 6，熔断防连环误杀）。
  设置存 `<data_dir>/popup-settings.json`，后端会把 `action`/`min_score`/`max_per_minute` 收进安全范围；列表里被自动处理的记录带「已自动关闭 / 已自动隐藏」标记。
- 边界（写进 UI 说明）：只能显示后处理（弹窗会闪一下）；UIPI 下关不掉管理员窗口；UAC / 锁屏 / 内嵌广告不可见；不注入、不驱动、不杀进程；拦截模式失败不会重试（下一轮轮询再判断）。
- 非 Windows：模块编译 stub，命令返回「仅 Windows 支持」；前端用 `platform.is_windows` 隐藏页签。

### 4.4 默认应用

交互：常用类型列表（扩展名 → 当前默认应用 → 「更换」），Windows 额外提供「选择默认应用…」与「打开系统默认应用设置」。

命令：`defaultapps_list` / `defaultapps_candidates(extension)` / `defaultapps_set(extension, app_id)` / `defaultapps_reveal_settings` / `defaultapps_open_with(extension)`。

- macOS：扩展名 → UTI（`UTTypeCreatePreferredIdentifierForTag(kUTTagClassFilenameExtension)`）→ 当前 handler（`LSCopyDefaultRoleHandlerForContentType`）→ 改（`LSSetDefaultRoleHandlerForContentType(kLSRolesAll, bundle_id)`）。候选应用 = `/Applications`、`~/Applications` 里能打开该 UTI 的 bundle（`LSCopyAllRoleHandlersForContentType` + `LSCopyApplicationURLsForBundleIdentifier`）。不需要 TCC/root。
- Windows：只读展示扩展名当前关联（`HKCU\...\Explorer\FileExts\<ext>\UserChoice` 的 `ProgId` → `HKCR\<ProgId>\shell\open\command`）；「选择默认应用…」用 `rundll32 shell32.dll,OpenAs_RunDLL <临时文件>` 打开系统「打开方式」对话框（用户勾选「始终使用」即完成设置）；「打开系统设置」用 `ms-settings:defaultapps`。**不静默写 UserChoice**（Win8+ 哈希保护）。
- 常见类型首批：`.txt .md .pdf .png .jpg .mp4 .mp3 .zip .html .json .csv .docx .xlsx .pptx`。
- 错误：bundle 解析失败 / 没有可用应用 / 系统接口拒绝时返回中文提示并保持原值。

## 5. 验证计划

- 单测：启动项解析与 StartupApproved 编解码（正/反例）、弹窗启发式评分（构造特征组合）、防休眠状态机与定时（不依赖平台 API 的部分）、默认应用类型表与路径归一化。
- macOS 实机：`pmset -g assertions` 断言出现/消失；临时 LaunchAgent 上 enable/disable 后清理；`LSSetDefaultRoleHandlerForContentType` 改一个测试扩展名再改回；备份 JSON 内容断言。
- Windows：`cargo xwin check` + 实机验收清单（启动项状态位、`powercfg /requests`、弹窗检测、OpenAs 对话框）。
- 前端：`pnpm build` + Tabbit mock 渲染截图（四个新页签）。

### 5.1 已执行（2026-11-07，macOS 14 真机）

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| Rust 全量单测 | `cargo test --lib` | 382 passed / 0 failed / 7 ignored（跨平台单测 + 手动探针） |
| 电源断言真实性 | `cargo test --lib cleaner::sleep::tests::manual_assertion_shows_in_pmset -- --ignored` | `pmset -g assertions` 里出现 `ws-workbench Keep Awake`，`PreventUserIdleSystemSleep` + `PreventUserIdleDisplaySleep` 各一条，停止后消失 |
| 启动项枚举 | `... cleaner::startup::tests::manual_list_real_items --ignored` | 本机 22 条（用户 LaunchAgents 可切、`/Library` 只读带原因、禁用时间反解正常） |
| 默认应用读取 | `... cleaner::defaultapps::tests::manual_list_real_defaults --ignored` | 14 种类型全为真实 handler；`.txt` 候选 18 个（含 `~/Applications`） |
| macOS 编译 | `cargo check` | 通过，仅 3 条 vpn 既有 warning |
| Windows 交叉编译 | `cargo xwin check --target x86_64-pc-windows-msvc` | 通过，0 error（含拦截模式线程、自动保持电源/显示器探测与 win32 调用） |
| 前端构建 | `pnpm build` | 通过（vue-tsc + vite） |
| 应用启动 | `pnpm tauri dev` | 正常启动，托盘快捷面板创建成功，无 panic |

未做（环境限制）：Windows 实机验收；真实改动系统默认应用（避免动用户既有文件关联，只验证读与候选）；弹窗检测只在 Windows 有效，未实机跑；四页签的浏览器 mock 截图未做（改为真机启动 + 组件代码走查）。


## 6. 已知限制

- 内存占用功能已于 2026-11-07 移除：不再展示系统内存、结束进程与深度释放；卸载流程改为只调用系统卸载器 / 移入废纸篓，不再先结束运行中的应用（应用在运行时由卸载器自行提示，macOS 运行中的实例会随 bundle 移入废纸篓、持续到用户退出应用）。

- 屏幕休眠：只防空闲睡眠/关屏；不拦屏保（macOS 屏幕断言会压制屏保）、合盖、主动睡眠。
- 开机自启：Windows `StartupApproved` 格式无官方文档，按 Win10/11 观察实现，写坏了有备份可回滚；macOS 登录项依赖自动化授权。
- 弹窗拦截：检测模式，误报（正常弹窗被标可疑）可忽略，不会自动做事；对管理员窗口无效。
- 默认应用：Windows 不能静默改（系统限制），只能引导；macOS 用的 LaunchServices API 已标记 deprecated，但仍是当前唯一可行方式。
