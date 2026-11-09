# ws-workbench

基于 Tauri 2 的桌面工作台，把交付日常收进一个窗口：分组/项目管理、Markdown 笔记与加密附件、Layui 项目打包、VPN 规则分流、Hosts 在线编辑、运行环境自检；macOS 下还带一份加密落盘的剪贴板历史（文本 / 图片 / 图文）。主页按「分组 → 项目」组织内容，系统 / 代理 / 设置 / 日志从托盘进入；关闭窗口不退出应用，常驻系统托盘。

## 功能

### 工作区

- 两级结构：分组 → 项目。默认分组不可删除，但组内项目可删；分组与项目都支持拖拽排序，项目可以在分组之间拖动
- 分组和项目各自带一份 Markdown 笔记、一组加密附件（上传目录统一在「设置 → 附件上传」里配）
- 项目配置：源目录、输出目录。目录旁边是「打开方式」分体按钮：左半边固定用默认应用打开（访达 / 资源管理器），右半边是「用某个应用打开」的动作列表，点哪一项就立刻用那个应用打开当前目录，不切换、也不记住上次选了谁，菜单里给默认项标「（默认）」。macOS 列访达、VS Code、Trae、Cursor、Xcode、IntelliJ IDEA、PyCharm、WebStorm、GoLand、iTerm、终端，图标从各应用的 `.icns` 里取；Windows 列资源管理器、VS Code、IntelliJ IDEA、PyCharm、WebStorm、GoLand、RustRover、Cursor、Trae、终端、PowerShell，图标用 `PrivateExtractIcons` 从 exe 里取 64px PNG——终端取的是 Windows Terminal 的 `WindowsTerminal.exe`（`WindowsApps\wt.exe` 只是 0 字节别名），`.cmd` 启动器（如 Trae 的 `bin\trae.cmd`）会先解析出它真正启动的 exe 再取图标，否则拿到的是脚本或快捷方式的通用图标。打开失败会弹提示而不是静默无反应；目录不存在时自动退到最近存在的上级目录。Windows 上「打开」统一走 `ShellExecuteW`（也就是资源管理器里双击那条路），不再用 `CreateProcess`——后者必须带 `CREATE_NO_WINDOW` 才不闪黑框，控制台程序因此拿不到窗口（终端、PowerShell 点了没反应就是这个原因）。VS Code / Trae / Cursor 这类 Electron 编辑器改走它们自带的 `bin\code.cmd`、`bin\trae.cmd`（直接给 `.exe` 传目录在部分机器上什么都不发生，由 CLI 负责单实例转发和「在哪个窗口打开」），图标仍然从真正的 exe 上取
- 新建项目时识别类型（Layui / Vue2 / Vue3 / React / SpringBoot / Spring / Struts2 / Java）和版本控制远端（Git / SVN），并做源目录校验；项目名称下方的 Git / SVN 标签和仓库地址都可以点击复制

### 笔记与附件

笔记用 `Vditor` 的即时渲染模式（IR）编辑，单栏所见即所得、边写边排版，交互参考 Typora：标题 / 列表 / 引用 / 表格 / 代码块即时呈现；工具栏末尾的铅笔按钮可在即时渲染与纯源码之间切换。光标落在表格里时，表格右侧会出现一个小工具条，点「↑ 行 / ↓ 行 / ← 列 / → 列」直接加行加列，不用切源码；代码块右上角的语言标签点开就是可搜索的语言选择列表（鼠标悬停在代码块上也会露出入口），选完立即换成对应语法高亮，不用进源码改围栏。内容 500 毫秒防抖后回写配置；切换实体或组件卸载前会把未落盘的修改补交一次。

Vditor 的运行时资源（lute / highlight.js / i18n / 图标 / 内容主题）不走 CDN：`vite.config.ts` 里的 `vditorOfflineAssets` 插件在 dev 时从 `node_modules/vditor/dist` 直出 `/vditor/dist/*`，构建时拷进 `dist/vditor/dist/*` 随安装包一起发布。

笔记页右侧是「大纲 / 附件」栏（默认显示大纲）：

- **附件**自动列出正文里引用的加密图标 / 图片（`![]()` 与 `<img>` 两种写法都认），显示缩略图、名称、大小，可复制 Markdown、预览大图、下载（解密另存）、重命名（改 alt）和删除（确认后同时移除正文引用与密文 blob）。附件栏不提供手动上传：资源只在笔记正文引用后出现，新增图片请用编辑器工具栏 / 粘贴 / 拖入正文
- 资源清单从正文的 `wst-img` 地址解析（地址里带着目录 / id / 扩展名），不额外存元数据，老笔记打开即生效；正文里没有的资源不显示
- 大小通过后端 `stat_attachment` 读取 WST1 明文头（magic / 版本 / 原始大小），不解密内容；密文文件丢失时卡片标记「文件缺失」，仍可从正文移除引用
- **大纲**解析 Markdown 标题，点击滚动到编辑器对应位置；窗口变窄时侧栏自动收起，右下角悬浮按钮可重新展开

