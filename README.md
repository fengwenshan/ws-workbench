# ws-workbench

基于 Tauri 2 的桌面工作台，把交付日常收进一个窗口：分组/项目管理、Markdown 笔记与加密附件、Layui 项目打包、VPN 规则分流、运行环境自检。主页按「分组 → 项目」组织内容，代理 / 设置 / 日志从托盘进入；关闭窗口不退出应用，常驻系统托盘。

## 功能

### 工作区

- 两级结构：分组 → 项目。默认分组不可删除，但组内项目可删；分组与项目都支持拖拽排序，项目可以在分组之间拖动
- 分组和项目各自带一份 Markdown 笔记、一组加密附件（上传目录统一在「设置 → 附件上传」里配）
- 项目配置：源目录、输出目录。目录旁边是「打开方式」分体按钮：左半边固定用默认应用打开（访达 / 资源管理器），右半边是「用某个应用打开」的动作列表，点哪一项就立刻用那个应用打开当前目录，不切换、也不记住上次选了谁，菜单里给默认项标「（默认）」。macOS 列访达、VS Code、Trae、Cursor、Xcode、IntelliJ IDEA、PyCharm、WebStorm、GoLand、iTerm、终端，图标从各应用的 `.icns` 里取；Windows 列资源管理器、VS Code、IntelliJ IDEA、PyCharm、WebStorm、GoLand、RustRover、Cursor、Trae、终端、PowerShell，图标用 `PrivateExtractIcons` 从 exe 里取 64px PNG——终端取的是 Windows Terminal 的 `WindowsTerminal.exe`（`WindowsApps\wt.exe` 只是 0 字节别名），`.cmd` 启动器（如 Trae 的 `bin\trae.cmd`）会先解析出它真正启动的 exe 再取图标，否则拿到的是脚本或快捷方式的通用图标。打开失败会弹提示而不是静默无反应；目录不存在时自动退到最近存在的上级目录。Windows 上「打开」统一走 `ShellExecuteW`（也就是资源管理器里双击那条路），不再用 `CreateProcess`——后者必须带 `CREATE_NO_WINDOW` 才不闪黑框，控制台程序因此拿不到窗口（终端、PowerShell 点了没反应就是这个原因）。VS Code / Trae / Cursor 这类 Electron 编辑器改走它们自带的 `bin\code.cmd`、`bin\trae.cmd`（直接给 `.exe` 传目录在部分机器上什么都不发生，由 CLI 负责单实例转发和「在哪个窗口打开」），图标仍然从真正的 exe 上取
- 新建项目时识别类型（Layui / Vue2 / Vue3 / React / SpringBoot / Spring / Struts2 / Java）和版本控制远端（Git / SVN），并做源目录校验；项目名称下方的 Git / SVN 标签和仓库地址都可以点击复制

### 笔记与附件

笔记用 `Vditor` 的即时渲染模式（IR）编辑，单栏所见即所得、边写边排版，交互参考 Typora：标题 / 列表 / 引用 / 表格 / 代码块即时呈现，工具栏可切换到源码模式。内容 500 毫秒防抖后回写配置；切换实体或组件卸载前会把未落盘的修改补交一次。

Vditor 的运行时资源（lute / highlight.js / i18n / 图标 / 内容主题）不走 CDN：`vite.config.ts` 里的 `vditorOfflineAssets` 插件在 dev 时从 `node_modules/vditor/dist` 直出 `/vditor/dist/*`，构建时拷进 `dist/vditor/dist/*` 随安装包一起发布。

笔记页右侧是「大纲 / 附件」栏（默认显示大纲）：

- **附件**自动列出正文里引用的加密图标 / 图片（`![]()` 与 `<img>` 两种写法都认），显示缩略图、名称、大小，可插入到光标、预览大图、复制 Markdown、用系统程序打开、导出、重命名（改 alt）和删除（确认后同时移除正文引用与密文 blob）。附件栏不提供手动上传：资源只在笔记正文引用后出现，新增图片请用编辑器工具栏 / 粘贴 / 拖入正文
- 资源清单从正文的 `wst-img` 地址解析（地址里带着目录 / id / 扩展名），不额外存元数据，老笔记打开即生效；正文里没有的资源不显示
- 大小通过后端 `stat_attachment` 读取 WST1 明文头（magic / 版本 / 原始大小），不解密内容；密文文件丢失时卡片标记「文件缺失」，仍可从正文移除引用
- **大纲**解析 Markdown 标题，点击滚动到编辑器对应位置；窗口变窄时侧栏自动收起，右下角悬浮按钮可重新展开

附件从上传开始就是密文：

