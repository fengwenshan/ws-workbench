# 项目规则（ws-workbench）

本文件是**硬规则**，`README.md` 只做项目简介与上手入口，功能与实现细节在 `docs/`（见 README「文档索引」）。
规则与文档冲突时，按「安全 > 加密 > 平台隔离 > 本文件 > docs」处理，并在同一次提交里把两边改一致。
动代码前先看第 1～4 节，收尾按第 15 节清单过一遍。

## 0. 项目速览

- Tauri 2 + Vue 3.5 + TypeScript + Vite 6 + ant-design-vue 4；Rust 2021，库名 `ws_workbench_lib`。
- 命令按业务域放在 `src-tauri/src/commands/<域>.rs`（config / project / system / attachments / cleaner / clipboard / hosts / lanshare / quick / openwith / deepseek / resources）。
- 业务数据在 `ws-workbench.db`（SQLite，`payload` 列加密），设置在 `config.json`（整份加密），附件与剪贴板图片是 WST1 密文。
- 平台：macOS 11+ / Windows 10+；VPN 走内置 openconnect + 路由注入（不抓包）。
- 细节（功能语义、文件清单、已知限制）看 `docs/`，入口见 README「文档索引」。

## 1. 每次任务收尾都要重启并验证

改完代码（尤其 Rust）必须重启应用再看效果——`tauri dev` 不会热更 Rust，重启才算改动生效。

- **Windows**：`powershell -ExecutionPolicy Bypass -File scripts\dev-restart.ps1`。
  它先结束本仓库残留的 `ws-workbench*`、vite、tauri 进程再拉起；检测到 `openconnect`
  仍在运行会直接退出（先断开 VPN）。脚本故意不经过 pnpm（本机 pnpm 的 shim 有问题），不要改成 `pnpm tauri dev`。
- **macOS**：先 `pgrep -fl ws-workbench` 确认没有残留（单实例互斥会把新进程顶掉，看起来像「改了没生效」），
  再结束残留的 vite / tauri 进程，用 `pnpm exec tauri dev` 重新拉起。
  注意别用 `pnpm tauri dev`：`package.json` 里的 `tauri` 脚本会把它解析成 `tauri dev dev`。
- 重启后至少手工走一遍改动涉及的路径（点按钮 / 看日志 / 看托盘），只看到编译通过不算验证。
- `.ps1` 必须保存为**带 BOM 的 UTF-8**：Windows PowerShell 5.1 对无 BOM 的脚本按 ANSI 解码，中文注释会被解成乱码并把脚本解析坏。

## 2. 分支、提交与文档

- 日常开发在 `beta` 分支；正式版 = `beta` 合进 `main`；临时特性开 `feat/<名字>`，合回 beta。
- 提交信息用 Conventional Commits：`type(scope): 中文一句话`。
  - type 只用：`feat` / `fix` / `docs` / `style` / `refactor` / `perf` / `test` / `chore` / `ci`；
  - scope 写模块名（`vpn` / `cleaner` / `lanshare` / `project` / `release` / `tray` / `app` 等），跨模块可省；
  - 描述写「改了什么、为什么」，不写 `update` / `fix bug` 这类无信息量的话。
- 新功能先设计再写代码：设计文档 `docs/superpowers/specs/YYYY-MM-DD-<主题>-design.md`，
  实施计划 `docs/superpowers/plans/YYYY-MM-DD-<主题>.md`（复选框分步），然后才动代码。
- 改行为同步对应 `docs/` 文档（功能 → `features`、开发 → `development`、发布 → `release`、数据 → `data`、平台 → `platform`、界面 → `ui`）；改规则同步本文件；代码、文档、本文件不一致时在同一次提交里改齐。
- 不手工打 tag、不手工改 `updates/latest.json`（由发布脚本生成；救火流程见 [docs/release.md](docs/release.md)「内测（beta）通道」）。
- 不提交构建产物与机器状态：`dist/`、`release/`、`src-tauri/target/`、`.tmp/`、`.superpowers/`、
  `src-tauri/.config/`、`src/auto-imports.d.ts`、`src/components.d.ts`（后两个由插件生成）。

## 3. 工具链与命令