附件从上传开始就是密文：

- 选文件，或把文件拖到「上传」页任意位置（整个内容区都是投放区）；上传时加密成 `<upload_dir>/<uuid>.wst`，先写 `.tmp` 再原子改名，磁盘上不会出现明文副本
- 容器格式 `WST1`：明文头（magic / 版本 / 原始大小）+ 若干 `SM4-CBC(PKCS7)` 帧 + HMAC-SM3 尾。分帧读写，内存占用近似常数，大文件不会整份读进内存
- 上传目录：设置页全局默认目录 → App 私有 `<app_data_dir>/attachments`（未设置时按需创建）
- 预览 = 应用内按需解密渲染（图像走 `wst-img` 协议，不生成明文临时文件）；下载 = 解密到指定路径；重命名只改元信息；删除会连同密文 blob 一起删
- `⌘/Ctrl+P` 快速搜索命中附件时也走应用内预览，同样不在系统临时目录留明文副本
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

分流的配置单位是**规则集**，每个 VPN 配置绑定其中一个。

- 规则类型只保留真正有数据面的三类：`DOMAIN`、`DOMAIN-SUFFIX`、`IP-CIDR`；出口只有 `DIRECT`（直连）与 `VPN` 两种
- 路由注入模式没有「按连接判定」这一步：`VPN` 规则只负责把命中的域名 / 网段送进隧道，未命中的一律直连（所以 `DIRECT` 规则天然满足，写不写都一样）
- 服务端下发的默认域名（`CISCO_DEF_DOMAIN`）与 split-include 网段会被自动并入，不用手工抄一遍
- 保存前校验规则语法，可以预览一条规则对指定「域名 / IP」的判定结果
- 右侧规则列表可编辑（点编辑按钮或双击整行，原地改，顺序不变）、可逐条启用/禁用（默认启用）；禁用的规则不参与分流但保留在列表里。编辑一条被禁用的规则并保存后，它会变成启用

三种代理模式，已连接时也能运行期切换（都是本地布线，点了就生效）：

- **规则模式（默认）**：不抓包、不抢默认路由、不改系统 DNS。命中「走 VPN」的域名交给 VPN DNS 解析，解析出来的内网 IP 装 `/32` 路由指向隧道；`IP-CIDR` 与服务端 split-include 网段整段注入隧道。默认路由和系统 DNS 都不动，结构上不可能「一连上就全网断」。
  - **macOS**：写 `/etc/resolver/<域名>` + 本机 DNS 转发器（`127.0.0.1:53`），转发器把应答回给应用**之前**就装好 `/32`，没有时间差。
  - **Windows**：用 NRPT（`Add-DnsClientNrptRule`）把这些域名的解析交给 VPN DNS；后台线程每 15 秒按域名向 VPN DNS 查 A 记录并注入 `/32`。Windows 没有「应答前注入」的用户态时机，所以按域名解析出的地址是连接后 1~2 秒补齐的；服务端下发的 split-include 网段不受影响（一开始就整段注入）。NRPT 不可用时退回「只给隧道网卡配 DNS」并在日志里说明原因。
- **全局模式**：`0.0.0.0/1` + `128.0.0.0/1` 两条半默认路由指向隧道，系统 DNS 改成 VPN DNS。装完立刻验证系统解析器还能解析，不通就整段回滚（路由与 DNS 一起还原）。本地直连网段比 `/1` 更具体，自动保持直连。
- **直连模式**：只建隧道，不接管任何流量。

`DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `RULE-SET` 需要按连接抓包或按进程判定，路由注入模式没有落点，保存时直接报错并提示改法，不会再「显示已启用但从不生效」。

#### 系统通知

四个「状态真的变了」的时刻发一条**系统通知**（Windows 通知 / macOS 通知中心），窗口最小化到托盘、或者用户切到别的应用时也能知道结果：

- **连接生效**：`phase` 从非 `connected` 跳到 `connected`（不是点完按钮就发，避免「点了但没连上」也报成功）
- **断开生效**：从 `connected` 落到 `disconnecting` / `disconnected`。`disconnecting → disconnected` 算同一次断开，只报一条；本来就失败或没连上的不报
- **规则生效**：规则热更新写进会话目录之后
- **代理模式生效**：`applied_mode` 在已连接状态下发生变化才算；连上那一拍 worker 会把 `applied_mode` 一起写进去，不能当成用户切了模式

这些通知由 **Rust 侧的状态轮询线程**发，不是前端在 `watch` 里调命令：窗口收进托盘、webview 被销毁时同样要提示；判定用「和上一次的状态快照比」，状态文件每 600ms 重复写同样的内容不会连环弹。通知都在独立线程里发（起一次 PowerShell 要几百毫秒），不占状态轮询和命令线程。界面上不再额外弹卡片，只走系统通知。

实现不依赖第三方通知库：Windows 用 PowerShell 调 WinRT 的 `ToastNotificationManager`（AUMID 固定为 `WsWorkbench`），macOS 用 `osascript -e 'display notification … with title … sound name "default"'`（带提示音，避免只闪一下被漏看）。Windows 的脚本通过 `-EncodedCommand`（UTF-16LE + base64）传给 PowerShell，而不是 `-Command` —— toast 的 XML 属性必须带双引号，走命令行转义时会被吃掉（`LoadXml` 报 `0xC00CE502`），编码方式零转义、也不受控制台代码页影响。

**Windows 上这条路并不保证成功**：Win11 24H2/25H2 起，便携 exe（没有 MSIX 包身份、也没有同 AUMID 的开始菜单快捷方式）调 `CreateToastNotifier` 会直接报 `0x80073D54`「该进程没有程序包标识符」——第三方库 `notify-rust` 报的是同一个错，实测过；而在放行 toast 的机器上，`Show` 也可能被系统静默吞掉。所以：

- 每次触发都会把结果写进 VPN 日志：`[通知] 已向系统提交「…」` 或 `[通知] 系统通知「…」发送失败：…`；
- 发送前会按当前用户注册一次 AUMID（`HKCU\Software\Classes\AppUserModelId\WsWorkbench`，写 `DisplayName`/`IconUri`/`ShowInSettings`，免管理员、可反复写），让系统设置里能看到这个应用，降低被静默丢弃的概率；
- 不用托盘气泡兜底：`NotifyIcon.ShowBalloonTip` 在同一台机器上既不弹也不报错（通知历史里查不到记录），指望不上。

#### 权限与进程拆分

建虚拟网卡、改路由表都要管理员权限，但让 GUI 常驻管理员既危险又每次启动弹窗，因此拆成两个进程：

- 主进程（普通权限）读配置、探测连接器、把请求写进会话目录、拉起 worker、轮询状态文件
- worker（管理员权限）独立进程，负责建隧道、按规则下发分流、守护、退出时回滚

提权方式按平台区分：

- Windows：`schtasks /Create /RL HIGHEST /F` 注册计划任务 `WsWorkbenchVpnWorker`，之后免 UAC。
  任务可用（存在且指向当前 exe + 当前会话目录）时连接直接 `schtasks /Run`，不弹任何框；
  **不可用时不再单独预注册**，而是把「注册任务 + 拉起 worker」合并进同一次提权批处理
  （`create-task-and-launch`）：这一次 UAC 同时把任务修好，下一次连接起就不再弹。
  `/F` 会把指向旧路径的过期任务直接覆盖掉，所以「移除旧任务」也是自动的，不需要用户点按钮。
  界面上**不再有「提权方式」区块**（原先的「注册免 UAC 任务 / 移除任务」按钮已撤掉）：
  整个生命周期跟着「连接」按钮走，状态只体现在日志里（`[启动]` / `[elevate]` 行）。
  注册失败（组策略禁止建任务）不影响本次连接，schtasks 的原始输出落在会话目录的
  `create-task.log` 里；这种机器只能每次连接弹一次系统授权框。
- macOS：`osascript ... with administrator privileges`

> 换新包时**覆盖解压到同一个目录**，任务的 `/TR` 里 exe 路径不变，升级后也完全不会弹 UAC；
> 解压到日期命名的新目录则会多一次 UAC（这一次会把任务挪到新路径）。

两个进程之间没有管道可用（提权进程拿不到父进程的句柄），IPC 全部靠会话目录里的文件。

#### 路由与脚本

下发路由不解析 `netsh` / `route print` 的文本输出，本地化后那套输出会解析失败。Windows 侧用 JScript（cscript）执行脚本，路由 / DNS / 网卡一律通过 PowerShell 的 `Get-NetRoute` / `New-NetRoute` / `Get-NetAdapter` / `Get-DnsClientNrptRule` 拿结构化结果；macOS 侧用 `route` / `ifconfig` / `netstat`。

- 装隧道路由：Windows `New-NetRoute -NextHop 0.0.0.0 -PolicyStore ActiveStore`（on-link，不落持久化路由表），macOS `route -n add -interface <utun>`；先判存在再建，重复调用幂等
- 删路由：Windows `Remove-NetRoute`，macOS `route -n delete`
- 半默认路由崩溃清理：只删**同时满足**「前缀是 `0.0.0.0/1` / `128.0.0.0/1`」且「网卡驱动描述像 VPN（Wintun / OpenConnect / TAP / AnyConnect）」的条目，不误伤 WireGuard、OpenVPN 这类同样用 `/1` 的客户端
- 按域名解析：macOS 写 `/etc/resolver`，Windows 写 NRPT。清理时都只删自己写的：macOS 靠清单文件 + 内容校验，Windows 只认 `DisplayName` 前缀 `ws-workbench:`
- 全局模式的 DNS：macOS `scutil` + `networksetup`（两处都改，`OverridePrimary`），Windows 只改隧道网卡（`Set-DnsClientServerAddress`），断开时复位成自动获取

生成的 vpnc-script 必须是纯 ASCII：Windows 那份由 cscript 按 ANSI 代码页解码，混入非 ASCII 字节会直接编译失败。脚本里用 `WS-WORKBENCH` 作为标记行，清理时据此定位自己写入的条目。脚本只负责配接口地址 / MTU、给 VPN 服务器加防环主机路由，并把 `TUNIDX`、`INTERNAL_IP4_*`、`CISCO_DEF_DOMAIN`、`CISCO_SPLIT_INC_*` 等参数回报给 worker —— 默认路由、DNS 与分流布线全部由 worker 按模式接管。

Windows 上的虚拟网卡由 Wintun 驱动提供（`src-tauri/resources/apps/openconnect/wintun.dll`），macOS 用系统自带的 utun。DNS 分流用到的 NRPT 由 `Dnscache` 服务执行，服务没跑时自动退回「只给隧道网卡配 DNS」并记录日志。

#### 凭据

密码不在命令行里传（进程列表可见），走 stdin 交给 openconnect 的 `--passwd-on-stdin`。落盘前加密：

- Windows：DPAPI，密文形如 `dpapi:<hex>`，解密时带 `CRYPTPROTECT_UI_FORBIDDEN`
- macOS：钥匙串，密文形如 `keychain:<key>`，服务名 `ws-workbench-vpn-<key>`

#### 安装引导

openconnect 探测失败时，代理页给出按平台区分的安装引导：macOS/Linux 给可复制的安装命令加官方下载地址；Windows 只说明内置副本缺失并提示覆盖安装，不引导下载外部二进制。

### 系统

左侧图标栏第四个工具（`⌘/Ctrl+4`，托盘菜单里也叫「系统」），四个页签（弹窗拦截仅 Windows 可见）：

- **软件卸载**：Windows 读注册表 `Uninstall` 键（HKLM 64/32 位、HKCU），macOS 扫 `/Applications` 与 `~/Applications`；支持按名称/发布者/位置搜索，按名称、大小、安装日期排序（顺序记在本地）。点「确认卸载」先只读检测应用是否在运行：不在运行直接卸载；在运行则弹二次确认（显示进程数与进程名），确认后先按平台正常退出（macOS `NSRunningApplication.terminate()`、Windows `CloseMainWindow()`），5 秒内退不干净就中止本次卸载并提示手动退出，不强制结束进程。卸载执行 `UninstallString`（MSI 的 `/I` 自动换成 `/X`，需要时一次 UAC），macOS 把 `.app` 移入废纸篓；没有卸载命令的条目提供「强制移除」（Windows 删注册表项）。卸载后按 bundle id / 注册表键匹配固定残留位，并对 macOS 用户目录做低置信度深扫（厂商二级目录、helper 容器、ByHost 等，最多 200 条、默认不勾选）；残留按「文件系统 / 注册表」分组、带置信度展示，勾选删除后自动重扫，已删条目从列表消失。
- **弹窗拦截**（仅 Windows）：默认关闭，开启后后台线程每 800ms `EnumWindows` 扫可见顶层窗口，按「工具窗口 / 不抢焦点 / 始终置顶 / 无标题 / 尺寸异常 / 类名与标题广告线索 / 进程不在前台」打分，可疑窗口进列表并通过 `popup-detected` 事件实时推给界面。两档模式：**检测模式**（默认）只记录，每条由用户点「关闭 / 隐藏」，同进程 5 秒限流；**拦截模式**（可选，默认关）命中闸门后自动处理——分数达阈值、至少两条命中原因、不在系统进程/外壳窗口保护表、不在忽略名单、同进程冷却 5 秒、每分钟最多 6 个（熔断），设置存 `<data_dir>/popup-settings.json`。「忽略」按进程名记到同目录 `popup-ignore.json`。不注入、不装驱动、不杀进程。
- **屏幕休眠**：阻止系统空闲睡眠（参考 Amphetamine 的公开电源断言方案），并支持「自动保持规则」（Triggers）：指定 App 运行时 / 接通电源时 / 连接外接显示器时自动保持，条件失效自动释放；手动开启优先，规则只释放自己开的会话。规则存 `<data_dir>/sleep-triggers.json`，后台 5 秒探测一次（`sysinfo` 进程列表、`pmset -g batt` / `GetSystemPowerStatus`、CoreGraphics 活动屏幕数 / `GetSystemMetrics(SM_CMONITORS)`），全部只读、不需要管理员权限。macOS 用 IOKit `IOPMAssertionCreateWithName`（`PreventUserIdleSystemSleep`，不勾「允许屏幕关闭」时再加 `PreventUserIdleDisplaySleep`，断言名 `ws-workbench Keep Awake`）；Windows 用专用线程持有 `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED [| ES_DISPLAY_REQUIRED])`。时长按会话式菜单选（参考 Amphetamine 的「开启新会话」）：不限时 / 5·10·15·20·25·30·45 分钟 / 1·2·3·4·6·8·12 小时，或自定义小时+分钟，点时长立即开始一个会话，到点自动释放（状态查询还有一道惰性回收）；不需要 root / 管理员，也不做登录级保活。
- **Hosts**：把系统 hosts 的完整内容直接读进等宽编辑器，想加什么行就加什么行，`⌘/Ctrl+S` 保存整份文件（保存前自动备份 + 一次系统授权，取消不会改动文件）。路径固定：macOS `/etc/hosts`，Windows `%SystemRoot%\System32\drivers\etc\hosts`；「备份 N」可展开最近 30 份备份并还原（路径见「打开目录」按钮的提示）。编辑器按 hosts 格式高亮注释 / IPv4 / IPv6 / 主机名，非法地址标红；超过 32 KB 自动关闭高亮，避免大文件输入卡顿。非 UTF-8 的 hosts 会拒绝编辑，避免写坏原有内容；CRLF 文件会保持原来的换行风格。
安全约束：普通删除只进废纸篓；需 root 的条目走一次提权批处理（`rm -rf` / `Remove-Item`，不经过废纸篓，删除前会明确提示）；所有路径过白名单校验，`/`、主目录、`/System`、`/usr`、`C:\Windows` 等硬拒绝；`pkgutil` 安装收据只读展示；系统应用（SIP / `com.apple.*`）不提供卸载。macOS 的废纸篓走 `NSFileManager` API，不会弹「控制 Finder」的自动化授权。弹窗拦截不做注入/驱动/进程结束；屏幕休眠不碰合盖与主动睡眠。

已知限制：Windows 侧只做了交叉编译检查与验收清单，未实机跑卸载/UAC/弹窗检测；残留以精确匹配为主，深扫可能漏报（不认识的自定义目录名）也可能多报（通用名字的目录），所以 low 条目默认不勾选；托盘常驻应用可能拦截正常退出，此时卸载会中止并提示手动退出；弹窗检测是「显示后处理」（弹窗会闪一下），UIPI 下关不掉管理员窗口，UAC / 锁屏 / 网页内嵌广告看不到；macOS 登录项在未授权「自动化」时只读。

**托盘快捷面板**：左键点菜单栏/任务栏图标弹出柠檬式的无边框小面板（`quick.html`，360 宽，高度随当前页签内容自动调整，失焦或再点一次托盘图标收起），两个页签：

- **VPN**：当前状态与配置名、「连接/断开」、规则/全局/直连三种代理模式即切。
- **休眠**：一键保持唤醒 / 停止，15 分钟 / 30 分钟 / 1 小时 / 不限四档时长，状态点与剩余时间实时刷新（复用系统页「屏幕休眠」的同一套命令与 composable）。

面板只做快速查看与动作；卸载、残留清理、规则编辑仍在主窗口。窗口位置以点击时的全局光标为锚点计算（托盘事件里的 rect 是窗口局部坐标，不能当屏幕坐标用）：面板在菜单栏图标下方 / 任务栏图标上方，水平居中于图标（像 Lemon 那样图标悬在面板上方中间），贴边时夹进显示器范围；macOS 全程用 AppKit 逻辑点（`NSEvent::mouseLocation` + `CGDisplayBounds` + `NSScreen` 菜单栏高度），因为 tao 的 cursor/monitor 接口在 Retina 缩放下把物理像素和逻辑点混算了。逻辑在 `lib.rs::panel_origin_at`，有 8 个单测（位置夹取、展开方向、切换页签变高时顶边/底边不动）。

### 剪贴板历史（macOS）

日常复制的代码片段、链接、命令，截图，甚至「文字 + 图片」的富文本选区，都会进本地加密历史：macOS 下按 `Command+Shift+V`（`⌘⇧V`）随时呼出选择面板，点一条就写回系统剪贴板，再手动 `⌘V` 粘贴。

- 采集由 `NSPasteboard.changeCount` 驱动（600ms 轮询，面板唤出时再强制补采一次），用 `arboard` 读 text / HTML / image，变了才读；启动时也把当前剪贴板收进来
- 历史落 SQLite（`clipboard_items` / `clipboard_attachments`）：文本与 HTML 是 SM4-CBC + HMAC-SM3 密文信封，图片是 WST1 密文文件，放在 `<data_dir>/clipboard-attachments/<id>.wst`，缩略图为 `<id>.thumb.wst`；库表里只有时间、类型、哈希等元信息
- HTML 里的 `data:image/...` 图片会被抽出来单独加密存，正文只留 `wsclip-img://<attachment_id>` 占位；回贴时还原成 data URI，尽量保留富文本。远程图片保留原 URL，不下载
- 不设条数上限，面板与管理页滚动懒加载（首屏 20 条）；面板唤出时显示总条数，搜索由后端解密文本 / HTML 后匹配，最多返回 100 条
- 按内容哈希去重：重复复制同一内容只会把记录置顶，不会新增；单条文本 ≤ 20 万字符、HTML ≤ 2MB、单图 ≤ 10MB、单条合计 ≤ 24MB，超限的复制直接跳过
- 面板是独立透明无边框窗口（`clipboard.html`，380×480），出现位置在鼠标附近、贴边时自动夹进屏幕；`⌘⇧V` 只负责打开 / 聚焦（300ms 去抖，连按不会收起来），失焦、`Esc` 或选中复制后收起
- 管理入口在「系统 → 剪贴板」页签：搜索、复制、删除单条、清空全部，图片可预览 / 复制单图 / 导出原图；删除与清空会级联删掉本地图片附件
- 图片展示走自定义协议 `wsclip://localhost/img?id=<id>&variant=thumb|full`（Windows 形态为 `http://wsclip.localhost/...`），读时解密并带 `Cache-Control: no-store`
- 只在 macOS 启用：Windows 自带的剪贴板历史 `Win+V` 已经够用，所以热键 / 采集线程 / 面板窗口在 Windows 上都不创建，管理页签显示说明