- 选文件，或把文件拖到「上传」页任意位置（整个内容区都是投放区）；上传时加密成 `<upload_dir>/<uuid>.wst`，先写 `.tmp` 再原子改名，磁盘上不会出现明文副本
- 容器格式 `WST1`：明文头（magic / 版本 / 原始大小）+ 若干 `SM4-CBC(PKCS7)` 帧 + HMAC-SM3 尾。分帧读写，内存占用近似常数，大文件不会整份读进内存
- 上传目录：设置页全局默认目录 → App 私有 `<app_data_dir>/attachments`（未设置时按需创建）
- 打开 = 解密到临时文件后交给系统默认程序；导出 = 解密到指定路径；重命名只改元信息；删除会连同密文 blob 一起删
- 上传与解密进度通过 `attachment-progress` 事件推给界面

笔记里插入的图片（工具栏选图、粘贴剪贴板图片或拖入）同样走加密上传，正文只保存 `wst-img://`（macOS/Linux）或 `http://wst-img.localhost`（Windows）地址，编辑器渲染时由自定义协议按需解密返回，不落明文图片文件；这些图片不占用「上传」页的附件列表，而是显示在笔记右侧的「附件」栏里。

### 项目打包

先识别项目类型（Layui / Vue2 / Vue3 / React / SpringBoot 等）并读出 Git 或 SVN 的远端地址，再决定能不能打包。

SVN 地址取的是「仓库根 + 工作副本在仓库内的路径」（如 `https://host/svn/NewApp/2025/team/oa`），不是只有仓库根的半截地址。顺序是：`svn` 命令行（含 TortoiseSVN / SlikSVN / Homebrew 的常见安装位置）→ 直接读 `.svn/wc.db` 的 `REPOSITORY` / `NODES` 表 → 二进制扫描 `wc.db` → SVN 1.6 的 `.svn/entries`。所以没装 svn 命令行的 Windows 机器也能拿到完整地址，源目录是工作副本的子目录时同样能补全。

打包只对 Layui 项目开放：

```ts
// src/components/ProjectConfig.vue
const canPack = computed(() => projectType.value === 'layui')
```

其他类型不渲染打包按钮；识别为未知类型时提示「未识别到 Layui 项目特征，打包功能不可用。请确认源目录选择正确。」

产出方式有两种：

- **开始打包**：逐个复制到输出目录，复制前会清空输出目录
- **打包 ZIP**：流式 Deflate 直接写 zip，不落中间产物；zip 内会多包一层输出目录名，条目名按 UTF-8 写入并置位 bit 11

打包用**白名单（layui 项目打包规则）**：只有命中规则的文件才进包，规则统一放在**设置页 →「layui 项目打包规则」**，项目级不再单独配置。

```
*.html *.htm *.css *.js *.mjs *.json
*.png *.jpg *.jpeg *.gif *.svg *.webp *.avif *.ico *.bmp
*.woff *.woff2 *.ttf *.eot *.otf
*.mp4 *.webm *.mp3 *.wav *.ogg
*.txt *.xml
*.thm *.tjs *.dotm     # layui 模板 / 模板脚本 / WPS 控件模板
*.doc *.docx           # WebOffice 新建正文的空白模板（旧版 / 新版 Word）
```

- 只列运行时真的会被请求的类型；`scss`/`ts`/`vue`/`map`/`md`、构建配置（`package.json`、`vite.config.*`）、设计源文件、压缩包这些开发期产物都不在里面
- layui 特有：`.thm`（表单/列表模板，`rsViewRender.js` 按 `APP_ROOT + '/xmodules/.../**.thm'` 拉取）、`.tjs`（模板脚本）、`.dotm`（WPS 控件的 `Normal.dotm`）都是运行时资源，默认收
- `.doc`/`.docx` 只收运行时真会 GET 的那类模板（`rsWebOffice.js` 用 `$.ajax` 拉 `weboffice/newDocument.doc(x)` 当新建正文模板，旧版 Word 模板是 `.doc`）；上传控件 accept 里出现的 `.pdf`/`.wps` 等字符串不算引用，工程里没有对应资源的仍不收
- **`.` 或 `_` 开头的路径段一律跳过**（任意层级，文件或目录都算）：`.svn`/`.git`/`.idea`/`.vscode`、`.DS_Store`、`_draft.html`、`partials/_header.html` 都由这一条硬规则兜住，不占规则名额
- 匹配语义：不带 `/` 的规则按**文件名**匹配、任意层级都算（`*.png`）；带 `/` 的按**相对源目录的路径**匹配（`assets/**`、`js/app.js`）
- 规则清空 = 视为未配置：除 `.`/`_` 开头的条目外全部打包（避免误删规则后打出空包）
- 路径里含空格的文件始终跳过