- Node 22 + pnpm（CI 固定 10）；`pnpm-lock.yaml` 是 9.0 格式。**不要用 pnpm 12 跑 install**：
  它会把 `packageManagerDependencies` 和 `@pnpm/exe.*` 写进锁文件，CI 的 pnpm 10 读不懂。
- `pnpm dev` 只起前端（固定 8790 端口 + `strictPort`，被占用直接失败；`@` 别名指向 `./src`）；
  完整桌面应用用 `pnpm exec tauri dev`。
- `pnpm build` = `vue-tsc --noEmit && vite build`；构建与发布流程见第 14 节。
- `dist/` 是 `generate_context!` 的编译期依赖：没 build 过前端时 `cargo test` 会直接编译失败，先 `pnpm build`。
- Rust 单测：`cd src-tauri && cargo test --lib`。
- 本机临时目录被安全软件拦住时，把 `TEMP` / `TMP` 指到仓库内 `.tmp/test-temp`（[docs/development.md](docs/development.md)「实机冒烟」有说明）。

## 4. 验证门槛（没跑过不算完成）

- 前端 / 共享层改动：`pnpm build` 绿，并按第 1 节重启后手工验证。
- 动 Rust：`cd src-tauri && cargo test --lib` 绿。
- 动共享层（`util/`、`process.rs`、`crypto.rs` 等）或平台文件：两个平台的 CI（`test-macos` / `test-windows`）都要绿；
  本机跑不到对面平台就依赖 CI，不要只跑一边就宣布完成。
- 只在真机才能验证的路径：按第 7 节写 `TODO(platform-verify:…)`、按第 12 节验收，不得口头说「应该没问题」。
- 报告结论必须带证据：跑了什么命令、结果是什么。没验证的部分明确说「未验证」。

## 5. 本地存储一律加密

凡写入本地磁盘的数据（配置文件、`localStorage`、数据库内容、附件）必须密文存储，展示时解密。

- Rust 侧：新增持久化字段必须经 `crypto::encrypt_str` / `crypto::decrypt_str`（`src-tauri/src/crypto.rs`），不得直接明文落盘。
- 配置：`config.json` 整份加密，信封形如 `v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`（SM4-CBC + HMAC-SM3）；主密钥由 macOS 钥匙串 / Windows DPAPI 托管。
- 数据库：`ws-workbench.db` 表里只留 `id`、`group_id`、`sort_order` 等少量明文列，每行内容序列化后加密进 `payload` 列。
- 前端侧：读写本地键值必须经 `safeStorage` 封装（`src/composables/safeStorage.ts`），不得直接调用 `localStorage`。
- 新增配置读写入口复用 `config.rs` 的加密信封边界，不要在业务代码里各自加解密。
- 附件从上传开始就是密文（WST1：明文头 + SM4-CBC 分帧 + HMAC-SM3 尾），先写 `.tmp` 再原子改名；
  预览 / 下载 / 复制不得在系统临时目录留明文副本。
- 新增本地存储位置必须走 `storage/` 的标准数据仓库与回退链，不要在业务代码里硬拼 `app_data_dir`。

## 6. 密钥、令牌与敏感信息

- 私钥 `~/.tauri/ws-workbench.key`；令牌与密码只放 `env.local`（`*.local` 已被忽略）或 CI secrets。
  `env` 可以提交，但只放空占位或公开默认值。
- 环境变量优先级：命令行 / CI 显式设置 > `env.local` > `env`。
- 密钥、令牌、密码不得出现在代码、命令行参数、日志、提交记录、截图中。VPN 密码走 stdin（`--passwd-on-stdin`），不进进程列表。
- 更新签名密钥所有平台共用同一把；**不要重新生成**——换密钥等于所有已装客户端自动更新失效。
- 发现 `*.key`、`*.sig`、`env.local`、`src-tauri/.config/` 被 Git 跟踪时，先修 `.gitignore` 并从索引移除。

## 7. Windows / macOS 平台代码必须分离

目的不是目录好看，而是**修一个平台时不要顺手改坏另一个平台**。同一功能的两个平台实现必须拆开写，禁止把平台差异堆在共享代码里。

### 目录与边界