已知取舍：`⌘⇧V` 在应用运行期间是全局注册的，会接管 Chrome / 编辑器里同款「粘贴为纯文本」快捷键。若和你的习惯冲突，改 `src-tauri/src/lib.rs` 里注册的 `"Command+Shift+V"` 即可（历史与面板逻辑不用动）。

### 设置

显示运行环境（操作系统、架构、可执行文件后缀、是否需要提权、配置文件位置、各网卡的 MAC 与全部 IP —— 多 IP 逐条列出），以及附件实际落盘位置（可直接打开目录）。

「用编辑器打开项目」与非 Windows 的 VPN 连接器探测共用同一套应用查找逻辑，优先级为专用路径 > 已导入副本 > 内置资源 > 注册表 > PATH；导入副本只对 openconnect 开放。Windows 的 VPN 探测是例外：只认安装包内置副本，不查注册表、安装目录或 PATH。macOS 与 Linux 下分别检查 `/Applications`、Homebrew 前缀以及 PATH 里的 `/usr/bin`、`/usr/local/bin`。

### 日志

应用日志与 VPN 日志分两个来源，可切换、清空。VPN 日志来自会话目录里 worker 与 vpnc-script 的输出。

### 托盘

右键菜单：显示主窗口 / 系统 / 代理 / 设置 / 日志 / 连接 VPN / 断开 VPN / 退出。左键单击图标弹出快捷面板（VPN / 休眠两个页签，见上），菜单只在右键时出现。点「退出」会先请求 worker 收尾再结束进程，否则可能留下没人回收的路由。关闭主窗口只是隐藏，应用继续在托盘里运行。