默认规则定义在 `src-tauri/src/models.rs` 的 `DEFAULT_INCLUDE`，带版本号（当前 `DEFAULT_INCLUDE_VERSION = 4`）：老配置（没有 include 字段）加载时自动补上默认集，之后用户自己删掉的规则不会被重新塞回去。「恢复默认」按钮调用 `get_default_include_rules` 取同一份清单。

遍历由 `ignore::WalkBuilder` 完成，标准过滤全部关闭、不跟随符号链接；`.`/`_` 开头的条目在 `filter_entry` 里直接剪掉，包含规则用 `globset` 在遍历结果上过滤（文件名匹配 + 路径匹配两个集合）。相对路径中任一段含空格的文件会被跳过。输出目录与 zip 自身通过 `extra_excludes` 整棵子树剪掉，避免上一次的产物被再打一遍。

打包过程中通过 `pack-progress` 事件推送进度，阶段依次为 `scanning` → `cleaning` → `copying` / `zipping` → `done`。

### 代理（VPN）

底层只调 openconnect，不依赖 Cisco AnyConnect 客户端或 OpenConnect GUI。

连接器来源按平台区分：

- **Windows 只认安装包内置的 openconnect**（`src-tauri/resources/apps/openconnect/openconnect.exe` + 同目录 `wintun.dll`）。不扫描注册表、常见安装目录或 PATH，也不提供「导入外部 openconnect」入口，避免把来源不明的二进制拉进提权进程。内置副本缺失时直接报错并提示重装。
- **macOS** 按「手动指定 > 已导入副本 > 内置资源 > Homebrew / 常见目录 > PATH」查找。

判定可用性的方式是真正执行一次 `openconnect --version` 并解析版本号，而不是看文件存在与否 —— 解压后缺依赖 DLL 是很常见的情况，光看路径会把跑不起来的二进制当成可用。

支持的协议：`anyconnect`、`nc`、`gp`、`pulse`、`f5`、`fortinet`、`array`。

#### 规则分流

分流的配置单位是**规则集**，每个 VPN 配置绑定其中一个。规则语义与 Clash 一致：

