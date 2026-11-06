# 系统清理（软件卸载 + 内存释放）设计

- 日期：2026-11-06
- 分支：feat/uninstaller
- 状态：待评审（用户已确认方向，按此实现）
- 关联：新增工具页；不改动现有项目/打包/代理逻辑

## 1. 背景与目标

工作台目前有项目、代理、设置三个工具。日常维护机器还要另开 Geek Uninstaller（Windows）/ 腾讯柠檬（macOS）卸载软件、看内存占用。本次把这两件事收进工作台，做成第四个工具页「清理」。

参考了开源实现，但**不复用、不打包任何第三方二进制**：

- [Bulk Crap Uninstaller](https://github.com/bcuninstaller/Bulk-Crap-Uninstaller)（Apache-2.0）Windows 侧：注册表 `Uninstall` 键枚举、静默卸载参数、残留规则
- [Pearcleaner](https://github.com/alienator88/Pearcleaner)（Apache-2.0 + Commons Clause）macOS 侧：按 bundle id 搜 `~/Library` 残留、删除走废纸篓
- 腾讯柠檬的做法（仅作行为参考）：`LemonDaemon` 里用 `purge` + `killall` 做内存释放

### 目标（In Scope）

1. 左侧图标栏新增「清理」工具，`⌘/Ctrl+4` 切换；页面两个页签：软件卸载、内存占用。
2. 软件卸载：列出已安装应用（图标/名称/版本/发布者/大小/位置），支持搜索、排序、刷新；执行卸载；卸载后扫描残留并勾选删除。
3. 内存占用：整机内存总览 + 按应用聚合的占用排行；结束所选应用进程；深度释放（Windows 清工作集/待机列表，macOS 提权 `purge`），显示释放前后差值。
4. 全程：删除只进废纸篓；系统应用不可卸载；破坏性操作必须显式确认；路径白名单校验。

### 非目标（Out of Scope）

- 垃圾/大文件/重复文件/隐私清理（柠檬的其他功能）
- Windows Store / AppX 应用
- 启动项管理、实时安装监控、批量移除（Geek Pro 功能）
- 卸载器重启后延迟删除（`MoveFileEx`）；被占用文件只报错，提示关闭后重试
- 批量静默卸载、卸载队列
- 常驻提权守护进程、系统级开机自启
- Linux 支持、应用自动更新
- 修改现有项目树 / 打包 / VPN 行为

## 2. 信息架构与交互

### 2.1 入口

- `AppView` 增加 `'cleaner'`；图标栏在「代理」下加「清理」按钮（扫帚/闪光图标，与现有 24px 线性图标一致）
- 快捷键：`⌘/Ctrl+4`；`selectTool('cleaner')` 语义和 settings 一致（重复点击不折叠任何面板）
- 托盘菜单**不新增**入口（避免菜单膨胀；后续需要可加）
- 标题栏显示的当前工具名由 `App.vue` 现有映射推导

### 2.2 页面结构

```text
┌────────────────────────────────────────────────────────┐
│ 系统清理  [重新扫描]                    ← 页头          │
│ [软件卸载] [内存占用]                  ← 页签           │
├────────────────────────────────────────────────────────┤
│ 工具条：搜索 / 排序 / 数量统计                          │
│ 列表：图标 名称 版本 发布者 大小 安装日期 位置  [卸载]  │
│ …                                                      │
│ 空态 / 加载态 / 扫描进度                                │
└────────────────────────────────────────────────────────┘
```

内存页签：

```text
│ 内存占用 75%  [释放]  ← 总览卡（进度条 + 已用/总量）     │
│ 应用            进程数        内存                       │
│ ☐ Code          12          2.46 GB                     │
│ ☐ Google Chrome 31          2.36 GB                     │
│ …                                                      │
│ 底部：已选 N 项 / [结束进程] [深度释放]                  │
```

- 视觉沿用现有 CSS 变量（`--primary` 等）与 ant-design-vue 的 `Modal`、`message`，不引入新 UI 库
- 页面切换用 `v-show`（与现有工具页一致），内存轮询在页面不可见时暂停

## 3. 数据模型与命令接口

所有结构体 serde 字段用 snake_case（与 `OpenWithApp` 等现有类型一致），前端 `invoke` 参数用 camelCase。

### 3.1 类型

```rust
pub struct InstalledApp {
    pub id: String,             // "win:{hive}|{key_path}" / "mac:{bundle_path}"
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub install_location: String,
    pub size_bytes: u64,        // 0 表示未知
    pub icon: Option<String>,   // data:image/png;base64,...
    pub source: String,         // "registry" | "applications" | "system"
    pub can_uninstall: bool,    // 系统应用/SIP 应用为 false
    pub uninstall_kind: String, // "msi" | "exe" | "app_bundle" | "system" | "force"
    pub install_date: String,   // YYYY-MM-DD；未知为空
    pub detail: String,         // 备注：安装日期、注册表键、bundle id 等
}

pub struct UninstallOutcome {
    pub ok: bool,
    pub command: String,        // 实际执行的命令（展示给用户）
    pub exit_code: Option<i32>, // 拿不到时为 None
    pub elevated: bool,
    pub message: String,
}

pub struct LeftoverItem {
    pub path: String,
    pub kind: String,           // install_dir/app_data/registry/cache/preference/log/
                                // launch_agent/shortcut/container/receipt
    pub size_bytes: u64,
    pub is_dir: bool,
    pub confidence: String,     // "high" | "medium" | "low"
    pub selected: bool,         // 默认勾选：high
    pub requires_elevation: bool,
    pub exists: bool,
}

pub struct DeleteOutcome {
    pub trashed: usize,
    pub failed: Vec<DeleteFailure>,
}

pub struct DeleteFailure {
    pub path: String,
    pub error: String,
}

pub struct MemoryApp {
    pub id: String,             // bundle id / 应用目录 / exe 路径
    pub name: String,
    pub exe: String,
    pub icon: Option<String>,
    pub process_count: usize,
    pub memory: u64,            // 该应用所有进程 RSS 之和
    pub pids: Vec<u32>,
    pub kind: String,           // "app" | "process"
}

pub struct MemorySnapshot {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub used_percent: f32,
    pub swap_total: u64,
    pub swap_used: u64,
    pub apps: Vec<MemoryApp>,   // 按 memory 降序
    pub sampled_at: i64,        // epoch 毫秒
}

pub struct QuitOutcome {
    pub quit: usize,
    pub forced: usize,
    pub failed: Vec<String>,
}

pub struct ReleaseOutcome {
    pub ok: bool,
    pub before_used: u64,
    pub after_used: u64,
    pub freed: u64,             // after 用系统重采样，不夸张
    pub message: String,
    pub note: String,           // 原理说明，直接展示在 UI
}

pub struct CleanerProgress {
    pub task: String,           // "list_apps" | "scan_leftovers" | "uninstall"
    pub phase: String,          // "scanning" | "loading_icons" | "sizing" |
                                // "running" | "waiting" | "done"
    pub current: usize,
    pub total: usize,
    pub message: String,
}
```

### 3.2 命令

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `cleaner_list_apps` | `refresh: bool` | `Vec<InstalledApp>` | 有缓存直接返回；`refresh=true` 重新扫描；扫描中重复调用返回错误「正在扫描」 |
| `cleaner_uninstall_app` | `appId: String` | `UninstallOutcome` | 阻塞到卸载程序退出或超时（15 分钟） |
| `cleaner_scan_leftovers` | `appId: String` | `Vec<LeftoverItem>` | 卸载后调用；也允许对未卸载应用预览 |
| `cleaner_delete_leftovers` | `appId: String`, `paths: Vec<String>` | `DeleteOutcome` | 白名单校验后逐条进废纸篓；需提权的条目合并成一次系统授权 |
| `cleaner_memory_snapshot` | — | `MemorySnapshot` | 单次采样，前端按 2 秒轮询 |
| `cleaner_quit_apps` | `appIds: Vec<String>`, `force: bool` | `QuitOutcome` | 优雅退出优先；`force=true` 时直接强制结束 |
| `cleaner_deep_release` | — | `ReleaseOutcome` | 需要时弹一次系统授权 |

事件：`cleaner-progress`（payload = `CleanerProgress`），前端订阅后更新扫描进度条。

### 3.3 状态

- 后端 `CleanerState`：`Mutex<Option<Vec<InstalledApp>>>` 缓存列表 + `Mutex<bool>` 扫描中标记；`app.manage()` 注册
- 前端不持久化卸载/内存数据；只持久化「排序方式」到 `safeStorage`（键 `ws-workbench:cleaner:sort`）

## 4. 软件卸载

### 4.0 Geek Uninstaller 1.5.1.163 解析结论（行为参考）

对用户提供的 `geek.exe`（PE32，MFC + Codejock，6.3 MB，2022-10-17 编译，CrystalBit Solutions 签名，manifest `requireAdministrator`）做了静态解析，可提取的行为：

- **数据源**：`SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\`（含 `Wow6432Node`，HKCU/HKLM/HKEY_USERS），MSI 另外读 `Installer\UserData\%s\Products\`、`Installer\Folders`、`App Paths`
- **卸载执行**：`CreateProcessW` / `ShellExecuteExW` 跑 `UninstallString`，识别 `msiexec.exe`、`rundll32`；对 COM 组件先 `regsvr32.exe /s /u`
- **强制移除**：卸载失败或条目无 `UninstallString` 时提示「清理注册表条目 + 清理文件」；删不掉的项用 `MoveFileExW` 的延迟删除（重启后删），注册表非空键逐级删除
- **残留追踪**：卸载后扫描「文件系统 + 注册表」，UI 是两棵树的勾选列表（`File System (N items)` / `Registry (N items)`），文案为「Scanning for leftover files, folders and registry items...」「Some leftover traces found!」
- **进程占用检测**：`CreateToolhelp32Snapshot` + `QueryFullProcessImageNameW`，配合 `EnumWindows`/`PostMessage` 处理占用
- **商店应用**：`PowerShell Remove-AppxPackage` + `AppxManifest.xml`（本设计不做）
- **Pro 独有**：实时安装监控、启动项管理、批量移除（本设计不做）
- 全程强制管理员；除疑似内部开关 `/store_apps`、`/restart` 外**没有公开命令行接口**，不适合作为被调用的子组件，也不适合随包分发（闭源、免费版许可为个人使用）

本设计取用其中「注册表枚举 + UninstallString + 文件/注册表残留分组 + 强制移除」这四件事，其余按第 1 节的目标重做。

### 4.1 列表数据源

**Windows**（`installer_windows.rs`）

- `winreg` 读三处：`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`、`HKLM\SOFTWARE\WOW6432Node\...`、`HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`；用 `KEY_WOW64_32KEY`/`KEY_WOW64_64KEY` 显式取，避免 32/64 视图漏项
- 读取字段：`DisplayName`、`DisplayVersion`、`Publisher`、`InstallLocation`、`DisplayIcon`、`UninstallString`、`QuietUninstallString`、`EstimatedSize`（KB）、`InstallDate`、`SystemComponent`、`ParentKeyName`、`ReleaseType`
- 过滤：无 `DisplayName`；`SystemComponent == 1`；`ParentKeyName` 非空（补丁）；`ReleaseType` 含 `Update`/`Hotfix`；`DisplayName` 形如 `KB\d+`
- 去重：同名同版本只留一条（HKLM 优先）
- 大小：`EstimatedSize * 1024`；没有时若 `InstallLocation` 存在则递归统计（最多 3 层，防御性上限 2GB 统计量）
- 图标：批量 `PrivateExtractIcons`（复用 `openwith` 的 `windows_icons`，改为 `pub(crate)`），`DisplayIcon` 去掉 `,0` 后缀取 exe

**macOS**（`installer_macos.rs`）

- 扫 `/Applications`、`~/Applications`（含一层子目录，如 `/Applications/Utilities`；`/System/Applications` 不扫，系统应用不可卸载）
- 只收 `.app` bundle；`Info.plist` 用 `plist` 解析：`CFBundleIdentifier`、`CFBundleDisplayName`/`CFBundleName`、`CFBundleShortVersionString`、`CFBundleExecutable`
- 图标：把 `openwith` 的 `mac_icon` 改为 `pub(crate)` 复用（icns → 64px PNG）
- 大小：递归统计 bundle，防止符号链接环（`symlink_metadata` 判断）
- 系统应用判定：路径在 `/System` 下、或 bundle id 以 `com.apple.` 开头 → `can_uninstall=false`（本版本不扫 `/System`，保留判定逻辑给以后）

**扫描与进度**

- `cleaner_list_apps` 内部在 `spawn_blocking` 中执行；先枚举（快），再并发取图标/算大小（`std::thread` 最多 8 个 worker，避免引入 rayon），每完成一项发一次 `cleaner-progress`（`sizing` 阶段）
- 结果缓存到 `CleanerState`；再次进入页面优先用缓存，点「重新扫描」才重新枚举
- 前端首次加载显示骨架 + 进度文案，不阻塞其他页签

### 4.2 卸载流程

1. 用户点某行「卸载」→ 确认弹窗展示：应用名、版本、将执行的命令（`UninstallString` 或「移入废纸篓」）、是否提权
2. 可选勾选「先结束该应用进程」（默认勾选；按 exe 路径/bundle id 匹配运行中进程，优雅退出）
3. 执行：
   - **Windows**：`MsiExec.exe /I{GUID}` 转成 `/X{GUID}`；HKCU 条目普通权限启动，HKLM 条目走一次 UAC；统一用 PowerShell `Start-Process -FilePath <exe> -ArgumentList <args> -Wait -PassThru`，拿 `ExitCode`；命令解析用 Rust 实现 Windows argv 拆分（带单测）
   - **macOS**：结束运行中的实例 → bundle 用 `trash` 移入废纸篓（系统应用/SIP 保护会失败并原样报错）
4. 返回 `UninstallOutcome`；成功后前端自动进入「残留扫描」
5. 失败或条目没有 `UninstallString` 时，弹窗提供「强制移除」（见 4.5）；被占用文件删除失败只报错，不做重启延迟删除（非目标）

### 4.3 残留规则

**Windows**（每条带置信度）

| 来源 | 置信度 |
| --- | --- |
| `InstallLocation` 目录 | high |
| `HKCU/HKLM\SOFTWARE\<Publisher>\<Name>` 及 `\WOW6432Node` 下同名键 | medium（只列出，删除注册表需显式勾选） |
| `%APPDATA%`、`%LOCALAPPDATA%`、`%PROGRAMDATA%` 下与 Name/Publisher 精确同名的目录 | medium |
| 开始菜单、桌面里与 Name 精确匹配的 `.lnk` | high |
| `%LOCALAPPDATA%\Programs\<Name>` | high |

**macOS**（bundle id / 应用名精确匹配）

- high：`~/Library/Caches/<bundleId>`、`~/Library/Preferences/<bundleId>.plist`、`~/Library/Saved Application State/<bundleId>.savedState`、`~/Library/Application Support/<bundleId>`、`~/Library/Containers/<bundleId>`、`~/Library/Group Containers/*<bundleId>`、`~/Library/HTTPStorages/<bundleId>`、`~/Library/WebKit/<bundleId>`、`~/Library/Application Scripts/<bundleId>`、`~/Library/LaunchAgents/<bundleId>*.plist`
- medium：按应用名命名的 `Application Support`/`Caches`/`Logs` 目录
- 需要提权（`requires_elevation=true`）：`/Library/Application Support`、`/Library/Caches`、`/Library/Preferences`、`/Library/LaunchAgents`、`/Library/LaunchDaemons`、`/Library/PrivilegedHelperTools` 下的精确匹配项
- `pkgutil` receipts：本版本只读展示（kind=`receipt`），不自动删除

**匹配原则**

- 只做「精确同名」或「bundle id 前缀」匹配，不做模糊包含，避免误删
- 所有条目先 `symlink_metadata` 判存在；`exists=false` 的不显示
- 删除前逐条再过一次白名单；勾选状态由用户最终决定
- 结果 UI 按来源分组展示（Windows：文件系统 / 注册表两组，标题带数量；macOS：文件一组），支持组级全选

### 4.5 强制移除

- Windows：条目无 `UninstallString`（损坏/绿色软件残留）或卸载失败时，提供「强制移除」：删除该注册表条目（白名单内、必须是扫描时列出的键）→ 自动进入残留扫描 → 用户勾选删除
- macOS：正常路径就是「移入废纸篓」，无卸载器时不额外提供强制移除；SIP 保护的应用直接报错
- 强制移除不清理 COM 注册、服务、驱动（Geek 也不做），UI 注明「仅移除应用本体与可识别残留」

### 4.4 删除

- 统一走 `trash_guard::trash_path`：先校验路径在平台白名单根下，再调用 `trash::delete`
- 白名单根（只允许这些具体目录，**不允许整个 `$HOME`**）：
  - macOS：`/Applications`、`~/Applications`、`~/Library/{Application Support,Caches,Preferences,Logs,Saved Application State,Containers,Group Containers,HTTPStorages,WebKit,Cookies,Application Scripts,LaunchAgents}`、`/Library/{Application Support,Caches,Preferences,LaunchAgents,LaunchDaemons,PrivilegedHelperTools}`、`~/Library/Preferences/*.plist`
  - Windows：`%APPDATA%`、`%LOCALAPPDATA%`、`%PROGRAMDATA%`、`Program Files*` 下且必须带有应用名/发布者名作为路径段
- 明确拒绝：`/`、`/System`、`/usr`、`/bin`、`/sbin`、`C:\Windows`、`C:\`、用户主目录本身
- 注册表删除（Windows）：`winreg` 删除子键，白名单限制在 `HKCU/HKLM\SOFTWARE` 下，且必须是扫描时列出的键
- 需要提权的条目：macOS 用一次 `osascript ... with administrator privileges` 执行生成的临时 shell 脚本（逐行 `rm -rf` 用单引号转义）；Windows 用一次提权 PowerShell；失败条目逐条回传，不影响其他条目
- 每次删除都把「路径 + 结果」写进页面内的操作结果列表，供用户核对

## 5. 内存占用

### 5.1 采集

- `sysinfo` 单例 `System`，`refresh_memory` + `refresh_processes_specifics(All, true, ProcessRefreshKind::nothing().with_memory()::with_cpu())`（只取需要的字段）
- 总览：`total_memory/used_memory/available_memory/total_swap/used_swap`
- 进程：`pid/name/exe/memory`；`exe` 为空（已删除/内核进程）时跳过

### 5.2 应用聚合

- macOS：`exe` 向上找到最近的 `.app` 祖先 → 应用身份 = bundle 路径；`Info.plist` 取名与图标；找不到 bundle 的按 exe 路径单独成项（`kind="process"`）
- Windows：`exe` 位于 `Program Files*`/`Program Files (x86)`/`%LOCALAPPDATA%\Programs` 下时，取 exe 的父目录作为身份；父目录名是 `bin`/`Bin` 或形如版本号（`v1.2.3`、`1.2.3`）时再上一层；其余情况按 exe 路径单独成项
- 聚合结果按 `memory` 降序；同一身份的进程数一并展示
- 图标：复用 4.1 的图标提取，按应用级别缓存（前端不再单独请求）

### 5.3 轮询与生命周期

- 内存页签激活时每 2 秒调 `cleaner_memory_snapshot`；切走页签或 `document.hidden` 时暂停；重新可见时立即采样一次
- 采样是只读操作，不加锁、不进后台常驻线程；前端只保留最近一次快照
- 采样失败（权限/系统错误）时在页面内提示，不弹全局错误

### 5.4 结束进程

1. 勾选应用 → 「结束进程」→ 确认弹窗（列出应用名、进程数、合计内存，说明会丢失未保存内容）
2. 优雅退出：
   - macOS：`NSRunningApplication.runningApplicationWithProcessIdentifier` → `terminate()`（objc2-app-kit，现有依赖）
   - Windows：PowerShell `Get-Process -Id <pids> | ForEach-Object { $_.CloseMainWindow() }`（只对有主窗口的进程有效）
3. 等待 3 秒后重新采样，仍存活且用户选择「强制结束」→ 再确认一次 → `SIGKILL`/`Stop-Process -Force`
4. 返回 `QuitOutcome`；前端提示成功/失败数量并立即刷新快照

### 5.5 深度释放

- **Windows**：一次 UAC 提权 PowerShell：
  1. 遍历进程调用 `EmptyWorkingSet`（psapi，Add-Type P/Invoke）
  2. `NtSetSystemInformation(SystemMemoryListInformation=0x50, MemoryPurgeStandbyList=4)` 清待机列表；失败不影响第 1 步结果
- **macOS**：一次系统授权 `osascript -e 'do shell script "/usr/sbin/purge" with administrator privileges'`
- 释放前后各采样一次（释放后等待 1 秒再采样），`freed = max(before - after, 0)`，`note` 写明原理：
  - Windows：「清空各进程工作集与系统待机列表，被清掉的页仍在磁盘/页面文件里，系统会按需换入」
  - macOS：「purge 只清可回收的文件缓存，不释放应用已 malloc 的内存，收益有限」
- 用户取消授权 → `ok=false` + `message="已取消授权"`，不报错弹窗
- 深度释放按钮与「结束进程」分开，避免误触

## 6. 权限与安全

- 破坏性操作（卸载、删除残留、强制结束、深度释放）全部二次确认
- 删除只进废纸篓（`trash` crate），不做永久删除
- 系统应用、`/System`、`/usr`、系统盘根等硬拒绝
- 残留列表默认只勾选 high 置信度项；medium/low 必须手动勾
- 提权只按需触发，一次操作合并成一次授权；取消授权按「未执行」处理
- 所有后端错误用 `Result<_, String>` 返回中文可读信息（沿用项目现有命令风格）
- 不读取、不上传任何用户数据；内存快照只在本进程内存中保留一份

## 7. 后端结构

```text
src-tauri/src/cleaner/
  mod.rs                 // 对外导出 + 平台分派
  models.rs              // 3.1 全部类型
  installer.rs           // 列表/卸载/残留的统一入口 + 缓存 + 进度事件
  installer_macos.rs     // /Applications 扫描、Info.plist、残留规则、app 退出
  installer_windows.rs   // 注册表枚举、UninstallString 解析、残留规则、提权执行
  memory.rs              // sysinfo 采样 + 聚合主流程
  memory_macos.rs        // .app 归属、图标、NSRunningApplication 退出
  memory_windows.rs      // 产品目录归属、CloseMainWindow、EmptyWorkingSet/待机列表
  trash_guard.rs         // 白名单校验 + trash 封装 + 提权批处理

src-tauri/src/commands/cleaner.rs   // 薄封装 + CleanerState
```

- `commands/mod.rs` 增加 `mod cleaner; pub use cleaner::*;`
- `lib.rs`：`mod cleaner;`、`.manage(cleaner::commands::CleanerState::new())`、`generate_handler!` 注册 7 个命令
- 前端放在 `src/components/cleaner/`，由 `CleanerPanel.vue` 统一持有两个页签

## 8. 依赖

| crate | 版本 | 用途 |
| --- | --- | --- |
| `sysinfo` | 0.40，`default-features=false, features=["system"]` | 内存/进程采样（macOS + Windows 均已实测可编译） |
| `trash` | 5.3 | 跨平台废纸篓 |
| `plist` | 1 | 读 Info.plist（锁文件里已有） |
| `winreg` | 0.55 | Windows 注册表枚举（仅 Windows target） |
| `windows` | 0.61 | Windows 端少量 Win32 调用（仅 Windows target；与 trash 依赖的 0.62 并存不冲突） |

依赖可行性已用独立探针 crate 在 `aarch64-apple-darwin` 与 `x86_64-pc-windows-msvc`（cargo-xwin）两个 target 上 `cargo check` 通过。

## 9. 错误处理与测试

### 9.1 错误处理

- 列表/残留扫描失败：页面内错误条 + 「重试」按钮，不清空已有数据
- 卸载失败：`UninstallOutcome.ok=false`，展示命令与错误；用户可手动重试或改用系统卸载入口
- 删除部分失败：逐条展示；已成功的条目从列表移除
- 内存采样失败：页面内提示，轮询继续（下次可能恢复）
- 提权取消：按未执行处理，不红字报错

### 9.2 测试

Rust 单测（`cargo test`）：

- Windows：`UninstallString` 解析（含引号、`msiexec /I` 转换、无参数 exe）、注册表条目过滤/去重、`EstimatedSize` 换算、白名单校验
- macOS：Info.plist fixture 解析、bundle id 残留规则匹配、应用名匹配、符号链接防环
- 通用：内存聚合（构造假进程数据）、容量格式化、白名单拒绝系统路径

构建与静态检查：

- `pnpm build`（`vue-tsc --noEmit && vite build`）
- `cargo test --manifest-path src-tauri/Cargo.toml`
- `cargo xwin check --target x86_64-pc-windows-msvc`（Windows 端类型/API 正确性）

手工验收（macOS 本机）：

1. 进入清理页：列表带图标、版本、大小；搜索/排序生效；重新扫描有进度
2. 内存页签：总览与 Lemon 的占用率量级一致；应用聚合合理；轮询可暂停
3. 卸载一个测试应用（如自己装的小工具）：确认弹窗 → 卸载 → 残留扫描 → 勾选删除 → 废纸篓里能看到
4. 结束一个测试进程（如打开的预览/文本工具）：优雅退出成功；强制结束路径也验证
5. 深度释放：弹一次系统授权；取消授权有提示；成功时显示前后差值
6. 系统应用（如 Safari）不出现卸载入口

Windows 验收清单（用户在 Windows 机器执行）：

1. 列表与「应用和功能」数量量级一致，32/64 位注册表项都在
2. 卸载一个 MSI 应用（走 `/X` 转换）与一个 exe 卸载器应用
3. 卸载后残留扫描能列出安装目录/AppData/快捷方式，删除进回收站
4. 结束进程：CloseMainWindow 生效；强制结束生效
5. 深度释放：UAC 授权一次，工作集下降；取消授权有提示
6. 高 DPI/中文系统下路径与名称不乱码

## 10. 实施顺序

1. 后端骨架：`cleaner` 模块 + 类型 + 命令注册 + 空实现编译通过（mac/win 双 target）
2. Windows 卸载列表 + UninstallString 解析 + 单测
3. macOS 卸载列表 + Info.plist + 图标 + 单测
4. 删除/残留：`trash_guard` + 平台规则 + 单测
5. 内存：sysinfo 采样 + 聚合 + 结束进程
6. 深度释放（平台提权）
7. 前端：类型/API/composable/两个面板 + App.vue 入口 + 页签
8. 联调：进度事件、轮询生命周期、错误提示
9. 验证：`pnpm build`、`cargo test`、cargo-xwin check、macOS 手工验收；产出 Windows 验收清单
