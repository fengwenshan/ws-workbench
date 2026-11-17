# 开发细节

> 来源：原 [README](../README.md) 的「开发」章节；硬规则见 [AGENTS.md](../AGENTS.md)。

## 开发

```bash
pnpm install          # 安装前端依赖
pnpm dev              # 只起前端，http://localhost:8790
pnpm exec tauri dev   # 起完整桌面应用
```

改完通用层（`src-tauri/src/util`、`process` 等）或 VPN 逻辑后，本地跑一遍 Rust 单测（`dist/` 不存在时先 `pnpm build`，`generate_context!` 编译期要读 `frontendDist`）：

```bash
cd src-tauri && cargo test --lib
```

Vite 固定在 8790 端口并开启 `strictPort`，端口被占用会直接失败而不是换端口；`@` 别名指向 `./src`，监听忽略 `src-tauri/`。

**样式里的 px 一律用偶数**（`11px` 写 `12px`，`13px` 写 `14px`）：缩放不是 100% 时，奇数尺寸会被摆到半个像素上，边框、图标、文字都会发虚。`1px` 的描边和分隔线是刻意留的发丝线，不受这条约束。

注意别用 `pnpm tauri dev`。`package.json` 里有一个名为 `tauri` 的脚本（内容是 `tauri dev`），`pnpm tauri dev` 会被解析成 `pnpm run tauri dev`，最终执行 `tauri dev dev` —— 多出来的 `dev` 会被当成传给 runner 的附加参数，和预期行为不一致。

### 浏览器里看样式

`pnpm dev` 只起 Vite，没有 Tauri 注入的 `__TAURI_INTERNALS__`，直接打开 `http://localhost:8790` 会因 `getCurrentWindow()` 拿不到 `metadata` 而白屏。入口文件在 `import.meta.env.DEV` 下调用 `installBrowserTauriShim()`（`src/dev/browserTauriShim.ts`）补一份假后端：窗口元数据、事件注册，以及首页 / 设置 / LANLink / 剪贴板用到的只读命令和少量假数据。

- 浏览器里能直接看到带数据的界面，调样式不用每次起桌面应用；`/quick.html`、`/clipboard.html`、`/uninstall.html` 同样能用。
- 桌面应用里检测到真实 `__TAURI_INTERNALS__` 会直接返回；生产构建被 `DEV` 挡掉，`dist` 里不含假数据。
- 假数据只维护在 `src/dev/browserTauriShim.ts`，不写进业务代码；要预览新页面，在那里补一条命令返回即可。

### 平台代码分离

Windows 与 macOS 的实现必须拆开写，禁止在共享文件里用 `cfg` 堆两个平台的业务分支——目的是修一个平台时别把另一个改坏：

- 结构照 `src-tauri/src/vpn/platform/`：`mod.rs` 只做平台分派，`common.rs` 放平台无关逻辑，`macos.rs` / `windows.rs` 各自完整实现；两个平台文件之间不互相引用。
- 共享层只留「拿掉系统调用、权限、注册表/plist、安装卸载机制后仍成立」的纯逻辑；系统 API 一律放对应平台文件。
- 改动必须过 CI 的 `test-macos` / `test-windows` 两个 job。
- 本机验证不了的代码用 `// TODO(platform-verify:windows): <待验证行为>；验证步骤：<命令/手动步骤>；期望：<结果>` 标记，换到该平台验证通过后再删；提权/删除/卸载等高风险路径不得带未验证 TODO 发版。

软件卸载、屏幕休眠、hosts、剪贴板、项目打包的平台代码都已按此规范拆完（`mod.rs` 只做分派，平台文件内不再出现 `cfg(target_os)`，两个平台文件互不引用）：