- 规则是**有序列表**，自上而下**首个命中生效**，全部不命中时由 `default_policy` 兜底（对应 Clash 的 `MATCH`）
- 规则类型：`DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`PROCESS-NAME`、`DST-PORT`、`IP-CIDR`；出口只有 `DIRECT`（直连）与 `VPN` 两种
- `IP-CIDR` 规则可勾 `no_resolve`，命中网段后不再为该连接做域名解析
- 保存前会校验规则语法，可以预览一条规则对指定「域名 / 进程 / 端口」的判定结果，也能检查同一规则集里的网段重叠

两个平台实现方式不同：

- **macOS：TUN 透明接管。** worker 另建一个捕获 utun，把 `0.0.0.0/1` 与 `128.0.0.0/1` 两条半默认路由指向它。抓到的 TCP/UDP 包交给 smoltcp 用户态协议栈完成握手与收发，连接被终结后再按规则选出口：`DIRECT` 用 `IP_BOUND_IF` 把出站 socket 钉在物理网卡，`VPN` 钉在隧道网卡并补一条 `/32` 流路由，随后双向搬运字节。DNS 由本地 fake-ip 服务应答：A 记录分配池内假 IP、AAAA 返回空答；连接到达后用假 IP 反查域名参与规则匹配，命中 `fake_ip_filter` 的域名不伪造、走真实解析。
- **Windows：TUN 透明接管。** 用 Wintun 另建捕获网卡，同样把 `0.0.0.0/1` 与 `128.0.0.0/1` 两条半默认路由指向它，后续 TCP/UDP/DNS 处理与 macOS 共用同一套 smoltcp 引擎。出口用 `IP_UNICAST_IF` 在 socket 上钉物理网卡或隧道网卡；VPN 流另外补一条 `/32` 路由指向隧道（同一真实 IP 的多条流用引用计数共用，最后一条关闭时才删除），避免隧道网卡没有目标网段路由时连接失败。服务器的 `/32` 防环路由通过 `route.exe` 走物理网关，且必须先于两条半默认路由安装。DNS 同样是本地 fake-ip 应答，连接到达后反查域名再走规则。

规则改动不会热生效，需要断开重连。

#### 权限与进程拆分

建虚拟网卡、改路由表都要管理员权限，但让 GUI 常驻管理员既危险又每次启动弹窗，因此拆成两个进程：

- 主进程（普通权限）读配置、探测连接器、把请求写进会话目录、拉起 worker、轮询状态文件
- worker（管理员权限）独立进程，负责建隧道、按规则下发分流、守护、退出时回滚

提权方式按平台区分：

- Windows：`schtasks /Create /RL HIGHEST /F` 注册计划任务 `WsWorkbenchVpnWorker`，之后免 UAC；注册失败时回退到 `Start-Process -Verb RunAs`，弹一次系统授权框
- macOS：`osascript ... with administrator privileges`

两个进程之间没有管道可用（提权进程拿不到父进程的句柄），IPC 全部靠会话目录里的文件。

#### 路由与脚本

下发路由不解析 `netsh` / `route print` 的文本输出，本地化后那套输出会解析失败。Windows 侧用 JScript（cscript）执行脚本配合 PowerShell 的 `Get-NetRoute` / `Get-NetIPAddress` / `Get-NetAdapter` 拿 JSON；macOS 侧用 `route` / `ifconfig` / `netstat`。

生成的 vpnc-script 必须是纯 ASCII：Windows 那份由 cscript 按 ANSI 代码页解码，混入非 ASCII 字节会直接编译失败。脚本里用 `WS-WORKBENCH` 作为标记行，清理时据此定位自己写入的条目。

Windows 上的虚拟网卡由 Wintun 驱动提供（`src-tauri/resources/apps/openconnect/wintun.dll`），macOS 用系统自带的 utun。

#### 学习模式

只拿到域名、拿不到网段时用它反推：worker 每 15 秒采样一次 `Get-NetTCPConnection -State Established`，记录实际连通的远端地址，再聚合成候选网段交给用户判断。

- 仅 Windows 可用（macOS 侧采样直接返回空），采样连续失败 3 次后自动停用
- 按 `/24` 聚合；同一个 `/16` 内观察到至少 4 个 `/24` 才升级为 `/16`，同一个 `/8` 内观察到至少 4 个 `/16` 才升级为 `/8`
- 单次上限：2000 个主机、每主机 8 个端口、8 个时间样本、4 个进程
- **只观察、只建议**：候选网段要点「采纳」才会写进规则表，程序不会自己改规则

摘要字段写进 `status.json`（前端每 600 毫秒轮询，不能塞几十 KB 的明细），完整数据单独落 `learn.json`，只在展开明细时按需读取。清空走标志文件 `learn.clear`，worker 重置内存中的报告后删掉这个文件，否则下一轮 15 秒采样会把内容原样写回。

#### 凭据

密码不在命令行里传（进程列表可见），走 stdin 交给 openconnect 的 `--passwd-on-stdin`。落盘前加密：

- Windows：DPAPI，密文形如 `dpapi:<hex>`，解密时带 `CRYPTPROTECT_UI_FORBIDDEN`
- macOS：钥匙串，密文形如 `keychain:<key>`，服务名 `ws-workbench-vpn-<key>`

#### 安装引导

openconnect 探测失败时，代理页给出按平台区分的安装引导：macOS/Linux 给可复制的安装命令加官方下载地址；Windows 只说明内置副本缺失并提示覆盖安装，不引导下载外部二进制。

### 系统清理

左侧图标栏第四个工具（`⌘/Ctrl+4`），两个页签：

- **软件卸载**：Windows 读注册表 `Uninstall` 键（HKLM 64/32 位、HKCU），macOS 扫 `/Applications` 与 `~/Applications`；支持按名称/发布者/位置搜索，按名称、大小、安装日期排序（顺序记在本地）。卸载执行 `UninstallString`（MSI 的 `/I` 自动换成 `/X`，需要时一次 UAC），macOS 直接把 `.app` 移入废纸篓；没有卸载命令的条目提供「强制移除」（Windows 删注册表项，macOS 结束进程后删除）。卸载后按 bundle id / 注册表键精确匹配文件与注册表残留，按「文件系统 / 注册表」分组、带置信度展示，勾选后删除。
- **内存占用**：按应用聚合进程 RSS（Chromium/Electron 的 helper 归到宿主应用），显示总占用率、已用/总量/swap 与应用排行，2 秒轮询（页面不可见时暂停）。支持结束进程（先优雅退出，3 秒后仍在运行再确认强制结束）与「深度释放」（macOS 一次授权跑 `purge`；Windows 清进程工作集 + 系统待机列表，一次 UAC）。释放前后都用真实采样，展示差值，不夸大收益。

安全约束：删除只进废纸篓（需 root 的条目走一次提权批处理）；所有路径过白名单校验，`/`、主目录、`/System`、`/usr`、`C:\Windows` 等硬拒绝；`pkgutil` 安装收据只读展示；系统应用（SIP / `com.apple.*`）不提供卸载。macOS 的废纸篓走 `NSFileManager` API，不会弹「控制 Finder」的自动化授权。

已知限制：Windows 侧只做了交叉编译检查与验收清单，未实机跑卸载/UAC/深度释放；`purge` 只回收可缓存页，收益有限；残留按精确匹配，宁可漏报不误删。

**托盘快捷面板**：左键点菜单栏/任务栏图标弹出柠檬式的无边框小面板（`quick.html`，360×560，失焦或 Esc 自动收起），两个页签：

- **内存**：占用率环 + 已用/总量/swap + 占用排行前 7 个应用，「释放」直接走深度释放（需要时弹一次系统授权），底部可跳主窗口看全部应用与卸载。
- **VPN**：当前状态与配置名、「连接/断开」、规则/全局/直连三种代理模式即切，以及跳转代理页的入口。

面板只做快速查看与动作；卸载、残留清理、规则编辑仍在主窗口。窗口位置按托盘图标位置计算（上方/下方任务栏、多显示器负坐标都夹进屏幕范围），逻辑在 `lib.rs::panel_origin`，有 5 个位置单测。

### 设置

显示运行环境（操作系统、架构、可执行文件后缀、是否需要提权、配置文件位置、各网卡的 MAC 与全部 IP —— 多 IP 逐条列出），以及附件实际落盘位置（可直接打开目录）。

「用编辑器打开项目」与非 Windows 的 VPN 连接器探测共用同一套应用查找逻辑，优先级为专用路径 > 已导入副本 > 内置资源 > 注册表 > PATH；导入副本只对 openconnect 开放。Windows 的 VPN 探测是例外：只认安装包内置副本，不查注册表、安装目录或 PATH。macOS 与 Linux 下分别检查 `/Applications`、Homebrew 前缀以及 PATH 里的 `/usr/bin`、`/usr/local/bin`。

### 日志

应用日志与 VPN 日志分两个来源，可切换、清空。VPN 日志来自会话目录里 worker 与 vpnc-script 的输出。

### 托盘

右键菜单：显示主窗口 / 清理 / 代理 / 设置 / 日志 / 连接 VPN / 断开 VPN / 退出。左键单击图标弹出快捷面板（内存 / VPN 两个页签，见上），菜单只在右键时出现。点「退出」会先请求 worker 收尾再结束进程，否则可能留下没人回收的路由。关闭主窗口只是隐藏，应用继续在托盘里运行。

快捷面板是独立的透明无边框窗口：点面板外部（失焦）或按 Esc 收起；「打开主窗口」会把主窗口唤到前台并切到对应页面。macOS 的透明窗口依赖 Tauri 的 `macos-private-api` feature（`Cargo.toml` + `tauri.conf.json` 的 `macOSPrivateApi`），**直装/DMG 渠道没问题，但用不了 Mac App Store**。

macOS 的应用菜单做了汉化，点 Dock 图标会把窗口重新显示出来。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Tauri 2（`tauri` 开启 `tray-icon`、`image-png`、`macos-private-api`） |
| 前端 | Vue 3.5 + TypeScript 5.6 + Vite 6；ant-design-vue 4 + Vditor 4（组件与图标按需自动引入） |
| 后端 | Rust edition 2021，库名 `ws_workbench_lib` |
| 数据面 | smoltcp 0.11 用户态协议栈（macOS 透明代理） |
| 存储加密 | SM4-CBC + HMAC-SM3；主密钥由 macOS 钥匙串 / Windows DPAPI 托管 |
| 插件 | dialog、shell、store、updater、process |
| 关键 crate | `ignore`、`zip`、`chrono`、`uuid`、`tempfile`、`sm4`、`hmac`、`cbc`、`smoltcp`、`nix` |

## 环境要求

- Node 22 与 pnpm（CI 固定 pnpm 10；`pnpm-lock.yaml` 为 9.0 格式）
- Rust stable 工具链
- 仅在需要 VPN 时：可用的 openconnect。Windows 必须使用 `src-tauri/resources/apps/openconnect/` 里的内置副本（安装包会一起打进去），macOS 建议 `brew install openconnect`
- VPN 连接需要管理员授权，见「权限与进程拆分」；两个平台都通过 TUN 透明接管做规则分流，macOS 用系统 utun，Windows 用内置 `wintun.dll`

## 开发

```bash
pnpm install          # 安装前端依赖
pnpm dev              # 只起前端，http://localhost:8790
pnpm exec tauri dev   # 起完整桌面应用
```

Vite 固定在 8790 端口并开启 `strictPort`，端口被占用会直接失败而不是换端口；`@` 别名指向 `./src`，监听忽略 `src-tauri/`。

注意别用 `pnpm tauri dev`。`package.json` 里有一个名为 `tauri` 的脚本（内容是 `tauri dev`），`pnpm tauri dev` 会被解析成 `pnpm run tauri dev`，最终执行 `tauri dev dev` —— 多出来的 `dev` 会被当成传给 runner 的附加参数，和预期行为不一致。

### 重启

改完代码要重启时用 `scripts/dev-restart.ps1`，它会先结束残留进程再拉起一份：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\dev-restart.ps1
```

