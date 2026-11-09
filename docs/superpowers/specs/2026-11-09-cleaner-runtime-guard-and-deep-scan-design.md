# 软件卸载：运行中检测 + 二次确认 + 残留深扫设计（2026-11-09）

> 增补 `docs/superpowers/specs/2026-11-06-cleaner-design.md` 的 4.2 / 4.3。
> 设计范围：macOS 完整实现并在本机验收；Windows 只做交叉编译检查与验收清单。
> 用户已确认的交互：检测到应用仍在运行时弹二次确认，按钮为「结束进程并卸载 / 取消」；
> 先优雅退出，退不掉就中止卸载并提示手动处理，不静默强杀。

## 1. 背景与目标

现有卸载链路（列表 → 卸载 → 自动扫描残留 → 手动勾选删除 → 进废纸篓）已可用，但有三处不完整：

1. 卸载前不判断应用是否仍在运行。macOS 把运行中的 `.app` 移入废纸篓后进程仍在，删除缓存可能失败或被重新生成；Windows 运行中的文件也会让卸载/删除失败。
2. macOS `Saved Application State` 规则拼出的路径缺少 `.savedState` 后缀，永远命中不了；`Preferences/ByHost`、`CrashReporter`、`DiagnosticReports`、`.binarycookies` 等常见残留没有规则。
3. 残留只按 bundle id / 应用名的「精确同层」匹配，漏掉 helper 容器（微信 `.MiniProgram` 等）、厂商多级目录（Chrome `Application Support/Google/Chrome`）和带后缀的变体。

本次目标：

- 卸载前检测运行状态；仍在运行时二次确认；确认后先优雅退出，退出失败则中止卸载。
- 修复 macOS 残留规则缺陷，增加低置信度深扫（默认不勾选）。
- 删除残留成功后自动重扫，避免列表停在旧状态。

## 2. 非目标

- 不新增「永久删除 / 清空废纸篓」模式；普通条目仍只进废纸篓。
- 不恢复已被移除的内存采样 / 结束进程页签；运行检测只服务卸载流程。
- Windows 不做低置信度深扫（bundle id 是 macOS 概念），也不在本机做运行时验收。
- 不做开机后延迟删除被占用文件（沿用 2026-11-06 设计的非目标）。

## 3. 交互流程

```
点行内「卸载」→ 卸载确认弹窗（原有）
  └─ 点「确认卸载」
       ├─ 查运行状态：未运行 → 直接卸载（原有流程）
       └─ 运行中 → 二次弹窗「应用正在运行」
            ├─ 取消 → 回到卸载确认弹窗，什么也不做
            └─ 结束进程并卸载
                 ├─ 优雅退出成功 → 继续卸载 → 成功后自动扫描残留
                 └─ 5 秒内未退干净 → 中止卸载，弹窗内展示失败原因
```

二次弹窗文案（含运行进程数与最多 3 个进程名）：

> 「微信」有 3 个进程正在运行（WeChat、WeChatAppEx Helper 等）。卸载前会先结束这些进程；未能退出时本次卸载会中止，请手动退出后重试。
>
> [取消] [结束进程并卸载]

实现细节：

- 运行状态在「确认卸载」时查询（不在打开弹窗时查），避免列表页每次点卸载都多一次枚举。
- 二次弹窗确认后调用卸载命令并带 `quitRunning=true`；后端在执行卸载前再检测一次，避免弹窗打开期间应用被重新启动的竞态。
- 退出期间复用 `cleaner-progress` 事件（`task="uninstall"`、`phase="waiting"`），前端在卸载弹窗内显示「正在退出应用…」。
- 退出失败返回 `UninstallOutcome.ok=false`，沿用现有失败展示（命令 / 退出码 / 原因），不弹全局错误。

## 4. 数据模型与命令

```rust
/// 应用运行状态（cleaner_app_running 返回值）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunningState {
    pub running: bool,
    /// 进程数（主进程 + 该应用 bundle/安装目录内的 helper 进程，按 pid 去重）
    pub processes: usize,
    /// 进程显示名，去重；前端只展示前 3 个
    pub names: Vec<String>,
}
```

命令变更：

| 命令 | 签名 | 说明 |
| --- | --- | --- |
| `cleaner_app_running` | `(app_id: String) -> Result<RunningState, String>` | 只读检测，不弹授权 |
| `cleaner_uninstall_app` | 增加 `quit_running: bool` | `true` 时先退出再卸载；退出失败直接返回 `ok=false`，不执行卸载 |

前端 API：

```ts
appRunning(appId): Promise<RunningState>
uninstallApp(appId, quitRunning = false): Promise<UninstallOutcome>
```

## 5. 运行检测与退出实现

新增 `src-tauri/src/cleaner/running.rs`（与 `trash_guard.rs` 一样按 `cfg` 分平台放在一个文件里）：

### 5.1 公共部分