- 卸载：`cleaner/installer_macos/`、`cleaner/installer_windows/`（`mod.rs` 门面 + 纯解析 / 注册表或 bundle / 卸载 / 残留 / 关联文件子模块）、`cleaner/running/`、`cleaner/trash_guard/`；
- 屏幕休眠：`cleaner/sleep/`（共享会话状态机；`macos.rs` 是 IOKit 电源断言、`windows.rs` 是 `SetThreadExecutionState`，`triggers/` 下 pmset + CoreGraphics 与 `GetSystemPowerStatus` + `GetSystemMetrics` 各一份）；
- hosts：`hosts/file/`（共享读取与提权编排、`scripts.rs` 纯脚本文本生成；`macos.rs` osascript 提权、`windows.rs` UAC PowerShell 提权）；
- 剪贴板：`clipboard/`（共享采集编排 / 快捷键 / 面板门面；`macos.rs` NSPasteboard + NSWorkspace，`windows.rs` Win32 剪贴板与焦点）；
- 数据目录：`storage/`（共享迁移逻辑，`macos.rs` / `windows.rs` 只列旧目录候选）；
- 项目打包：`packer/svn_platform/`（svn 候选路径与 Windows `\\?\` 前缀处理各一份）。

门面分派（`cleaner/installer.rs`、`cleaner/related.rs`、`commands/cleaner*.rs`、`uninstall_info.rs` 里只影响门面/窗口装饰/脚本落盘的零散 `cfg`）属于允许写法。还没拆的共享文件（下一批按本规范处理）：`vpn/split.rs`、`vpn/route.rs`、`vpn/fulltunnel.rs`、`vpn/elevate.rs`、`vpn/worker.rs`、`vpn/credential.rs`、`vpn/script.rs`（`vpn/platform/` 只覆盖连接器探测那一块）、`commands/openwith.rs`、`crypto.rs`（钥匙串 / DPAPI）、`apps/finder.rs`、`process.rs` / `winps.rs`、`lib.rs` 的窗口/托盘/菜单、`cleaner/popup.rs`；`platform.rs` 本身是平台判定入口，不算。Windows 实机验收项已用 `TODO(platform-verify:windows)` 标注，发版前用 `grep -rn "TODO(platform-verify" src-tauri/src src-tauri/nsis src-tauri/resources` 清点（NSIS 钩子和 PowerShell 资源里的 TODO 不在 Rust 目录，别漏）。

### 消息订阅与状态同步

后端状态和前端动作要分开：**状态用订阅，动作用命令**。不要把所有交互都做成事件，也不要每个组件各自轮询、各自求和。判断标准是「这份状态是否需要长期、跨组件地跟着后端变化」，不是「能不能用事件」。

| 场景 | 做法 |
| --- | --- |
| 同一状态被多个窗口/多个组件长期展示（未读数、VPN 连接状态、传输进度、peer 在线列表、托盘角标） | 订阅后端事件 |
| 状态由后台线程/另一个窗口改变，前端无法从本地操作推导（收到消息、定时探测、传输完成） | 订阅后端事件 |
| 打开下拉框、详情页首屏、表单草稿等只在当前组件生命周期内用一次的数据 | 调 command 取，不用事件 |
| 模态框、选中项、校验提示等纯前端临时 UI 状态 | 组件内 `ref`，不用事件 |
| 能由已有状态推导的值（例如「未读总数」） | 只维护一份源状态，其余 computed；不要为派生值再发事件 |
| 需要返回值/错误/超时/重试的动作（发送、删除、保存） | command；事件只做「发生了变化」的通知，不当 RPC |
| 高频或大 payload 的数据 | 事件只带 id/计数/进度，正文让前端按需拉；聊天事件只带 `peer_id/kind/direction` 就是按这条做的 |

实现要求：

- **单一事实源在后端**（SQLite / 运行时状态）。一个状态只维护一份，禁止各组件遍历列表自己求和；反例是 QuickPanel 以前用 `conversations.reduce(...)` 算未读，现在统一订阅 `lan-share://unread`。
- **事件命名**用 `域://事件`，与命令名一起写进设计文档；payload 用 serde 结构、前后端字段名对齐，并加序列化单测锁契约（参考 `UnreadPayload` 的 `{"total":n}` 断言）。
- **统一出口推送**，不要在多处零散 emit；已经算出来的值顺手复用（例如 emit 未读时把 total 传给托盘，别为托盘再查一次库）。
- **前端封装在 composable**：`onMounted` 先订阅再拉初始值，用 revision 丢弃过期初始值，`onUnmounted` 取消订阅并处理 disposed 竞态；不要在组件里裸 `listen` 不清理。
- **每个 WebView 各自订阅**（主窗口、QuickPanel 独立）。不要试图跨窗口共享 JS 单例；跨窗口一致性靠后端事件广播。
- **幂等与降噪**：同一状态重复推送要幂等（前端覆盖赋值、托盘按状态去重）；高频事件做防抖/节流。
- **失败降级**：订阅失败只打日志、不阻塞界面；首屏初始值用 command 兜底，事件恢复后自动补齐。
- **先查再建**：新增事件前先看有没有现成的域事件能扩展；事件越多，契约、清理和排查成本越高。

评审新增订阅时先回答四个问题：谁消费？需要跨窗口吗？订阅生命周期谁负责？payload 是否最小？答不上来就先用 command。

### 实机冒烟

单测只覆盖纯逻辑（过滤 / 去重 / `UninstallString` 解析），「列表 → 残留扫描 → 删进回收站 → 卸载 → 注册表键删除」这条链路要真机。每个平台各有一份 `#[ignore]` 的冒烟用例，只在手动执行时跑，且写入都用唯一名字、结束（含断言失败）自动清理：