脚本按仓库路径匹配进程命令行，只结束本仓库的 `ws-workbench`、vite、tauri 进程，不误伤其他项目；检测到 `openconnect` 仍在运行会直接退出，避免留下无人回收的路由。

脚本不经过 pnpm，因为本机 PATH 里的 pnpm 来自 TRAE 自带的 corepack 0.32.0，它给 pnpm 12 生成的 shim 指向 `bin/pnpm.cjs`，而 pnpm 12 只提供 `bin/pnpm.mjs`，调用必然 `MODULE_NOT_FOUND`。脚本改为直接调用 `node_modules\.bin\tauri.cmd`，并用 `-c` 把 `beforeDevCommand` 覆盖成 `npm run dev`。另装一份能跑通的 pnpm 12.5.1 也不合适：它会把 `packageManagerDependencies` 和 `@pnpm/exe.*` 写进 `pnpm-lock.yaml`，CI 用的 pnpm 10 读不懂这个格式。

脚本文件必须保存为带 BOM 的 UTF-8。Windows PowerShell 5.1 对无 BOM 的 `.ps1` 按 ANSI 解码，中文注释会被解成乱码并把脚本解析坏。

## 构建与发布

```bash
pnpm build                                  # vue-tsc --noEmit + vite build
pnpm exec tauri build                       # 打当前平台安装包
node scripts/release.mjs                    # 构建 + 发布
node scripts/release.mjs --no-publish       # 只构建，产物落在 release/
node scripts/release.mjs --dmg              # macOS 额外生成 dmg
node scripts/release.mjs --target x86_64-pc-windows-msvc   # 交叉编译 Windows
```