- `detect(state_cache 中的 app) -> RunningState`：平台实现只负责产出 `(pid, name, exe)` 列表，公共层去重、排序、组装。
- 纯逻辑 `path_within(exe, root)`：路径按组件比较，Windows 大小写不敏感、macOS 区分大小写；根目录自身不算「在其中」。
- 纯逻辑 `usable_install_root(raw) -> Option<PathBuf>`：拒绝空串、盘符根、`Program Files` / `AppData` / `ProgramData` / 用户主目录这类公共父目录，避免把「安装位置」写成公共目录时把所有进程都算成该应用。
- 超时：常量 `QUIT_TIMEOUT = 5s`，轮询间隔 250ms；超时即失败，不强制结束。

### 5.2 macOS

检测：

1. `read_bundle_info(bundle)` 取 bundle id；`NSRunningApplication::runningApplicationsWithBundleIdentifier`（objc2-app-kit，依赖已有）拿到主应用实例。
2. `sysinfo` 枚举进程，`exe` 在目标 `.app` 路径内的进程也算该应用（覆盖 Electron helper、XPC、扩展进程）。
3. 两组按 pid 去重；bundle id 为空时只用第 2 步。

退出：

1. 对第 1 步的 `NSRunningApplication` 逐个 `terminate()`（应用可弹保存确认框）。
2. 对第 2 步中不在第 1 步的 pid，用 `runningApplicationWithProcessIdentifier(pid)` 找到实例后 `terminate()`；找不到实例的 pid 不处理，交由超时判定。
3. 轮询检测直到 0 个进程或 5 秒超时。

### 5.3 Windows

检测：

1. 安装根：优先 `InstallLocation`；为空时取注册表 `DisplayIcon`（去掉 `,0` 后缀）所在目录；仍不可用则返回「未运行」（宁可漏提示，不误弹）。
2. `sysinfo` 枚举进程，`process.exe()` 在安装根内即算运行中；公共父目录会被 `usable_install_root` 拒绝。

退出：

1. PowerShell 脚本（`winps::run_with_timeout_ms`，15 秒上限）：

```powershell
$pids = @(1234,5678)
foreach ($p in $pids) {
  $proc = Get-Process -Id $p -ErrorAction SilentlyContinue
  if ($proc) { $null = $proc.CloseMainWindow() }
}
```

2. 轮询检测直到 0 个进程或 5 秒超时。
3. 托盘应用 / 无主窗口进程 `CloseMainWindow()` 无效，会按「退不掉」中止并提示手动退出（已知限制，写进 README）。

## 6. 残留规则修复（macOS）

### 6.1 修正与新增的固定规则

| 规则 | 类型 | 置信度 | 说明 |
| --- | --- | --- | --- |
| `~/Library/Saved Application State/<bundleId>.savedState` | app_data | high | 修正现有路径缺少 `.savedState` 的 bug |
| `~/Library/Preferences/ByHost/<bundleId>.*.plist` | preference | high | 目录内按前缀匹配，含 `<bundleId>.ShipIt.<UUID>.plist` |
| `~/Library/Application Support/CrashReporter/<bundleId|name>_*` | crash_report | medium | 崩溃报告 |
| `~/Library/Logs/DiagnosticReports/<bundleId|name>_*` | log | medium | 诊断报告（文件与目录都算） |
| `~/Library/HTTPStorages/<bundleId>.binarycookies` | cache | high | 与现有 `<bundleId>` 目录规则并列 |
| `~/Library/Cookies/<bundleId>.binarycookies` | cache | medium | 旧版 Cookie 位置 |

`LeftoverKind` 前端类型补充 `crash_report`，`format.ts` 补中文标签「崩溃报告」。

### 6.2 低置信度深扫

目标：命中现有「精确同层」规则漏掉的条目，默认**不勾选**，由用户逐条确认。

扫描根与深度（只扫用户 `~/Library`，不碰系统级目录）：

| 根 | 深度 | kind |
| --- | --- | --- |
| `Library/Caches` | 2 | cache |
| `Library/Application Support` | 3 | app_data |
| `Library/Containers` | 1 | container |
| `Library/Group Containers` | 1 | container |
| `Library/HTTPStorages` | 1 | cache |
| `Library/WebKit` | 1 | cache |
| `Library/Application Scripts` | 1 | app_data |
| `Library/Saved Application State` | 1 | app_data |
| `Library/Preferences` | 1 | preference |
| `Library/Preferences/ByHost` | 1 | preference |
| `Library/Logs` | 2 | log |

条目标识（去空白、大小写不敏感）：

- 主键：bundle id 精确相等、`<bundleId>.` 前缀（helper 容器/扩展）、`.<bundleId>` 后缀（Group Containers）。
- 变体：去掉 `.savedState` / `.binarycookies` / `.plist` 后缀后按上面的主键再比一次。
- 应用名：路径中任一段与 `CFBundleName/DisplayName` 精确相等；bundle id 最后一段（如 `com.google.Chrome` 的 `Chrome`）也作为名字候选，用于命中厂商多级目录。
- 排除：已被 high/medium 规则命中的路径；扫描根自身；隐藏文件（`.` 开头）。
- 产出的 `confidence="low"`、`selected=false`、`requires_elevation=false`；总数上限 200 条，防止弹窗刷屏。
- 全部结果按路径去重后再和前两类结果合并。