- 范本是 `src-tauri/src/vpn/platform/`：
  - `mod.rs`：唯一允许出现平台分派的地方，只做选择和再导出，不写业务；
  - `common.rs`：平台无关的共享逻辑；
  - `macos.rs` / `windows.rs`：各自完整实现同一组入口。
- 平台文件按 `cfg` 整文件编译：macOS 文件里不再写「如果是 Windows 就……」，Windows 文件同理。
- `macos.rs` 与 `windows.rs` **禁止互相引用**；需要共享的能力上提到 `common`，通过参数/返回值传递，不共享可变全局状态。
- 平台文件内部可以再按子功能拆子模块（如 `windows/registry.rs`），但只能同平台内部引用。
- 不支持的目标平台用显式 `compile_error!` 或统一错误返回，不靠「函数不存在」兜底。
- 允许出现 `cfg` 的位置：门面与门面分派（`cleaner/installer.rs`、`cleaner/related.rs`、`commands/cleaner*.rs`、`uninstall_info.rs` 里只影响门面 / 窗口装饰 / 脚本落盘的零散 `cfg`）以及平台文件顶层；`platform.rs` 是平台判定入口，不算违规。
- 共享层（`common.rs` 与功能门面）禁止出现 `target_os` 业务分支；`grep -n "cfg(target_os"` 只应出现在门面和平台文件顶层。

### 什么可以共享

判断标准：把系统调用、路径、权限、注册表/plist、进程、安装/卸载机制拿掉之后，这段逻辑是否还成立？

- 可以共享：数据模型与序列化契约、字符串/版本解析、排序过滤去重、阈值与规则表、错误文案组装、纯计算。
- 必须分平台：`NSRunningApplication` / `winreg` / PowerShell / osascript、废纸篓与 UAC/授权、安装包与注册表布局、平台路径规范（`/Applications` 与 `C:\`）。
- 依赖平台语义但本身不碰系统 API 的纯逻辑（例如 UninstallString 解析），放在对应平台文件里，用 `#[cfg(any(target_os = "windows", test))]` 让另一平台的测试机也能跑单测。

### 防止两个平台互相改坏

- 平台文件只在对应 target 编译；CI 的 `test-macos` / `test-windows` 两个 job 是硬护栏，新增平台代码必须两边都绿。
- 只改一个平台的文件时不应触碰另一个平台文件；改共享层必须同时跑两个平台的测试。
- 跨平台行为差异写在注释或设计文档里，不要在代码里用「如果是 Windows 再……」打补丁。
- 已拆完的范本：`cleaner/installer_macos/`、`cleaner/installer_windows/`、`cleaner/running/`、`cleaner/trash_guard/`、`cleaner/sleep/`、`hosts/file/`、`clipboard/`、`storage/`、`packer/svn_platform/`。
- 存量未拆清单见 [docs/development.md](docs/development.md)「平台代码分离」（`vpn/split.rs`、`vpn/route.rs`、`vpn/fulltunnel.rs`、`vpn/elevate.rs`、`vpn/worker.rs`、`vpn/credential.rs`、`vpn/script.rs`、`commands/openwith.rs`、`crypto.rs`、`apps/finder.rs`、`process.rs` / `winps.rs`、`lib.rs` 的窗口/托盘/菜单、`cleaner/popup.rs`）。**下一次修改这些模块时必须按本规范拆分**，禁止在共享文件里继续新增平台分支。

### 当前平台验证不了的代码：TODO 标记

- 只用于本机确实无法验证的部分（Windows 实机注册表/UAC、macOS 实机授权框、安装包等），不能用来掩盖没写测试。
- 标记格式固定，便于 grep：
  `// TODO(platform-verify:windows): <待验证行为>；验证步骤：<命令/手动步骤>；期望：<结果>`
  目标平台写在冒号后（`windows` / `macos`），同一行写全待验证行为、可照做的步骤和预期结果；涉及提权、删除、卸载的，注明不可在什么环境下验证。
- 换到该平台的人：先按 TODO 完成验证（补测试或记录实机结果），确认通过后**删除该 TODO**；验证不通过就改代码，不得只删标记。
- 发版前清点：`grep -rn "TODO(platform-verify" src-tauri/src src-tauri/nsis src-tauri/resources`
  （NSIS 钩子和 PowerShell 资源里的 TODO 不在 Rust 目录，别漏）。