```bash
# macOS：真实写 ~/Applications、~/Library 与废纸篓
CLEANER_SMOKE=1 cargo test --manifest-path src-tauri/Cargo.toml --lib \
  -- --ignored cleaner::macos_smoke

# Windows：真实写 HKCU 卸载项、%LOCALAPPDATA%/%APPDATA%/桌面快捷方式与回收站
CLEANER_SMOKE=1 cargo test --manifest-path src-tauri/Cargo.toml --lib \
  -- --ignored --test-threads=1 cleaner::windows_smoke
```

Windows 上有两个坑：

- 需要 UAC 的路径（提权批量删除、HKLM 键删除、真实 MSI 卸载）**不在**冒烟里，自动化不能替人点「是」，仍按 `TODO(platform-verify:windows)` 人工验收。
- 有些安全软件（火绒等）对「桌面 / 下载 / 临时目录里跑出来的新二进制」限制写用户目录，此时用例会直接失败并提示。绕过办法：把编译好的测试二进制复制到 `%LOCALAPPDATA%\Programs\` 下再跑，例如
  `copy src-tauri\target\debug\deps\ws_workbench_lib-*.exe "%LOCALAPPDATA%\Programs\wswb-probe\smoke.exe"`，
  然后在那个目录执行 `smoke.exe --ignored --test-threads=1 cleaner::windows_smoke`（记得同样带上 `CLEANER_SMOKE=1`）。临时目录（`%TEMP%`）被这类软件拦住时，`cargo test` 会大量报 `拒绝访问`，把 `TEMP`/`TMP` 指到仓库内的 `.tmp\test-temp` 即可。

### 重启

改完代码要重启时用 `scripts/dev-restart.ps1`，它会先结束残留进程再拉起一份：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\dev-restart.ps1
```

脚本按仓库路径匹配进程命令行，只结束本仓库的 `ws-workbench*`（含开发版的 `ws-workbench-dev`）、vite、tauri 进程，不误伤其他项目；检测到 `openconnect` 仍在运行会直接退出，避免留下无人回收的路由。

脚本不经过 pnpm，因为本机 PATH 里的 pnpm 来自 TRAE 自带的 corepack 0.32.0，它给 pnpm 12 生成的 shim 指向 `bin/pnpm.cjs`，而 pnpm 12 只提供 `bin/pnpm.mjs`，调用必然 `MODULE_NOT_FOUND`。脚本改为直接调用 `node_modules\.bin\tauri.cmd`，并用 `-c` 把 `beforeDevCommand` 覆盖成 `npm run dev`。另装一份能跑通的 pnpm 12.5.1 也不合适：它会把 `packageManagerDependencies` 和 `@pnpm/exe.*` 写进 `pnpm-lock.yaml`，CI 用的 pnpm 10 读不懂这个格式。

脚本文件必须保存为带 BOM 的 UTF-8。Windows PowerShell 5.1 对无 BOM 的 `.ps1` 按 ANSI 解码，中文注释会被解成乱码并把脚本解析坏。

### 开发版与安装版同时运行

开发构建（`pnpm exec tauri dev` / 本地 debug 二进制）的应用名是 `ws-workbench-dev`，发布版是 `ws-workbench`，两者可以同机同时各开一个：

- macOS：Dock 磁贴名、窗口标题显示 `ws-workbench-dev`；**菜单栏左上角那个应用名改不动，别指望它**——那是 AppKit 按应用身份画的，不是我们菜单里的文字。已经试过并排除：改可执行文件名、`NSProcessInfo.setProcessName`、`build.rs` 往二进制 `Info.plist` 里注入 `CFBundleName`（`otool -X -s __TEXT __info_plist` 确认嵌进去的是 `ws-workbench-dev`）、启动后补写 `NSApp.mainMenu` 第 0 项的 title（每 200ms 到每秒重写一次）：前三个都无效，补写能让菜单项 title 真的变成 `ws-workbench-dev`（读回来也是），但菜单栏显示始终是 `ws-workbench`，系统会按应用身份重新画。所以开发版靠这几处区分：Dock 图标右下角的 `dev` 角标、Dock 磁贴名、窗口标题、菜单栏右上角托盘图标正中的镂空小圆点（图标颜色仍由系统按菜单栏明暗染色，和发布版一致）、托盘面板的 `dev` 标签、主窗口标题栏更新按钮后面的 `dev` 标签
  - Dock 磁贴名 = **可执行文件名**：开发构建的 bin target 直接叫 `ws-workbench-dev`（`target/debug/ws-workbench-dev`），打包时由 `tauri.conf.json` 顶层的 `mainBinaryName: "ws-workbench"` 把产物改名回 `ws-workbench`；
  - Dock / 托盘图标：开发版用带 `dev` 标识的一套图标——Dock 走 `icons/dev/icon-dev.png`（正式图标 + 右下角白底蓝字 `dev` 角标，`set_dock_icon` 在 debug 下换用）；macOS 菜单栏托盘走 `icons/tray/tray-dev-template-36.png`（和发布版**同一张模板图**，只在箱体正中镂空一个透明小圆点：模板图会被系统按菜单栏明暗染色，所以开发版颜色和发布版完全一样，镂空处直接透出菜单栏背景，18pt 见方、不比发布版宽）；Windows / Linux 托盘用右下角带 `dev` 角标的彩色应用图标（`icons/dev/32x32-dev.png`）；发布版仍是正式图标；资产由 `design/icons/tools/build.py` 生成
  - `src-tauri/build.rs` 在 debug profile 下仍会把 `bundle.macOS.bundleName` 合并进 `TAURI_CONFIG`，让二进制里嵌的 `Info.plist` 的 `CFBundleName`、`NSRunningApplication.localizedName` 是 `ws-workbench-dev`（release profile 不注入）；对菜单栏没用，只是让系统信息/关于面板里的名字一致