快捷面板是独立的透明无边框窗口：点面板外部（失焦）或再点一次托盘图标收起，面板上没有关闭按钮，也没有底部操作栏（打开主窗口走托盘右键菜单）；窗口高度按当前页签内容自适应，切换 VPN / 休眠会重新量一次，不会留固定高度那种底部空白。macOS 的透明窗口依赖 Tauri 的 `macos-private-api` feature（`Cargo.toml` + `tauri.conf.json` 的 `macOSPrivateApi`），**直装/DMG 渠道没问题，但用不了 Mac App Store**。

macOS 的应用菜单做了汉化，点 Dock 图标会把窗口重新显示出来。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Tauri 2（`tauri` 开启 `tray-icon`、`image-png`、`macos-private-api`） |
| 前端 | Vue 3.5 + TypeScript 5.6 + Vite 6；ant-design-vue 4 + Vditor 4（组件与图标按需自动引入） |
| 后端 | Rust edition 2021，库名 `ws_workbench_lib` |
| 数据面 | 路由注入（隧道 /32 / 网段路由 + 按域名 DNS），不抓包 |
| 存储加密 | SM4-CBC + HMAC-SM3；主密钥由 macOS 钥匙串 / Windows DPAPI 托管 |
| 插件 | dialog、shell、store、updater、process；clipboard-manager（macOS 剪贴板历史）、global-shortcut（`⌘⇧V` 呼出） |
| 关键 crate | `ignore`、`zip`、`chrono`、`uuid`、`tempfile`、`sm4`、`hmac`、`cbc`、`nix` |