示例（本机实测）：

- Chrome：深扫补出 `Application Support/Google/Chrome`、`Caches/Google/Chrome`。
- 微信：深扫补出 `Containers/com.tencent.xinWeChat.MiniProgram`、`.WeChatMacShare`、`.WeChatFileProviderExtension` 等。
- VS Code：深扫补出 `Caches/com.microsoft.VSCode.ShipIt`，ByHost 规则补出 `com.microsoft.VSCode.ShipIt.<UUID>.plist`。

可测性要求：候选生成函数接收 `home: &Path` 参数（公共 API 仍用 `home_dir()`），单测用 `tempfile::tempdir()` 构造假 home。

## 7. 删除后自动重扫

- 前端在 `deleteLeftovers` 成功后重新调用一次 `scanLeftovers`（保留逐条删除结果展示，不重建弹窗）。
- 重扫结果重新计算默认勾选（仍只勾 high、仍排除只读 receipt）。
- 重扫失败时保留现有列表与删除结果，只在弹窗内显示错误条 + 重试按钮。

## 8. 错误处理

| 场景 | 行为 |
| --- | --- |
| 运行检测失败（枚举不到进程 / 读不到 exe） | 按未运行继续卸载，不阻断 |
| 二次弹窗取消 | 回到卸载确认弹窗，不做任何操作 |
| 退出超时 / 有进程退不掉 | `UninstallOutcome.ok=false`，message 写明「仍有 N 个进程未退出…本次未执行卸载」 |
| 退出过程中应用自行重启 | 卸载前再检测一次；仍运行则按退出失败处理 |
| 深扫路径读不到 | 跳过该条目/该根，不报错 |
| 删除后重扫失败 | 保留删除结果，显示错误条 + 重试 |

## 9. 测试

Rust 单测（`cargo test --lib`）：

- macOS 候选规则：`.savedState` 正确命中；ByHost 前缀匹配；CrashReporter / DiagnosticReports 前缀匹配；`.binarycookies`；深扫在假 home 下命中 helper 容器、厂商多级目录、ByHost 变体；深扫结果 `confidence=low` 且 `selected=false`；与 high/medium 去重；超过上限截断；隐藏文件跳过。
- 纯逻辑：`path_within`（bundle 内、安装目录、大小写、边界、公共父目录拒绝）、`usable_install_root`、名字/前缀/后缀匹配函数。
- 现有 364 项测试保持全绿。

前端检查：`vue-tsc --noEmit` 通过；`RunningState` 类型、二次弹窗状态机、删除后重扫无类型错误。

手工验收（macOS）：

1. 打开一个测试应用 → 点卸载 → 确认 → 出现二次弹窗（进程数/名字正确）→ 取消：不卸载。
2. 再次确认 → 结束进程并卸载：应用退出、`.app` 进废纸篓、自动进入残留扫描。
3. 应用弹保存确认框时超时：卸载中止，弹窗显示失败原因，`.app` 仍在原位。
4. 卸载后残留列表包含深扫条目（low、默认不勾）；勾选删除 → 自动重扫，已删条目消失。
5. 未运行的 app：不弹二次弹窗，直接卸载。

Windows：`cargo xwin check --target x86_64-pc-windows-msvc` 通过；输出实机验收清单（检测、CloseMainWindow、退不掉中止、卸载与回收站）。

## 10. 影响文件

- 后端：`src-tauri/src/cleaner/running.rs`（新增）、`src-tauri/src/cleaner/installer_macos.rs`、`src-tauri/src/cleaner/installer.rs`、`src-tauri/src/cleaner/models.rs`、`src-tauri/src/commands/cleaner.rs`、`src-tauri/src/lib.rs`、`src-tauri/src/cleaner/mod.rs`
- 前端：`src/types/cleaner.ts`、`src/api/cleaner.ts`、`src/composables/useCleaner.ts`、`src/components/cleaner/InstalledAppsPanel.vue`、`src/components/cleaner/format.ts`
- 文档：`README.md`、本文件、`docs/superpowers/specs/2026-11-06-cleaner-design.md`（加增补指引）

## 11. 已知限制

- 运行检测依赖进程 exe 可见性；系统完整性保护 / 其他用户进程可能枚举不到，此时按未运行处理。
- Windows 无实机验证；托盘应用可能因 `CloseMainWindow` 无效而中止卸载。
- 深扫是启发式匹配，可能漏（名字与 bundle id 完全无关的数据）也可能多（通用名应用），所以固定为 low + 默认不勾选。
- 退出不强制杀进程；应用坚持不退出时需要用户手动处理。