- Windows：自绘标题栏、窗口标题、任务栏都显示 `ws-workbench-dev`；窗口 / 任务栏 / exe 文件图标换成右下角带 `dev` 角标的彩色图标（`icons/dev/*`，启动时 `set_dev_window_icon` 设窗口图标，exe 资源由 `build.rs` 嵌入 `icon-dev.ico`），托盘图标用右下角带角标的彩色应用图标 `icons/dev/32x32-dev.png`（浅色任务栏也认得出）；单实例互斥量也按构建区分（`ws-workbench-single-instance` / `ws-workbench-dev-single-instance`）
- 单实例边界：发布版之间互斥、开发版之间互斥，但一个发布版 + 一个开发版可以并存（不是多开程序）
- WebView2 用户数据目录按构建分开（`webview` / `webview-dev`），避免两个进程锁同一个目录
- 数据库与 `config.json` 仍然共用（同一个 identifier），改动会互相可见；因此开发版**不跑剪贴板后台**：不注册全局快捷键、不采集历史（共享库里否则每复制一次会出现两条），也不允许在设置里改快捷键（否则会抢发布版的 `⌘⇧V`、改掉共用的组合键）
- 开发版**不检查更新**：启动时那次静默检查和每天 10:00/16:00 的定时校验都跳过，标题栏也不显示「更新」按钮——裸二进制装不了更新包，Windows 上还会把 `target/debug/ws-workbench-dev.exe` 覆盖成发布版
- 界面内的开发版标识：托盘快捷面板在页签右侧多一个 `dev` 小标签（`QuickPanel.vue` 的 `.qp-dev-tag`），主窗口标题栏在「更新」按钮后面也多一个 `dev` 小标签（`App.vue` 的 `.titlebar-dev-tag`），发布版两处都不渲染；两个窗口的网页标题也按构建名（`ws-workbench-dev`）
- 图标能换的边界：Dock / 任务栏 / 托盘图标都是运行时设置的，开发版能带上 `dev` 角标；Windows 的 **exe 文件图标**（资源管理器、桌面快捷方式、任务栏固定项读的那枚）由 `build.rs` 在 debug + windows 时经 `WindowsAttributes::window_icon_path` 嵌成 `icons/dev/icon-dev.ico`，也跟着变；macOS 的 Finder 文件图标和安装包图标换不了——开发版是裸二进制，没有 bundle 可换（`tauri build --debug` 打出来的 `.app` 也走 `tauri.conf.json` 里正式的 `icon.icns`）
- 名称来源细节：`tauri dev` 实际是 `cargo run --no-default-features`（关掉 `custom-protocol`），这时 `tauri-codegen` 才会把 `Info.plist` 嵌进二进制；`tauri build` 带 `custom-protocol` 且打包成 `.app`，名字走包的 `Info.plist`，与开发版互不影响
- 调试这类「名字没变」的问题注意：dev 版有单实例互斥，旧进程没退干净时再启动 `tauri dev`，新进程会直接退出、把旧窗口顶到前面，看起来就像「重启了还是旧名字」；先确认 `pgrep -fl ws-workbench` 没有输出再启动
- 开发构建的 exe / 进程名变了，所以按进程名找本应用的脚本都用通配：`ws-workbench*`（`dev-restart.ps1`、`collect-vpn-logs.ps1`）；前端 cleaner 的「不自动处理本应用窗口」按当前 exe 名兜底比较，开发版也命中