- 提权/删除/卸载等高风险路径不得带着未验证 TODO 发版；确实还在验证期的，在 [docs/platform.md](docs/platform.md) 登记验收计划。
- CI 双平台 job 只覆盖编译和单测，实机行为仍要人工验收；TODO 的删除以实机结果为准。

## 8. 系统调用、外部进程与脚本

- **不解析本地化文本输出**（`netsh`、`route print`、`sc` 等本地化后会解析失败）。Windows 用 PowerShell 的结构化命令
  （`Get-NetRoute` / `New-NetRoute` / `Get-NetAdapter` / `Get-DnsClientNrptRule` 等）取 JSON，macOS 用 `route` / `ifconfig` / `netstat`。
- 凡输出要被解析的子进程，一律显式钉死 C locale：`process::apply_c_locale` / worker 入口的 `pin_child_locale`（`LC_ALL=C` 并删掉优先级更高的 `LANGUAGE`）。版本探测、`Login failed` / `CSTP connected` 关键字匹配都算。
  用户可见的程序（编辑器、终端）不要带这个环境，保留用户自己的语言设置。
- 生成给 shell / cscript / PowerShell 执行的脚本必须是纯 ASCII（vpnc-script 在 Windows 侧按 ANSI 代码页解码，混入非 ASCII 字节会直接编译失败）。
- PowerShell 传带引号的 XML 或复杂字符串用 `-EncodedCommand`（UTF-16LE + base64），不要靠命令行转义。
- 外部命令失败时把原始输出/退出码落进对应模块日志（例如 `create-task.log`），只报「失败」不算可诊断。
- 提权进程与 GUI 进程之间没有管道可用，IPC 一律走会话目录文件（`request.json` / `status.json` / `stop.flag` / `main.pid` 等），不要临时加 socket / pipe。
- 调外部二进制只用安装包内置或系统自带且来源可信的：VPN 只认内置 openconnect，不读 PATH / 注册表 / Homebrew；引入新的外部依赖要说明来源与校验方式。

## 9. 安全边界：删除、卸载、提权

- 主进程保持普通权限，建网卡 / 改路由 / 写系统文件交给独立提权 worker；不要让 GUI 常驻管理员。
- 普通删除只进废纸篓；需要 root 的删除走一次合并提权批处理，删除前明确提示「不进废纸篓、不可恢复」。
- 所有删除路径过白名单校验：`/`、主目录、`/System`、`/usr`、`C:\Windows` 等硬拒绝；卸载别人之前先只读检测进程，运行中要二次确认并等进程真正退出。
- 不能卸载 / 删除自己；系统应用（SIP、`com.apple.*`）不提供卸载。
- 新增「删除 / 卸载 / 安装包钩子」逻辑时同步补平台冒烟或 `TODO(platform-verify:…)`，按第 12 节验收。
- 安装包与数据目录位置以 `uninstall-info.txt` 为准（纯路径、不含密钥），自定义附件目录只提示、不自动删。

## 10. 前端：组件、状态与界面

### 组件与分层

- 通用界面一律用 `ant-design-vue` 组件与设计 token，不自造同类控件；编辑器是例外（笔记 Vditor、Hosts Monaco）。
- 一个业务一个 composable 或组件；跨业务组合只在 `App.vue` 里做。
- 组件里不直接 `invoke`：走后端命令的类型化封装 `src/api/<域>.ts`。
- 通用能力只放 `src/utils`、`src/api`、`src-tauri/src/util`；业务代码不重复实现路径、时间、对话框、命令调用。
- 样式里的 px 一律用偶数（`11px` 写 `12px`，`13px` 写 `14px`）：非 100% 缩放时奇数尺寸会落在半像素上发虚。`1px` 描边和分隔线是刻意留的发丝线，例外。
- 运行时资源不打 CDN：Vditor / Monaco / 图标 / 字体都从本地依赖或 `src/assets` 打包（Vditor 资源由 `vite.config.ts` 的 `vditorOfflineAssets` 输出）。新增资源先确认离线可用。