## 环境要求

- Node 22 与 pnpm（CI 固定 pnpm 10；`pnpm-lock.yaml` 为 9.0 格式）
- Rust stable 工具链
- 仅在需要 VPN 时：可用的 openconnect。Windows 必须使用 `src-tauri/resources/apps/openconnect/` 里的内置副本（安装包会一起打进去），macOS 建议 `brew install openconnect`
- VPN 连接需要管理员授权，见「权限与进程拆分」；两个平台都用路由注入做规则分流，不抓包，macOS 用系统 utun，Windows 用内置 `wintun.dll`

## 开发

```bash
pnpm install          # 安装前端依赖
pnpm dev              # 只起前端，http://localhost:8790
pnpm exec tauri dev   # 起完整桌面应用
```

Vite 固定在 8790 端口并开启 `strictPort`，端口被占用会直接失败而不是换端口；`@` 别名指向 `./src`，监听忽略 `src-tauri/`。

**样式里的 px 一律用偶数**（`11px` 写 `12px`，`13px` 写 `14px`）：缩放不是 100% 时，奇数尺寸会被摆到半个像素上，边框、图标、文字都会发虚。`1px` 的描边和分隔线是刻意留的发丝线，不受这条约束。

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

Hosts 在线编辑直接改系统 hosts（`/etc/hosts` / `C:\Windows\System32\drivers\etc\hosts`），不再单独存环境配置；每次保存前把原内容备份到 `<data_dir>/hosts-backups/`（保留 30 份）。剪贴板历史的文本 / HTML 与其它配置一起加密存在 `ws-workbench.db` 里，图片附件在 `<data_dir>/clipboard-attachments/`（WST1 密文），备份时一起复制即可。