发布脚本用到的环境变量：

| 变量 | 用途 |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | 更新签名私钥内容，不设时回退读 `~/.tauri/ws-workbench.key` |
| `GITEE_TOKEN` | Gitee 私人令牌（权限含 projects），创建发行版与上传附件必需 |
| `GITHUB_TOKEN` | 把安装包镜像到 GitHub 发行版，不设则跳过镜像 |
| `TAURI_BUNDLES=app` | 只打 app、跳过 Tauri 的 dmg 打包脚本，受限环境配 `--dmg` 使用 |

发布版本的唯一来源是 git tag：CI 把 `vX.Y.Z` 里的 `X.Y.Z` 通过 `tauri build --config` 注入，不改源文件。发布流程：打 tag → 构建（含更新签名）→ 上传安装包到 Gitee 发行版（同名 tag）→ 合并生成本平台的 `updates/latest.json` 条目并提交回 `main`。安装包会同时镜像到 GitHub 发行版，但清单里的 `url` 始终指向 Gitee，因为已装客户端读的就是这个地址。

```bash
git tag v1.0.16
git push origin v1.0.16   # 只有推 tag 才会触发 GitHub 构建
```

### CI

`.github/workflows/release.yml` 只在推送 `v*` tag 时触发（直接 push `main` 不会构建），macOS 与 Windows 两个 job 并行，都跑在 `macos-14` 上；Windows 侧是交叉编译（`cargo-xwin` + `nsis` + `llvm`）。版本号直接从 tag 解析，因此 tag 必须是 `vX.Y.Z` 形式，否则第一个预检步骤就会失败。

跑之前必须配两个仓库密钥：`TAURI_SIGNING_PRIVATE_KEY` 和 `GITEE_TOKEN`，缺任意一个都会在预检步骤直接退出。

两个 job 会同时改 `updates/latest.json`，脚本用「快进到远端最新 → 以远端为基线合并本平台条目 → 推送被拒就撤销重试」来避免互相覆盖。CI 自己推回 `main` 的清单提交带 `[skip ci]`，且工作流只由 tag 触发，不会自触发。

## 自动更新

更新端点为 `https://gitee.com/feng_wenshan/ws-workbench/raw/main/updates/latest.json`，用 minisign 公钥（指纹 `9CB4E4B1ECEF3353`）校验签名。之所以只走 Gitee：GitHub 的 `raw.githubusercontent.com` 在目标网络下被拦，把地址换过去已装客户端会取不到更新。这个地址依赖远端仓库已同步改名为 `ws-workbench`。

`updates/latest.json` 的结构：

```json
{
  "version": "1.0.14",
  "notes": "ws-workbench v1.0.14",
  "pub_date": "2026-09-17T15:38:15Z",
  "platforms": {
    "windows-x86_64": { "signature": "...", "url": "https://gitee.com/.../ws-workbench-setup.exe" },
    "darwin-aarch64": { "signature": "...", "url": "https://gitee.com/.../ws-workbench.app.tar.gz" }
  }
}
```

> `updates/latest.json` 由发布脚本生成；仓库里这份仍是 v1.0.14 的旧记录，下次发版会被完整重写。