### 界面

- 现代开发者工具、极简、Linear.app / Vercel 风：左侧图标栏导航、卡片布局、4/6/12 圆角、1px 边框、不靠阴影堆层次；强调色只用在主按钮；高对比字体、充足留白。
- 完整规范（颜色令牌表、字号、间距、组件约定、亮暗主题、评审清单）见 [docs/ui.md](docs/ui.md)。下面几条是硬规则，评审会逐条查：
  - 颜色只用 `src/assets/styles/main.css` 的令牌（亮色 `:root`、暗色 `html[data-theme='dark']`），组件里不写字面色值 / `rgba()`；不用渐变、毛玻璃、装饰性光晕；技术栈 / VCS / 文件类型标签用中性色。
  - 圆角只有 4 / 6 / 12；间距落 4px 网格；阴影只有 `--shadow-card` / `--shadow-popover` 两级。
  - IP / 端口 / MAC / 路径 / 版本号 / 日志一律等宽（`.mono`）。
  - 每个页面最多一个主按钮；危险操作白底红字；选中态是浅灰底 + 加粗（必要时 2px 品牌蓝竖线），不做浮动/发光。
  - 主题三档（跟随系统 / 浅色 / 深色，默认跟随系统）由 `src/composables/useTheme.ts` 管，antd 主题在 `src/theme/antdTheme.ts`；新增界面必须亮色、暗色都看过。
  - 例外只有两处：卸载窗口自带 `--un-*` 令牌且保持亮色；SVG 图标内部的插画色不受上面限制。
- 用布局解释层级，不堆辅助标签和说明文字。
- **默认不加常驻的小字解释**：标题、卡片、表单项下面那行灰色小字，如果只是把字段名换个说法再说一遍（例：「本机设置 — 设备身份、接收位置与网络信息」），一律不加；字段名和值本身已经说明的事，不要再写一遍。
- 确实需要说明的只有三类：不写用户就会操作错（危险操作后果、不可逆提示）、字段含义无法从名字看出来、异步状态需要解释原因。这类说明优先放 `title` / tooltip / placeholder / 空态文案，不做成常驻小字。
- 评审界面改动时默认问一句「这行小字能不能删」，删掉不影响理解就删。
- 失败不静默：打开失败、更新失败、卸载/删除失败都要有可见提示，并把原因写进对应模块日志。
- 通知只在状态真的变化时发（连接生效、断开生效、规则生效、模式生效），走系统通知，不在界面另弹卡片。
- 日志按模块就近展示（VPN 在代理页抽屉、打包在项目页底部），不做全局日志页。

### 状态同步（评审会查）

**状态用订阅，动作用命令**。判断标准是「这份状态是否需要长期、跨组件地跟着后端变化」，不是「能不能用事件」。

| 场景 | 做法 |
| --- | --- |
| 同一状态被多个窗口/组件长期展示（未读数、VPN 状态、传输进度、在线列表、托盘角标） | 订阅后端事件 |
| 状态由后台线程/另一个窗口改变，前端无法从本地操作推导 | 订阅后端事件 |
| 下拉框、详情页首屏、表单草稿等只用一次的数据 | 调 command |
| 模态框、选中项、校验提示等纯前端临时状态 | 组件内 `ref` |
| 能由已有状态推导的值（如未读总数） | 只维护一份源状态，其余 computed，不要为派生值发事件 |
| 需要返回值/错误/超时/重试的动作（发送、删除、保存） | command；事件只做「发生了变化」的通知，不当 RPC |
| 高频或大 payload | 事件只带 id / 计数 / 进度，正文让前端按需拉 |

实现要求：