VPN 会话目录同时被主进程和提权 worker 使用，因此不做路径拼接了事，而是带写入探测再逐级回退（受控文件夹访问、企业策略、受限令牌都可能让 `app_data_dir` 静默拒绝写入）。目录内文件：

| 文件 | 用途 |
| --- | --- |
| `request.json` | 主进程下发的连接请求 |
| `status.json` | worker 上报的实时状态（阶段、隧道参数、生效模式与路由） |
| `worker.log` / `script.log` | worker 与 vpnc-script 日志 |
| `stop.flag` | 请求 worker 停止 |
| `mode` / `rules.json` | 运行期代理模式与规则集（主进程写、worker 轮询后重新布线） |
| `main.alive` | 父进程心跳，worker 靠它判断主进程是否还活着 |
| `script/` | 生成的 vpnc-script |

超时与轮询节奏：连接超时 120 秒，守护周期 15 秒，状态轮询 600 毫秒。worker 退出码 `0` 正常、`2` 无请求、`3` 失败。

## 平台与限制

| 项 | Windows | macOS |
| --- | --- | --- |
| VPN 提权 | 计划任务免 UAC，失败回退 UAC 弹窗 | 系统授权框 |
| 虚拟网卡 | Wintun 驱动 | 系统 utun |
| 分流实现 | 路由注入：NRPT 按域名解析 + 隧道路由（不抓包） | 路由注入：`/etc/resolver` + 本机转发器 + 隧道路由（不抓包） |
| 按域名解析 | NRPT（`Add-DnsClientNrptRule`） | `/etc/resolver` + `127.0.0.1:53` 转发器 |
| 路由查询 | PowerShell JSON（`Get-NetRoute` 等） | `route` / `ifconfig` / `netstat` |

其余已知限制：

- 打包只支持 Layui 项目
- 相对路径含空格的文件会被跳过
- 分流只处理 IPv4：域名解析可能返回 AAAA，IPv6 目标不进隧道、仍走系统默认路由
- 域名分流只覆盖系统解析器（getaddrinfo）：自带 DoH/DoT 的应用不受控；它解析出的 IP 若命中 `IP-CIDR` 规则，照样能进隧道
- Windows 上按域名解析出来的 `/32` 是后台轮询注入的，新地址最多晚约 15 秒生效；服务端下发的 split-include 网段不受影响（整段注入）
- 规则与代理模式的改动都会热生效（worker 轮询 `rules.json` / `mode` 后重新布线），不需要断开重连
- 系统清理的 Windows 运行时行为（注册表枚举、UAC 卸载、工作集/待机列表）只做了交叉编译检查，需按设计文档第 9 节的 Windows 清单实机验收
- `~/.Trash` 内容受 TCC 保护，删除结果以「原路径消失 + trash API 成功」为准
- 卸载残留只做精确匹配（bundle id / 注册表键 / 精确应用名），存在非标准目录的数据需要手动清理
- 托盘快捷面板的原生弹出（真实点菜单栏图标）受 macOS 辅助功能/屏幕录制权限限制，自动化测试只覆盖了页面渲染、位置计算与 `cargo check`，首次使用请手动点一下确认弹窗位置与失焦收起
- 快捷面板的透明圆角窗口在 macOS 上依赖 `macos-private-api`，因此不支持 Mac App Store 渠道
- 剪贴板历史只在 macOS 启用（Windows 用系统自带的 `Win+V`）；历史加密落 SQLite、图片附件单独密文存放，**不联网**；`⌘⇧V` 在应用运行期间全局生效，会占用其它 app 的「粘贴为纯文本」快捷键。富文本回贴是尽力而为：依赖 source app 是否写入 HTML、目标 app 是否接受 HTML，图片过多或过大时可能降级成纯文本
- Hosts 在线编辑需要管理员授权；Windows 的提权写入 / UAC 取消 / DNS flush 只做了代码审查，需实机验收；非 UTF-8 的 hosts 文件会被拒绝编辑；没有语法校验，写错条目需自己负责（可随时从备份还原）

## 目录结构