客户端启动时静默校验一次，之后每天 10:00 与 16:00 各校验一次；平时标题栏不显示更新入口，只有发现新版本时才在原位置显示「更新」按钮；用户选择忽略的版本记在 localStorage 的 `ws-workbench:dismissed-update-version` 下，不再重复提示。

## 配置与数据

业务数据（分组、项目、笔记、附件元信息、VPN 配置、规则集）存在应用私有目录的 `ws-workbench.db`（SQLite）中：表里只留 `id`、`group_id`、`sort_order` 等少量明文列，每行内容序列化成 JSON 后用 SM4 信封加密，存进 `payload` 列。`AppConfig` 与前端命令保持不变，加载时从多表组装，保存时与内存快照做 diff、事务内只写变化的行。

应用设置（layui 项目打包规则、上次选中的 VPN 配置、默认上传目录、worker 会话目录）仍存在 `config.json`（`tauri-plugin-store`，键名 `app_config`），**整份加密**：SM4-CBC + 随机 IV + HMAC-SM3，信封形如 `v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`。主密钥（32 字节）首次启动时随机生成，macOS 存系统钥匙串（`ws-workbench-vault` / `ws-workbench`），Windows 用 DPAPI 加密后写进 `<app_data_dir>/.vault_key`。

前端写 localStorage 也一律走 `safeStorage`：值经 `vault_encrypt` 变成密文再落盘。

数据库与设置文件都按以下顺序找可写目录，逐个做真实写入探测：`app_data_dir` → `app_config_dir` → `~/.ws-workbench` → `$HOME/.ws-workbench` → `cwd/.config`（开发兜底）；全部失败则退回进程内存，此时重启会丢数据。

附件元信息随所属分组/项目一起加密保存；密文 blob 就在上传目录里，文件名为 `<uuid>.wst`。备份时复制 `ws-workbench.db`、`config.json` 与上传目录即可。

VPN 会话目录同时被主进程和提权 worker 使用，因此不做路径拼接了事，而是带写入探测再逐级回退（受控文件夹访问、企业策略、受限令牌都可能让 `app_data_dir` 静默拒绝写入）。目录内文件：

| 文件 | 用途 |
| --- | --- |
| `request.json` | 主进程下发的连接请求 |
| `status.json` | worker 上报的状态与学习模式摘要 |
| `learn.json` | 学习模式完整明细 |
| `learn.clear` | 清空学习结果的标志文件 |
| `worker.log` / `script.log` | worker 与 vpnc-script 日志 |
| `stop.flag` | 请求 worker 停止 |
| `script/` | 生成的 vpnc-script |

超时与轮询节奏：连接超时 120 秒，守护周期 15 秒，状态轮询 600 毫秒。worker 退出码 `0` 正常、`2` 无请求、`3` 失败。

## 平台与限制

| 项 | Windows | macOS |
| --- | --- | --- |
| VPN 提权 | 计划任务免 UAC，失败回退 UAC 弹窗 | 系统授权框 |
| 虚拟网卡 | Wintun 驱动 | 系统 utun |
| 分流实现 | TUN 透明接管；域名 / 网段 / 端口规则 + fake-ip DNS 生效 | TUN 透明接管；六类规则 + fake-ip DNS 生效 |
| 学习模式 | 支持 | 不支持 |
| 路由查询 | cscript + PowerShell JSON | `route` / `ifconfig` / `netstat` |

其余已知限制：

- 打包只支持 Layui 项目
- 相对路径含空格的文件会被跳过
- Windows 的进程反查暂未接入 IP Helper，`PROCESS-NAME` 规则在 Windows 上不会命中；域名 / 网段 / 端口规则不受影响
- fake-ip 只在应用重新发起 DNS 查询时生效：连接前已缓存的真实 IP（系统 DNS 缓存、浏览器缓存、DoH）不会命中域名规则，连上后请刷新 DNS 或重启对应应用
- 捕获与分流只处理 IPv4 TCP/UDP；ICMP 和 IPv6 仍走系统默认路由，不参与域名规则
- 规则改动需要断开重连才生效
- 学习模式只给建议，采纳候选网段需要手动点击
- 系统清理的 Windows 运行时行为（注册表枚举、UAC 卸载、工作集/待机列表）只做了交叉编译检查，需按设计文档第 9 节的 Windows 清单实机验收
- macOS 深度释放的 `purge` 只回收可缓存页，收益有限；`~/.Trash` 内容受 TCC 保护，删除结果以「原路径消失 + trash API 成功」为准
- 卸载残留只做精确匹配（bundle id / 注册表键 / 精确应用名），存在非标准目录的数据需要手动清理
- 托盘快捷面板的原生弹出（真实点菜单栏图标）受 macOS 辅助功能/屏幕录制权限限制，自动化测试只覆盖了页面渲染、位置计算与 `cargo check`，首次使用请手动点一下确认弹窗位置与失焦收起
- 快捷面板的透明圆角窗口在 macOS 上依赖 `macos-private-api`，因此不支持 Mac App Store 渠道