- 单一事实源在后端，一个状态只维护一份；禁止各组件遍历列表自己求和。
- 新增事件命名用 `域://事件`（如 `lan-share://unread`），与命令名一起写进设计文档；存量 `pack-progress` / `attachment-progress` 是历史命名，别照抄。
- payload 用 serde 结构、前后端字段名对齐，并加序列化单测锁契约（参考 `UnreadPayload` 的 `{"total":n}` 断言）。
- 统一出口推送，已算好的值顺手复用（emit 未读时把 total 传给托盘，别再查一次库）。
- 前端封装在 composable：`onMounted` 先订阅再拉初始值，用 revision 丢弃过期初始值，`onUnmounted` 取消订阅并处理 disposed 竞态；不要在组件里裸 `listen` 不清理。
- 每个 WebView 各自订阅（主窗口、托盘面板、剪贴板面板独立），不要试图跨窗口共享 JS 单例。
- 幂等与降噪：同一状态重复推送要幂等（覆盖赋值、托盘按状态去重）；高频事件做防抖/节流。
- 失败降级：订阅失败只打日志、不阻塞界面；首屏初始值用 command 兜底，事件恢复后自动补齐。
- 先查再建：新增事件前先看有没有现成的域事件能扩展；事件越多，契约、清理和排查成本越高。
- 评审新增订阅先回答四个问题：谁消费？需要跨窗口吗？订阅生命周期谁负责？payload 是否最小？答不上来就先用 command。

## 11. Rust：分层与事件契约

- 命令按业务域放 `commands/<域>.rs`，新增命令先归域再导出，避免所有入口堆在一个文件里。
- 一个业务的范围只动它自己的模块；改通用层要同时跑 `pnpm build` 与 `cargo test --lib`。
- 纯逻辑（规则解析、过滤、去重、路径校验、脚本生成）抽成可单测的函数，放在对应模块的 `#[cfg(test)]` 里，不要依赖真实系统。
- 错误类型要带上下文（路径、命令、退出码、stdout/stderr 摘要），日志里能定位问题；不要把错误吞成一句「操作失败」。
- 新增持久化字段要兼容老数据（迁移或默认值），见第 5 节。

## 12. 测试与实机验收

- 单测只覆盖纯逻辑，放对应模块 `#[cfg(test)]`；CI 只跑 `pnpm build` + `cargo test --lib`（双平台）。
- 真机链路用 `#[ignore]` 冒烟，不进 CI：
  - macOS：`CLEANER_SMOKE=1 cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored cleaner::macos_smoke`
  - Windows：`CLEANER_SMOKE=1 cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored --test-threads=1 cleaner::windows_smoke`
- 冒烟写入用唯一名字，结束（含断言失败）自动清理，绝不删用户真实文件。
- 需要点 UAC 的路径（提权批量删除、HKLM 键删除、真实 MSI 卸载）不进自动化，按 `TODO(platform-verify:windows)` 人工验收。
- 新增系统能力要在 `src-tauri/src/cleaner/{macos,windows}_smoke.rs` 补对称用例（至少一边有真机结果记录）。
- 实机结果写进 [docs/platform.md](docs/platform.md)；TODO 的删除以实机结果为准，验证不通过就改代码，不得只删标记。

## 13. 开发版与安装版共存

开发构建叫 `ws-workbench-dev`，发布版叫 `ws-workbench`，同机可同时运行。

- 单实例互斥、WebView2 用户数据目录按构建分开；数据库与 `config.json` 共用（同 identifier），加字段要兼容两边。
- 开发版不跑剪贴板后台、不改快捷键、不检查更新、不提供安装入口。新增「常驻后台 / 全局快捷键 / 自更新」能力时必须按构建区分，避免开发版抢发布版的状态。
- 按进程名找本应用一律用通配 `ws-workbench*`（dev 版也算），脚本里别写死 `ws-workbench.exe`。
- 调试「名字 / 图标没变」先确认旧进程退干净（`pgrep -fl ws-workbench`）；单实例会把新进程顶掉，看起来就像重启无效。

## 14. 构建与发布

- **版本唯一来源**是 `src-tauri/tauri.conf.json` 的 `version`（正式版 `X.Y.Z`）。不手工打 tag、不手改 `updates/latest.json`；
  正式版发布 = push `main`，CI 的 preflight 按版本判重（已发过就跳过），重跑用 Actions `workflow_dispatch` + `force`。
- **beta 流程**：功能合到 `beta`；发布顺序固定「先推远程 `beta` → 再本地/各平台构建 → 最后上传 Gitee」。
  每次推到 beta 的代码都必须对应一个可安装的 beta 发行版，不允许「只有代码没有包」；构建失败补发用同一个 `--version`，不能重新分配 N。
  每发完一个正式版，beta 的 base 版本要抬到正式版之上。