```
src-tauri/src/
  commands/        前端命令入口按业务域拆分
    config.rs      配置读写、存储信息、本地键值加密
    project.rs     打包、校验、项目类型与 VCS 识别
    system.rs      打开目录/终端/编辑器、平台与网络信息
    attachments.rs 附件加解密、上传目录解析、笔记内联图片
    cleaner.rs     系统工具命令（列表/卸载/残留）
    cleaner_sleep.rs / cleaner_popup.rs
                   屏幕休眠 / 弹窗拦截（仅 Windows）的命令入口
    quick.rs       托盘快捷面板：隐藏、按页签内容调整高度
    clipboard.rs   剪贴板历史：changeCount 采集 / 运行时状态 / 面板显隐（macOS）
    hosts.rs       Hosts 在线编辑：读取 / 保存 / 备份列表 / 还原（整文件写入）
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
  clipboard.rs     剪贴板历史核心：changeCount 采集、内容分类、面板定位（纯逻辑带单测）
  clipboard/       rich.rs（HTML 图文抽取 / 还原 / 哈希 / 缩略图）、store.rs（SQLite 读写与附件路径）
  hosts/           Hosts 在线编辑：内容校验（mod.rs）、备份（store.rs）、提权写入（file.rs）
  cleaner/         系统工具：安装列表/卸载/残留扫描 + 屏幕休眠、弹窗拦截的后端
    running.rs     卸载前运行检测与优雅退出（sysinfo + NSRunningApplication / PowerShell CloseMainWindow）
    sleep.rs       防休眠（macOS IOKit 断言 / Windows SetThreadExecutionState）
    popup.rs       弹窗检测（仅 Windows：EnumWindows 轮询 + 启发式打分）
    *_macos.rs / *_windows.rs  平台实现，按 cfg 拆文件
  vpn/
    models.rs      规则、规则集、VPN 配置、状态
    mod.rs         模块导出与数据目录名
    rule_engine.rs 规则类型 / 走向解析（纯逻辑，可离线单测）
    split.rs       规则模式：按域名分流解析 + 隧道路由注入（macOS 转发器 / Windows 主动解析）
    win_dns.rs     Windows 专用：NRPT 规则与接口 DNS
    fulltunnel.rs  全局模式：半默认路由 + 系统 DNS 接管（带验证与回滚）
    dns_client.rs  最小 DNS 客户端：构造 A 查询、解析应答
    worker.rs      提权进程，连接生命周期主体
    commands.rs    主进程侧命令与状态轮询
    connector.rs   拉起 openconnect
    route.rs       路由表读写与崩溃清理
    script.rs      生成 vpnc-script
    credential.rs  密码加密存储
    detect.rs      连接器可用性探测
    elevate.rs     提权启动
    guide.rs       openconnect 安装引导（Windows 只提示重装内置副本）
    session.rs     会话目录 IPC
src/
  components/      ProjectList / GroupDetail / ProjectDetail / ProjectConfig /
                   NoteEditor / AttachmentPanel / PackProgress / ExcludeRules / LogPanel
    vpn/           VpnPanel / VpnProfileDialog / VpnRuleDialog / VpnRoutePreview /
                   VpnStatusDot / VpnLogDrawer / VpnInstallGuide
    settings/      SettingsPanel
    cleaner/       CleanerPanel / InstalledAppsPanel / MemoryPanel / AppIcon /
                   PopupPanel / SleepPanel / StartupPanel / DefaultAppsPanel
    hosts/         HostsEditor（整文件在线编辑 + 备份还原）
    clipboard/     ClipboardManager（「系统 → 剪贴板」管理页签）
  composables/     useConfig / usePack / useVpn / useCleaner / useApps / useLog / useUpdater /
                   useWorkbenchLayout / useEntitySelection / useProjectForm /
                   useDeleteConfirm / safeStorage / useKeepAwake / useStartup / usePopup /
                   useDefaultApps / useHosts / useClipboardHistory
  api/             后端命令的类型化封装（config / project / system / attachments / apps / cleaner /
                   sleep / startup / popup / defaultapps / clipboard / hosts）
  utils/           通用小工具（time / dialog / path / error）
  types/           前后端共享的数据结构（含 cleaner / sleep / startup / popup / defaultapps / hosts / clipboard）
  quick/           托盘快捷面板（独立入口：quick.html + main.ts + QuickPanel.vue）
  clipboard/       剪贴板历史面板（独立入口：clipboard.html + main.ts + ClipboardPanel.vue）
scripts/release.mjs  构建、上传、更新清单合并
updates/latest.json  更新清单（由发布脚本生成并提交）
```

分层约定：

- 通用能力只放 `src/utils`、`src/api` 与 `src-tauri/src/util`，业务代码不重复实现路径、时间、对话框、命令调用。
- 前端每个业务一个 composable 或组件（项目、VPN、设置、附件各自独立），跨业务组合只在 `App.vue` 里做。
- Rust 命令按业务域放在 `commands/<域>.rs`，新增命令先归域再导出，避免所有入口堆在一个文件里。
- 改某个业务时只动它自己的模块；通用层变更需同时跑 `pnpm build` 与 `cargo test --lib`。