## 目录结构

```
src-tauri/src/
  commands/        前端命令入口按业务域拆分
    config.rs      配置读写、存储信息、本地键值加密
    project.rs     打包、校验、项目类型与 VCS 识别
    system.rs      打开目录/终端/编辑器、平台与网络信息
    attachments.rs 附件加解密、上传目录解析、笔记内联图片
    cleaner.rs     系统清理命令（列表/卸载/残留/内存）
    quick.rs       托盘快捷面板：隐藏、打开主窗口并切页
  util/            通用能力层
    fs.rs          目录创建、文本读写、可写性探测
    time.rs        本地 RFC3339 / UTC 毫秒时间戳
  config.rs        配置读写：整份加密、写入目录多级回退、历史字段迁移
  crypto.rs        字符串密文信封（SM4-CBC + HMAC-SM3）与主密钥托管
  filecrypto.rs    附件流式加密容器 WST1
  packer.rs        打包：遍历、包含规则过滤、复制、流式 ZIP
  models.rs        AppConfig / 分组 / 项目 / 附件元信息 / 默认包含规则
  platform.rs      操作系统判定
  netinfo.rs       本机网卡 MAC / IP（Windows 走 PowerShell，macOS/Linux 走 getifaddrs）
  process.rs/winps.rs  进程管理与 PowerShell 调用
  apps/            外部应用定位（finder / registry）
  cleaner/         系统清理：安装列表/卸载/残留扫描/内存占用（平台实现按 cfg 拆文件）
  vpn/
    models.rs      规则、规则集、VPN 配置、状态
    rule_engine.rs Clash 式有序列规则引擎（纯逻辑，可离线单测）
    fakedns.rs     fake-ip 地址池：分配 / 反查 / 过滤 / 回收（纯逻辑）
    dns_srv.rs     fake-ip DNS 应答（A 分配假地址、AAAA 空答）
    dns_client.rs  最小 DNS 客户端：连接阶段把假地址解析成真实 IPv4
    netstack.rs    smoltcp 用户态协议栈，终结 TUN 上抓到的连接
    router.rs      出口选择、fake-ip 真实解析与双向字节搬运（双平台）
    platform.rs    TUN / 路由 / 网卡绑定的平台抽象（macOS IP_BOUND_IF / Windows IP_UNICAST_IF）
    tun_device.rs  捕获网卡创建与读写（macOS utun / Windows Wintun）
    proc_lookup.rs 五元组反查进程名（macOS）
    worker.rs      提权进程，连接生命周期主体
    commands.rs    主进程侧命令与状态轮询
    connector.rs   拉起 openconnect
    route.rs       路由表读写
    script.rs      生成 vpnc-script
    learn.rs       学习模式：采样与网段反推
    credential.rs  密码加密存储
    detect.rs      连接器可用性探测
    elevate.rs     提权启动
    guide.rs       openconnect 安装引导（Windows 只提示重装内置副本）
    session.rs     会话目录 IPC
src/
  components/      ProjectList / GroupDetail / ProjectDetail / ProjectConfig /
                   NoteEditor / AttachmentPanel / PackProgress / ExcludeRules / LogPanel
    vpn/           VpnPanel / VpnProfileForm / VpnRuleTable / VpnRoutePreview /
                   VpnLearnCard / VpnInstallGuide
    settings/      SettingsPanel
    cleaner/       CleanerPanel / InstalledAppsPanel / MemoryPanel / AppIcon
  composables/     useConfig / usePack / useVpn / useCleaner / useApps / useLog / useUpdater /
                   useWorkbenchLayout / useEntitySelection / useProjectForm /
                   useDeleteConfirm / safeStorage
  api/             后端命令的类型化封装（config / project / system / attachments / apps / cleaner）
  utils/           通用小工具（time / dialog / path / error）
  types/           前后端共享的数据结构（含 cleaner）
  quick/           托盘快捷面板（独立入口：quick.html + main.ts + QuickPanel.vue）
scripts/release.mjs  构建、上传、更新清单合并
updates/latest.json  更新清单（由发布脚本生成并提交）
```

分层约定：

- 通用能力只放 `src/utils`、`src/api` 与 `src-tauri/src/util`，业务代码不重复实现路径、时间、对话框、命令调用。
- 前端每个业务一个 composable 或组件（项目、VPN、设置、附件各自独立），跨业务组合只在 `App.vue` 里做。
- Rust 命令按业务域放在 `commands/<域>.rs`，新增命令先归域再导出，避免所有入口堆在一个文件里。
- 改某个业务时只动它自己的模块；通用层变更需同时跑 `pnpm build` 与 `cargo test --lib`。