- beta 包只发 Gitee 同名发行版与 `updates/beta/latest.json`，不发 GitHub、不触发 GitHub 构建。
- **产物必须齐**：macOS 出两个 dmg（`aarch64` + `x86_64`，更新载荷是 `.app.tar.gz`）；Windows 出 NSIS `.exe`（更新载荷）+ `.msi`（手工安装）。
- MSI 依赖 WiX，**必须在 Windows 主机构建**，不能交叉编译；WiX 不接受预发布版本号，脚本会单独注入纯数字 ProductVersion。
- 更新签名密钥所有平台共用一把；`TAURI_SIGNING_PRIVATE_KEY` / `_PASSWORD` 走 CI secrets 或 `env.local`，见第 6 节。
- Gitee 镜像：公开仓库（默认 `feng_wenshan/ws-workbench`），清单分支名必须和客户端端点一致（`GITEE_REPO` / `GITEE_BRANCH` 只能成对改）。
  Gitee 失败不影响 GitHub 发版，清单里该平台保留 GitHub 地址。
- 清单提交带 `[skip ci]`；两个构建 job 并行写清单靠「快进合并 + 推送被拒撤销重试」，不要改成直接覆盖。
- 本地试装用 `pnpm package:local`，输出目录不能在仓库内（会被打进下一次构建）。
- 发版前：`pnpm build` 与 `cd src-tauri && cargo test` 绿、工作区干净、分支正确、高风险 TODO 已清。
- macOS 最低系统版本 11.0（`LSMinimumSystemVersion`）；改内置 openconnect 构建要同步 `tauri.macos.conf.json` 与 `resources/openconnect-macos/README.md`。

## 15. 提交前检查

- [ ] 改动路径已按第 1 节重启后手工验证（不是只看编译通过）
- [ ] `pnpm build` 绿；动 Rust 时 `cd src-tauri && cargo test --lib` 绿
- [ ] 动共享层/平台文件：两个平台 CI 绿；本机验不了的用 `TODO(platform-verify:…)` 标清步骤与期望
- [ ] 新增/改动的本地落盘数据已加密（Rust 走 `crypto::encrypt_str`，前端走 `safeStorage`）
- [ ] 没有新增明文密钥/令牌；`env.local`、`*.key`、`*.sig` 未被跟踪
- [ ] 平台差异在平台文件里：`macos.rs` / `windows.rs` 零互引，共享层无 `cfg(target_os` 业务分支
- [ ] 前端新状态先想「订阅还是命令」；事件有单一出口、payload 最小、有契约单测
- [ ] 样式 px 全偶数（1px 除外）；用 antd 组件；没有新增常驻辅助说明文字（标题 / 卡片下的灰色小字）
- [ ] 界面符合 [docs/ui.md](docs/ui.md)：无字面色值 / 渐变 / 毛玻璃，圆角 4/6/12，间距 4px 网格，等宽字体已用，亮色与暗色都自检过
- [ ] 失败路径有可见提示 + 日志；通知只发真实状态变化
- [ ] README / docs / 本文件已同步；引用的文件路径与命名仍然成立
- [ ] 提交信息符合 `type(scope): 中文描述`
- [ ] 发版相关：版本号只改 `src-tauri/tauri.conf.json`，tag 与清单交给脚本
- [ ] 高风险路径（提权 / 删除 / 卸载 / 安装包）没有带着未验证 TODO 准备发版

## 16. 现状缺口（待决，不是规则）

- 仓库没有 ESLint / Prettier / `.editorconfig` / `rustfmt.toml`，CI 也没跑 `cargo fmt` / `clippy`：
  格式目前只靠 `vue-tsc --noEmit` 和 review 守。要加工具时同步改本文件与 CI。
- 本文件名固定为 `AGENTS.md`（这是工具自动加载的名字），不再另留 `AGENT.md` 副本，避免两套规则各改各的。
- `docs/architecture.md` 的目录结构已与正文对齐（`vpn/platform/macos.rs` 改为「只认内置副本」）。往后再发现文档与代码不一致，
  顺手修文档，别只在本文件记一笔。
