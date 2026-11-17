# 功能详解

> 来源：原 [README](../README.md) 的「功能」「日志」章节；编码规则见 [AGENTS.md](../AGENTS.md)。

## 功能

### 工作区

- 左侧项目树：分组和「一级项目」（不属于任何分组）混排在同一层，像文件管理器里的文件夹和文件。分组不能嵌套分组，也不能把分组拖进分组；默认分组不可删除，但组内项目可删
- 顶部「+ 新建」打开弹窗：顶部切换「新建分组 / 新建项目」，下面显示对应表单；从这里新建的项目落到一级。分组底部保留「+ 添加项目」：先选项目目录，选好后弹出新建项目表单（目录已带出），从哪个分组点进去，新建的项目就放在哪个分组里。分组与项目都支持拖拽排序：一级条目互相换位、项目在分组之间拖动、组内项目拖到一级或一级项目拖进分组。拖动时按落点实时预览（拖到哪就显示在哪），松手即落位，不用插入横线
- 分组和项目各自带一份 Markdown 笔记、一组加密附件（上传目录统一在「设置 → 附件上传」里配）
- 项目配置：源目录、输出目录。源目录的「打开方式」分体按钮挪到了配置页顶栏右侧（源目录 / 输出目录两行只留「选择」）：左半边固定用默认应用打开（访达 / 资源管理器），右半边是「用某个应用打开」的动作列表，点哪一项就立刻用那个应用打开当前目录，不切换、也不记住上次选了谁，菜单里给默认项标「（默认）」。macOS 列访达、DeepSeek、VS Code、Trae、Cursor、Xcode、IntelliJ IDEA、PyCharm、WebStorm、GoLand、iTerm、终端，图标从各应用的 `.icns` 里取；Windows 列资源管理器、VS Code、IntelliJ IDEA、PyCharm、WebStorm、GoLand、RustRover、Cursor、Trae、DeepSeek、终端、PowerShell，图标用 `PrivateExtractIcons` 从 exe 里取 64px PNG——终端取的是 Windows Terminal 的 `WindowsTerminal.exe`（`WindowsApps\wt.exe` 只是 0 字节别名），`.cmd` 启动器（如 Trae 的 `bin\trae.cmd`）会先解析出它真正启动的 exe 再取图标，否则拿到的是脚本或快捷方式的通用图标。打开失败会弹提示而不是静默无反应；目录不存在时自动退到最近存在的上级目录。Windows 上「打开」统一走 `ShellExecuteW`（也就是资源管理器里双击那条路），不再用 `CreateProcess`——后者必须带 `CREATE_NO_WINDOW` 才不闪黑框，控制台程序因此拿不到窗口（终端、PowerShell 点了没反应就是这个原因）。VS Code / Trae / Cursor 这类 Electron 编辑器改走它们自带的 `bin\code.cmd`、`bin\trae.cmd`（直接给 `.exe` 传目录在部分机器上什么都不发生，由 CLI 负责单实例转发和「在哪个窗口打开」），图标仍然从真正的 exe 上取
- 用 DeepSeek 打开：点 `DeepSeek` 会把当前项目目录交给 DeepSeek Harness——左侧没有这个目录就新增工作区，有就直接选中，并打开该工作区的空白新会话。DSH 侧靠一个独立发布的小插件（[@fengwenshan/dsh-open-workspace](https://github.com/fengwenshan/dsh-open-workspace)，按 tag 构建 zip 发到 GitHub Release 并同步 [Gitee](https://gitee.com/feng_wenshan/dsh-open-workspace) 的 `dist` 分支；「设置 → 资源」优先从 Gitee raw 下载，Gitee 不通再回退 GitHub）：点 `DeepSeek` 先检测本机有没有装 DeepSeek Harness（没装就提示先装）；装了但没装插件、或本地插件版本比远端最新版旧，会提示去「设置 → 资源」下载 / 更新，装完重启一次 DeepSeek 生效（10 分钟内的打开请求重启后自动生效）；DSH 没初始化过（profile 不存在）时会提示先打开一次。手动安装 / 卸载用 `scripts/dsh-plugin-install.sh --zip <zip路径>`（`--uninstall` 卸载）。
- 新建项目时识别类型（Layui / Vue2 / Vue3 / React / SpringBoot / Spring / Struts2 / Java）和版本控制远端（Git / SVN），并做源目录校验；项目名称下方的 Git / SVN 标签和仓库地址都可以点击复制

### 笔记与附件

笔记用 `Vditor` 的即时渲染模式（IR）编辑，单栏所见即所得、边写边排版，交互参考 Typora：标题 / 列表 / 引用 / 表格 / 代码块即时呈现；工具栏末尾的铅笔按钮可在即时渲染与纯源码之间切换。光标落在表格里时，表格右侧会出现一个小工具条，点「↑ 行 / ↓ 行 / ← 列 / → 列」直接加行加列，不用切源码；代码块右上角的语言标签点开就是可搜索的语言选择列表（鼠标悬停在代码块上也会露出入口），选完立即换成对应语法高亮，不用进源码改围栏。内容 500 毫秒防抖后回写配置；切换实体或组件卸载前会把未落盘的修改补交一次。

复制 / 剪切给的是**屏幕上看到的内容**（`text/plain` + `text/html`），不是 Markdown 源码：Vditor 自己的 copy 会把选区序列化成 Markdown，隐藏标记以 `\_`、`**`、反引号的形式进剪贴板，粘到公众号 / 密码管理器里就多出反斜杠（现场反馈：`demo_user@2022` 复制出来变成 `demo\_user@2022`）。现在复制走 IR DOM 的渲染层：去掉语法标记、还原代码块与链接（代码语言一并带上），应用内复制再粘贴能保住格式与围栏语言；源码模式下仍是原生的源码复制。另外从浏览器 / Word / 微信这类带 HTML 的来源粘贴时，Lute 的 `HTML2VditorIRDOM` 会把文本里的 `_` 转成转义节点存进正文；下划线夹在单词中间（`demo_user@2022`）这种多余转义会在粘贴和打开笔记时还原成普通字符，词边界上的下划线（`_init and foo_`，去掉反斜杠会被当成强调）保持转义不动。

Vditor 的运行时资源（lute / highlight.js / i18n / 图标 / 内容主题）不走 CDN：`vite.config.ts` 里的 `vditorOfflineAssets` 插件在 dev 时从 `node_modules/vditor/dist` 直出 `/vditor/dist/*`，构建时拷进 `dist/vditor/dist/*` 随安装包一起发布。

笔记页右侧是「大纲 / 附件」栏（默认显示大纲）：

- **附件**自动列出正文里引用的加密图标 / 图片（`![]()` 与 `<img>` 两种写法都认），显示缩略图、名称、大小，可复制 Markdown、预览大图、下载（解密另存）、重命名（改 alt）和删除（确认后同时移除正文引用与密文 blob）。附件栏不提供手动上传：资源只在笔记正文引用后出现，新增图片请用编辑器工具栏 / 粘贴 / 拖入正文
- 资源清单从正文的 `wst-img` 地址解析（地址里带着目录 / id / 扩展名），不额外存元数据，老笔记打开即生效；正文里没有的资源不显示
- 大小通过后端 `stat_attachment` 读取 WST1 明文头（magic / 版本 / 原始大小），不解密内容；密文文件丢失时卡片标记「文件缺失」，仍可从正文移除引用
- **大纲**解析 Markdown 标题，点击滚动到编辑器对应位置；窗口变窄时侧栏自动收起，右下角悬浮按钮可重新展开

附件从上传开始就是密文：

- 选文件，或把文件拖到「上传」页任意位置（整个内容区都是投放区）；上传时加密成 `<upload_dir>/<uuid>.wst`，先写 `.tmp` 再原子改名，磁盘上不会出现明文副本
- 容器格式 `WST1`：明文头（magic / 版本 / 原始大小）+ 若干 `SM4-CBC(PKCS7)` 帧 + HMAC-SM3 尾。分帧读写，内存占用近似常数，大文件不会整份读进内存
- 上传目录：设置页全局默认目录 → 数据目录下的 `attachments/`（未设置时按需创建；默认目录跟数据库同一个「数据仓库」，不可写时一起回退，见 [配置与数据](data.md)）
- 预览 = 应用内按需解密渲染（图像走 `wst-img` 协议，不生成明文临时文件）；下载 = 解密到指定路径；重命名只改元信息；删除会连同密文 blob 一起删
- `⌘/Ctrl+P` 快速搜索命中附件时也走应用内预览，同样不在系统临时目录留明文副本
- 快速搜索也搜项目：输入关键字后按项目名、分组路径和识别到的 Git / SVN 远端地址做「包含」匹配（支持局部关键字），选中直接跳到该项目；地址在打开搜索时按源目录现查并缓存，不写进项目配置
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

打包用**白名单（layui 项目打包规则）**：只有命中规则的文件才进包。规则放在**项目「配置 → 打包包含规则」**里，每个 layui 项目一套；项目没单独配置时沿用全局默认规则（`AppConfig.default_include`，老版本在设置页编辑的那份），「恢复默认」就是清掉项目覆盖、重新继承默认。输入纯后缀（如 `psd`、`.psd`）会自动补成 `*.psd`，方便给每个项目加自己的特殊文件类型。

```
*.html *.htm *.css *.js *.mjs *.json
*.png *.jpg *.jpeg *.gif *.svg *.webp *.avif *.ico *.bmp
*.woff *.woff2 *.ttf *.eot *.otf
*.mp4 *.webm *.mp3 *.wav *.ogg
*.txt *.xml
*.thm *.tjs *.dotm     # layui 模板 / 模板脚本 / WPS 控件模板
*.doc *.docx           # WebOffice 新建正文的空白模板（旧版 / 新版 Word）
*.cab                  # NTKO 正文控件的 ActiveX 安装包（IE）
*.ftl                  # NTKO 控件模板（服务端 FreeMarker 片段）
```

- 只列运行时真的会被请求的类型；`scss`/`ts`/`vue`/`map`/`md`、构建配置（`package.json`、`vite.config.*`）、设计源文件、压缩包这些开发期产物都不在里面
- layui 特有：`.thm`（表单/列表模板，`rsViewRender.js` 按 `APP_ROOT + '/xmodules/.../**.thm'` 拉取）、`.tjs`（模板脚本）、`.dotm`（WPS 控件的 `Normal.dotm`）都是运行时资源，默认收
- `.doc`/`.docx` 只收运行时真会 GET 的那类模板（`rsWebOffice.js` 用 `$.ajax` 拉 `weboffice/newDocument.doc(x)` 当新建正文模板，旧版 Word 模板是 `.doc`）；上传控件 accept 里出现的 `.pdf`/`.wps` 等字符串不算引用，工程里没有对应资源的仍不收
- NTKO 正文控件相关：`.cab`（`rsOfficeRender.js` 的 `codebase` 指向 `xmodules/pcip/module/lib/ntko/ntkoControl.cab`，IE 下由浏览器请求安装）、`.ftl`（控件模板 `ntko_lib.ftl`，前端不直接请求，但这类工程常与静态资源同目录部署、由后端 include，漏了正文控件起不来）
- **`.` 或 `_` 开头的路径段一律跳过**（任意层级，文件或目录都算）：`.svn`/`.git`/`.idea`/`.vscode`、`.DS_Store`、`_draft.html`、`partials/_header.html` 都由这一条硬规则兜住，不占规则名额
- 匹配语义：不带 `/` 的规则按**文件名**匹配、任意层级都算（`*.png`）；带 `/` 的按**相对源目录的路径**匹配（`assets/**`、`js/app.js`）
- 规则清空 = 视为未配置：除 `.`/`_` 开头的条目外全部打包。项目配置页因此把「清空」处理成回退到默认规则，避免误把整个目录打进包
- 路径里含空格的文件始终跳过

默认规则定义在 `src-tauri/src/models.rs` 的 `DEFAULT_INCLUDE`，带版本号（当前 `DEFAULT_INCLUDE_VERSION = 5`）：老配置（没有 include 字段）加载时自动补上默认集，之后用户自己删掉的规则不会被重新塞回去。项目级覆盖存在 `ProjectConfig.pack_include`（`None` = 继承全局默认），打包时前端取「项目规则非空 ? 项目规则 : 全局默认」传给后端，空数组不会直接传给打包器（后端把空规则当成「全部放行」）。

遍历由 `ignore::WalkBuilder` 完成，标准过滤全部关闭、不跟随符号链接；`.`/`_` 开头的条目在 `filter_entry` 里直接剪掉，包含规则用 `globset` 在遍历结果上过滤（文件名匹配 + 路径匹配两个集合）。相对路径中任一段含空格的文件会被跳过。输出目录与 zip 自身通过 `extra_excludes` 整棵子树剪掉，避免上一次的产物被再打一遍。

打包过程中通过 `pack-progress` 事件推送进度，阶段依次为 `scanning` → `cleaning` → `copying` / `zipping` → `done`。

### 代理（VPN）

底层只调 openconnect，不依赖 Cisco AnyConnect 客户端或 OpenConnect GUI。

左栏是配置列表，可以存多份配置（服务器、账号、协议、规则各自独立）；删除只拦「正在用这份配置连接」的那一份——连着 A 也能删 B，删完 A 的隧道照常跑（worker 启动时已经把参数拿到手）。

连接器来源按平台区分：

- **Windows 只认安装包内置的 openconnect**（`src-tauri/resources/apps/openconnect/openconnect.exe` + 同目录 `wintun.dll`）。不扫描注册表、常见安装目录或 PATH，也不提供「指定外部 openconnect」入口，避免把来源不明的二进制拉进提权进程。内置副本缺失时直接报错并提示重装。
- **macOS 也只认安装包内置的 openconnect**（源码在 `src-tauri/resources/openconnect-macos/openconnect`，安装包里映射到 `Contents/Resources/apps/openconnect/openconnect`）。和 Windows 一样不读 Homebrew、常见安装目录或 PATH，也不提供「指定外部 openconnect」入口；内置副本缺失或跑不起来时直接报错并提示覆盖安装。这份二进制是静态链接 GnuTLS 的自建产物（构建步骤见 `src-tauri/resources/openconnect-macos/README.md`），运行时只依赖 macOS 系统库，不受 `brew upgrade` 影响，也不再要求用户装 brew。
- macOS 旧版本留下的路径登记（`<data>/vpn/openconnect-path.txt`）和副本/软链（`<data>/apps/openconnect/`、`<data>/connectors/openconnect/`）会在下次检测时清理；清理后只用内置副本，内置副本缺失就回到安装引导。**「选择安装目录」入口在 macOS 与 Windows 上都不再提供**（界面与命令层逻辑只在尚未支持的 Linux 实现里保留）。

判定可用性的方式是真正执行一次 `openconnect --version` 并解析版本号，而不是看文件存在与否 —— 解压后缺依赖 DLL 是很常见的情况，光看路径会把跑不起来的二进制当成可用。

**子进程统一钉死 C locale**（`process::apply_c_locale` / worker 入口的 `pin_child_locale`）：openconnect 9.x 会按系统语言翻译输出，中文机器上 `--version` 打的是「OpenConnect 版本 v9.21」，worker 靠的 `Login failed` / `CSTP connected` / `Set up tun device failed` 也会跟着变中文。终端里跑（`LANG=C.UTF-8`）一切正常，打包成 .app 从 Finder 启动时环境里没有 `LANG` / `LC_ALL`，gettext 退回系统语言，版本行解析不到英文前缀，界面就误报「未找到 openconnect，请先安装」——`which openconnect` 却指向 `/opt/homebrew/bin/openconnect`。所以凡是输出要被解析的子进程都显式带 `LC_ALL=C`（并删掉优先级更高的 `LANGUAGE`），版本号解析也不再依赖英文 `version` 单词；拉起用户的编辑器 / 终端这类可见程序不带这个环境，保留用户自己的语言设置。

支持的协议：`anyconnect`、`nc`、`gp`、`pulse`、`f5`、`fortinet`、`array`。

#### 配置的增删改

新建 / 编辑配置先乐观更新列表，再整份落盘（`config.json` + `ws-workbench.db` 的 diff）；**落盘失败就不算成功**：列表退回改动前的状态，弹窗保持打开、表单内容不丢，并弹出失败原因（同时写进 VPN 抽屉日志），改完可以直接重试。之前失败只写抽屉日志，界面上看起来「创建成功、弹窗却不关」，重启后那条配置还消失了。

#### 规则分流

分流的配置单位是**规则集**，每个 VPN 配置绑定其中一个。

- 规则类型有三类：`DOMAIN`、`DOMAIN-SUFFIX`、`IP-CIDR`；出口只有 `DIRECT`（直连）与 `VPN` 两种。`IP-CIDR` 的值可以是单个 IP（自动按 `/32` 处理）或 CIDR 网段，例如 `192.168.6.115`、`192.168.6.0/24`
- 路由注入模式没有「按连接判定」这一步：`VPN` 规则只负责把命中的域名送进隧道，未命中的一律直连
- `DIRECT` 规则是**例外**：同一域名下取最具体的规则（后缀越长越具体；长度相同时 `DOMAIN` 比 `DOMAIN-SUFFIX` 具体）。所以 `DOMAIN-SUFFIX example.com → 走 VPN` 再加 `DOMAIN hlwapi.example.com → 直连`，就只有 hlwapi 不进隧道；规则在列表里的先后不影响判定。例外域名的解析照常走规则里的 DNS（答案来自 VPN DNS），只是不会被注入隧道路由
- `IP-CIDR` 不需要 DNS：连接完成就立刻把命中网段装成隧道路由，适合「内网网站只给 IP、没有域名」的场景。判定按最长前缀（`/24` 比 `/16` 具体），前缀相同时先出现的规则胜出；`DIRECT` 例外用更具体的直连路由表达（例如 `192.168.6.0/24 → VPN` 再加 `192.168.6.9/32 → DIRECT`，只有 `.9` 直连）
- 显式配置优先：规则里写了的网段一律按规则注入，不会因为「看起来像本机局域网」而自动跳过。如果规则恰好覆盖本机所在网段，那个网段的流量会进隧道（通常就访问不到了），断开连接时路由自动还原——所以别把本机局域网写成 `VPN`
- 历史 `IP-CIDR` 规则从本版本起生效；`DOMAIN` / `DOMAIN-SUFFIX` 里写 IP 的旧写法仍然不生效（弹窗会提示改用 IP 类型）
- 规则热更新不需要重连：改完点「完成」立刻重新布线。之前解析过的名字会按「DNS 答案账本」里记下的答案补注入 `/32`（macOS 由转发器记账，Windows 由缓存扫描记账），所以即使应用缓存着旧答案、不再发查询，「改成走 VPN / 改成直连」也会立即生效
- **严格规则模式**：走 VPN 的唯一依据是规则。服务端下发的默认域名（`CISCO_DEF_DOMAIN`）与 split-include 网段**不会自动并入**，只在「当前生效的分流」里展示（仅展示、不生效），方便把「服务端说内网在哪」和「客户端实际接管了什么」对照起来看；规则为空时不接管任何流量
- 保存前校验规则语法，可以预览一条规则对指定「域名 / IP」的判定结果
- 右侧规则列表可编辑（点编辑按钮或双击整行，原地改，顺序不变）、可逐条启用/禁用（默认启用）；禁用的规则不参与分流但保留在列表里。编辑一条被禁用的规则并保存后，它会变成启用

三种代理模式，已连接时也能运行期切换（都是本地布线，点了就生效）：

- **规则模式（默认）**：不抓包、不抢默认路由、不改系统 DNS。命中「走 VPN」的域名交给 VPN DNS 解析，解析出来的内网 IP 装 `/32` 路由指向隧道；`IP-CIDR` 规则命中的网段直接装成隧道网段路由，不经过 DNS。默认路由和系统 DNS 都不动，结构上不可能「一连上就全网断」。服务端下发的内网范围只做展示（见上方「严格规则模式」），不参与布线。
  - **macOS**：写 `/etc/resolver/<域名>` + 本机 DNS 转发器，转发器把应答回给应用**之前**就装好 `/32`，没有时间差。转发器绑 `127.0.0.1` 的**随机高位端口**（resolver 文件用 `port` 字段指过去），**不占 53**：Clash/dnsmasq 继续用自己的 53，不会因为 Vpn 先起来就把它们的 DNS 挤掉（之前「先试 53、失败再换端口」的写法一样会抢）。
  - **Windows**：用 NRPT（`Add-DnsClientNrptRule`）把这些域名的解析交给 VPN DNS。**不用本机转发器、也不占 53**：Windows DNS Client 不会把指向回环地址的 NRPT 查询送出来（`127.0.0.1` / `127.0.0.2` 实机都是 2.5 秒零查询），所以 Clash / dnsmasq / AdGuard 继续用它们的 53，我们不抢也不被抢。子域路由改为「扫系统 DNS 缓存 + 每 15 秒主动解析」补齐，首个请求可能失败一次、约 3 秒后重试即通；缓存扫描看到的答案同样记进「DNS 答案账本」（热更新时 teardown 要清空 DNS Client 缓存，没有这份记账，删掉直连例外就又要等应用重新解析）。命名空间与 `NameServers` 都没变时不重写 NRPT，写完还会校验一遍（缺了后缀规则就重写一次）——每次「删光再写」都会让 DNS 客户端读到半张策略表，系统日志会记 `Name resolution policy table has been corrupted`。
  - **系统代理会绕开按域名分流**：浏览器把请求交给本机 HTTP 代理（Clash、Surge、公司代理）后，解析发生在代理内部，NRPT / `/etc/resolver` 管不到；代理用自己的 DNS（常见 `8.8.8.8` / DoH）把内网域名解析成公网地址，表现就是「用 IP 能开、用域名打不开」。连接时检测系统代理（Windows 读 WinINET 注册表，macOS 解析 `scutil --proxy`），规则里的域名只要有一个不在代理绕过列表里，就写进 VPN 抽屉日志，并在代理页顶部显示一条可关闭的提示，给出要加进绕过的后缀（如 `*.risencn.com`）。只提示，不改用户的代理设置。
- **全局模式**：`0.0.0.0/1` + `128.0.0.0/1` 两条半默认路由指向隧道，系统 DNS 改成 VPN DNS。装完立刻验证系统解析器还能解析，不通就整段回滚（路由与 DNS 一起还原）。本地直连网段比 `/1` 更具体，自动保持直连。
- **直连模式**：只建隧道，不接管任何流量。

`DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` / `RULE-SET` 需要按连接抓包或按进程判定，路由注入模式没有落点，保存时直接报错并提示改法，不会再「显示已启用但从不生效」。

#### 关机 / 换网 / 代理长连接

三个「隧道本身没坏、但用户以为规则失效」的场景，macOS 与 Windows 用同一套处理：

- **关机 / 重启 / 注销前先断开**：在系统终止流程里同步写停止请求，最多等 2.5 秒让 worker 撤掉路由与解析器配置再放行。Windows 走 `WM_QUERYENDSESSION` 子类过程；macOS 走 `RunEvent::Exit`（`applicationWillTerminate → LoopDestroyed`，loginwindow 会等本进程退出，所以这几秒等于「先断开再关机」）。Windows 上不返回 `FALSE` 拦关机：`lParam` 区分不出关机 / 重启 / 休眠，拦下来再自己发起容易把重启变成关机。超时也放行，残留交给下次启动的 `cleanup_stale` / `restore_stale` 兜底；强制关机、断电这类系统不给收尾机会的情况拦不住。
- **换网后自动重连**：Windows 用 `NotifyUnicastIpAddressChange` 地址变更回调；macOS 每 2 秒用 `getifaddrs` 取一次「接口名 + IPv4」指纹（只是系统调用、不起子进程），代号一变 worker 再比一次物理默认出口。确认换了网就撤掉旧布线、重新拉起 openconnect；隧道网卡自身抖动不算换网。
- **点名「绕过规则的已有连接」**：路由注入只影响新连接，nginx / 数据库客户端这类长连接建立于布线之前，改路由也带不进去。worker 每 60 秒扫一次已建立的 TCP 连接：远端落在隧道路由里、本地又不是隧道地址的，在日志里点名进程与远端地址（Windows `Get-NetTCPConnection`，macOS `lsof -nP -iTCP -sTCP:ESTABLISHED -F pcn`），提示「重启这个程序后新连接才会按规则走」。nginx 自己配的 DNS（`resolver` / DoH）同样拦不住，日志里给的是同一条建议。

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
- macOS：`osascript ... with administrator privileges`。macOS 没有类似「注册计划任务免 UAC」的持久授权机制，所以**每次点「连接」都会弹一次系统授权框、要输一次密码**；断开后 worker 会自行收尾退出，不常驻 root 进程。这是刻意的取舍：少一次输密码就得常驻特权进程或写 sudoers 免密，安全面更大。

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

openconnect 探测失败时，代理页给出按平台区分的安装引导：macOS 与 Windows 都只说明内置副本缺失/无法启动并提示覆盖安装，不引导下载外部二进制；Linux 给发行版安装命令。macOS 与 Windows 都不再提供「选择安装目录」，内置副本缺失只能重装。

### 系统

左侧图标栏第四个工具（`⌘/Ctrl+4`），四个页签（弹窗拦截仅 Windows 可见）：

- **软件卸载**：Windows 读注册表 `Uninstall` 键（HKLM 64/32 位、HKCU），macOS 扫 `/Applications` 与 `~/Applications`；支持按名称/发布者/位置搜索，按名称、大小、安装日期排序（顺序记在本地）。行内「卸载」打开独立卸载窗口（820×560，macOS 隐藏标题、红绿灯浮在内容上；Windows 自绘标题栏，与主界面一致）：按「程序文件 / 支持文件 / 缓存文件」列出关联文件——程序文件 = 主 `.app` + Spotlight/兜底目录找到的同 id 副本，支持/缓存复用残留规则；带大小、版本、置信度：默认只勾缓存/日志/崩溃报告（可重建）和必须卸载的应用本体；设置、账号、插件配置等用户数据默认保留，点「彻底删除（含用户数据）」或手动勾选才会清理，low 只列出不勾选，底部「全选」可全选，应用本体始终必选。点「确认删除」先只读检测是否在运行：不在运行直接执行；在运行弹二次确认（进程数与进程名），点「强制退出」先按平台正常退出（macOS `NSRunningApplication.terminate()`、Windows `CloseMainWindow()`），3 秒内退不干净就逐进程强杀（macOS `SIGKILL`、Windows `taskkill /PID … /T /F`），每 250ms 重新检测并补杀重启出来的新进程，全部进程消失后才继续（总上限约 20 秒，退不干净即中止并提示）。随后先跑应用自带卸载程序（macOS 主 `.app` 移入废纸篓；MSI 的 `/I` 自动换 `/X`，需要时一次 UAC），卸载完成后自动再扫一次残留——仍有残留就回到勾选页（提示「卸载已完成。重新扫描到 N 项残留」，主按钮变「清理残留」），没有残留直接进结果页——再按本次扫描白名单删除勾选的关联文件：卸载程序已删掉的路径记「跳过」不算失败，需 root 的条目走一次合并授权；结果页给出删除/跳过/失败明细，失败项可在结果页「重试清理」，卸载/删除后主列表自动刷新。运行中的应用本体不出现在列表里，命令层也会再拦一次——不能卸载自己。
- **卸载 ws-workbench 自己**：macOS 安装包（DMG）里带「卸载 ws-workbench.command」。DMG 根目录另外还有「安装说明.txt」和「安装 ws-workbench.command」：双击后者会自动把 app 复制到 `/Applications`、执行 `sudo xattr -cr /Applications/ws-workbench.app` 清除隔离属性（未公证的包直接双击会被 Gatekeeper 拦成「已损坏」/「无法验证开发者」）并启动。卸载双击后先正常退出应用，退不掉会询问是否强制退出；**所有相关进程归零后才继续**，否则直接报「卸载失败」且不删除任何文件。继续后列出应用本体、数据目录、附件上传目录、剪贴板图片和 WebView 缓存，并弹出多选勾选：设置/数据库/剪贴板历史、附件目录、缓存/日志/偏好设置三类，默认**全部不勾（保留）**，勾选的类别二次确认后删除（附件/数据库删除后不可恢复）。自定义到应用数据目录之外的附件目录只提醒、不自动删；真实路径由应用启动/保存设置时写入 `uninstall-info.txt`（纯路径，不含密钥）。Windows 继续使用安装器自带的标准卸载器（设置 → 应用 → 卸载），安装包里的 NSIS 钩子会在删文件前检查 `ws-workbench.exe`（在运行就问是否强制退出，退不掉直接中止）、读取同一份 `uninstall-info.txt`，并调用 App 写在数据目录里的 `uninstall-choose-data.ps1` 显示同样的三类多选框；勾选结果写在 `%APPDATA%\ws-wb-uninstall-selection.txt`，NSIS 按结果删除、未勾选一律保留，自定义附件目录只提示不删；静默卸载/勾选脚本缺失/调用失败都按保留数据处理。
- **弹窗拦截**（仅 Windows）：火绒式的规则拦截。默认关闭，总开关持久化，开启后下次启动自动恢复；后台线程每 500ms `EnumWindows` 扫可见顶层窗口——**先匹配拦截规则**（进程必须一致，窗口类 / 标题子串可选，命中直接 `WM_CLOSE` / `SW_HIDE` 并记一条拦截记录），没有规则才走启发式打分（工具窗口 / 不抢焦点 / 始终置顶 / 无标题 / 尺寸异常 / 类名与标题广告线索 / 进程不在前台），可疑窗口进「可疑记录」等用户处理。对可疑弹窗点「拦截此类」会生成一条持久化规则（默认按「进程 + 窗口类」，同一个广告位换标题/素材也能拦；也可选「仅此弹窗」或「该程序全部」），之后同类弹窗出现即自动关闭；「从窗口添加」直接列出当前可见窗口挑选一个加规则，规则可停用 / 删除 / 立即执行，命中次数与最近命中时间落 `<data_dir>/popup-rules.json`。可选「智能拦截」（默认关）：没有命中规则时，按分数阈值 + 至少两条命中原因 + 系统保护表 + 忽略名单 + 每分钟上限自动处理，设置存 `popup-settings.json`；忽略名单（白名单）存 `popup-ignore.json`。界面用 exe 的版本信息取应用名和图标（取不到退回进程名 + 首字母色块）。不注入、不装驱动、不杀进程。
- **屏幕休眠**：阻止系统空闲睡眠（参考 Amphetamine 的公开电源断言方案），并支持「自动保持规则」（Triggers）：指定 App 运行时 / 接通电源时 / 连接外接显示器时自动保持，条件失效自动释放；手动开启优先，规则只释放自己开的会话。规则存 `<data_dir>/sleep-triggers.json`，后台 5 秒探测一次（`sysinfo` 进程列表、`pmset -g batt` / `GetSystemPowerStatus`、CoreGraphics 活动屏幕数 / `GetSystemMetrics(SM_CMONITORS)`），全部只读、不需要管理员权限。macOS 用 IOKit `IOPMAssertionCreateWithName`（`PreventUserIdleSystemSleep`，不勾「允许显示器睡眠」时再加 `PreventUserIdleDisplaySleep`，断言名 `ws-workbench Keep Awake`）；Windows 用专用线程持有 `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED [| ES_DISPLAY_REQUIRED])`。时长按会话式菜单选（参考 Amphetamine 的「开启新会话」）：不限时 / 5·10·15·20·25·30·45 分钟 / 1·2·3·4·6·8·12 小时，或自定义小时+分钟，点时长立即开始一个会话，到点自动释放（状态查询还有一道惰性回收）；不需要 root / 管理员，也不做登录级保活。
- **Hosts**：把系统 hosts 的完整内容直接读进等宽编辑器，想加什么行就加什么行，`⌘/Ctrl+S` 保存整份文件（保存前自动备份 + 一次系统授权，取消不会改动文件）。路径固定：macOS `/etc/hosts`，Windows `%SystemRoot%\System32\drivers\etc\hosts`；「备份 N」可展开最近 30 份备份并还原（路径见「打开目录」按钮的提示）。编辑器是 Monaco（VS Code 内核，本地打包、首次打开 Hosts 页签时懒加载）：行号对齐、注释 / IPv4 / IPv6 / 主机名高亮、非法地址标红，支持撤销重做、查找替换（`⌘/Ctrl+F`）、多光标与右键菜单。非 UTF-8 的 hosts 会拒绝编辑，避免写坏原有内容；CRLF 文件会保持原来的换行风格。
安全约束：普通删除只进废纸篓；需 root 的条目走一次提权批处理（`rm -rf` / `Remove-Item`，不经过废纸篓，删除前会明确提示）；所有路径过白名单校验，`/`、主目录、`/System`、`/usr`、`C:\Windows` 等硬拒绝；`pkgutil` 安装收据只读展示；系统应用（SIP / `com.apple.*`）不提供卸载。macOS 的废纸篓走 `NSFileManager` API，不会弹「控制 Finder」的自动化授权。弹窗拦截不做注入/驱动/进程结束；屏幕休眠不碰合盖与主动睡眠。

已知限制：Windows 侧只做了代码审查与交叉编译尝试（本机缺 MSVC SDK，第三方 `ring` 构建失败），未实机跑卸载/UAC/强制退出/弹窗检测；残留以精确匹配为主，深扫可能漏报（不认识的自定义目录名）也可能多报（通用名字的目录），所以 low 条目默认不勾选；强制退出会连同 bundle 内的 helper/XPC 一起 `SIGKILL`、可能丢掉未保存的数据（二次确认里已提示），仍有进程退不掉时卸载会中止并提示手动退出；卸载窗口的关联文件清单已过滤 `~/.Trash`，不会顺手清空废纸篓；弹窗检测是「显示后处理」（弹窗会闪一下），UIPI 下关不掉管理员窗口，UAC / 锁屏 / 网页内嵌广告看不到；macOS 登录项在未授权「自动化」时只读。

**托盘快捷面板**：右键点菜单栏/任务栏图标弹出柠檬式的无边框小面板（`quick.html`，360 宽，高度随当前页签内容 + 底部操作栏自动调整，失焦或再右键一次托盘图标收起），两个页签：

- **VPN**：当前状态与配置名、「连接/断开」、规则/全局/直连三种代理模式即切。
- **休眠**：一键保持唤醒 / 停止，15 分钟 / 30 分钟 / 1 小时 / 不限四档时长；状态与主窗口「屏幕休眠」双向同步：主窗口里开的会话这里显示「剩余约 X 分钟 · 共 Y」并高亮对应档位，保持中也能直接点其它档位切换时长，停止后面板沿用该时长。复用系统页同一套命令与 composable，面板自己按 5 秒轮询、每次弹出 / 切回休眠页签立即补拉，不会出现主窗口在保持、面板还显示「未开启」。

面板底部固定一条操作栏：**显示主界面**（收起面板并唤出主窗口）和 **退出**（结束整个应用）。面板只做快速查看与动作；卸载、残留清理、规则编辑仍在主窗口。窗口位置以点击时的全局光标为锚点计算（托盘事件里的 rect 是窗口局部坐标，不能当屏幕坐标用）：面板在菜单栏图标下方 / 任务栏图标上方，水平居中于图标（像 Lemon 那样图标悬在面板上方中间），贴边时夹进显示器范围；macOS 全程用 AppKit 逻辑点（`NSEvent::mouseLocation` + `CGDisplayBounds` + `NSScreen` 菜单栏高度），因为 tao 的 cursor/monitor 接口在 Retina 缩放下把物理像素和逻辑点混算了。逻辑在 `lib.rs::panel_origin_at`，有 8 个单测（位置夹取、展开方向、切换页签变高时顶边/底边不动）。

### 剪贴板历史（macOS / Windows）

日常复制的代码片段、链接、命令，截图，甚至「文字 + 图片」的富文本选区，都会进本地加密历史：默认按 `⌘⇧V`（Windows 为 `Ctrl+Shift+V`）随时呼出选择面板，点文字条目就写回系统剪贴板，点缩略图看大图（不触发复制）；面板收起后焦点回到呼出前的 App（主窗口不会被一起带到前台），再手动 `⌘V` / `Ctrl+V` 粘贴。

- 采集由系统剪贴板的变化序列号驱动（macOS `NSPasteboard.changeCount`、Windows `GetClipboardSequenceNumber`；600ms 轮询，面板唤出时再强制补采一次），用 `arboard` 读 text / HTML / image，变了才读；启动时也把当前剪贴板收进来
- 历史落 SQLite（`clipboard_items` / `clipboard_attachments`）：文本与 HTML 是 SM4-CBC + HMAC-SM3 密文信封，图片是 WST1 密文文件，放在 `<data_dir>/clipboard-attachments/<id>.wst`，缩略图为 `<id>.thumb.wst`；库表里只有时间、类型、哈希等元信息
- HTML 里的 `data:image/...` 图片会被抽出来单独加密存，正文只留 `wsclip-img://<attachment_id>` 占位；回贴时还原成 data URI，尽量保留富文本。远程图片保留原 URL，不下载
- 不设条数上限，面板与管理页滚动懒加载（首屏 20 条）；搜索由后端解密文本 / HTML 后匹配，最多返回 100 条
- 按内容哈希去重：重复复制同一内容只会把记录置顶，不会新增；单条文本 ≤ 20 万字符、HTML ≤ 2MB、单图 ≤ 10MB、单条合计 ≤ 24MB，超限的复制直接跳过
- 面板是独立透明无边框窗口（`clipboard.html`，380×480），出现位置在鼠标附近、贴边时自动夹进屏幕；`⌘⇧V` 只负责打开 / 聚焦（300ms 去抖，连按不会收起来），失焦、`Esc` 或选中复制后收起。窗口不在启动时创建：主窗口就绪后预热，第一次按快捷键时通常已经在了，抢在预热前也会现场建
- 图文条目写回剪贴板时，除了 HTML + 纯文本，还会原样带上来源应用的富文本私有表示（例如钉钉的 `com.dingtalk.richTextAttributedString`）：在钉钉里复制图文再粘回钉钉，能一次贴出文字+图片；不认这些表示的应用（微信等）仍按 HTML / 纯文本得到文字，图片可用大图里的「复制图片」单独贴
- 面板右上角三个图标：刷新、置顶、关闭。点「置顶」后失焦、复制都不再收起面板（可以连续复制给不同窗口用），`Esc` 或 `×` 收起；收起 / 关闭后置顶自动失效，下次呼出要重新点
- 管理入口在「系统 → 剪贴板」页签：搜索、复制、删除单条、清空全部，图片可预览 / 复制单图 / 导出原图；删除与清空会级联删掉本地图片附件
- 图片展示走自定义协议 `wsclip://localhost/img?id=<id>&variant=thumb|full`（Windows 形态为 `http://wsclip.localhost/...`），读时解密并带 `Cache-Control: no-store`
- macOS 与 Windows 都启用；Linux 不启用（不采集、不注册热键、不建面板）
- 快捷键可在「系统 → 剪贴板」工具栏的齿轮里按组合键重新录制，默认 `CommandOrControl+Shift+V`；保存时后端先尝试注册，被别的应用占用会报错并保留原来的组合。也可以点「清空」（录制时按退格同样清空），清空后不注册任何全局快捷键，面板要改回组合才能再呼出。设置存在 `config.json` 的 `clipboard_shortcut`（空字符串 = 已清空），启动时按这个值注册或跳过

已知取舍：默认组合在应用运行期间是全局注册的，会接管 Chrome / 编辑器里同款「粘贴为纯文本」快捷键。若和你的习惯冲突，在「系统 → 剪贴板」齿轮里换一个即可（历史与面板逻辑不用动）。

### 局域网分享（LocalSend 风格）

同一局域网里、都开着本应用的设备会自动出现在左侧图标栏的「局域网分享」页（快捷键 `5`），选中对方就能把本机文件发过去；对方面板会弹出待接收卡片，点「接受」文件才落盘，点「拒绝」或 60 秒不处理就取消。

- 首次运行默认开启（可在顶部状态卡里关掉）：关闭后不组播、不监听端口；开启后同一局域网内 3 秒内互相可见（15 秒没有心跳视为离线，从列表移除）
- 发现走 UDP 组播 `224.0.0.167:53317`；传输是内置的明文 HTTP 服务，端口默认 `53317`，被占用时在 `53317..53326` 里顺延，实际端口写进广播
- 多网卡 / 开 VPN 时：announce 里带上本机全部 IPv4，收到的一方会挨个探测，把真正连得上的那个（常常是 VPN 地址）显示在卡片上并用它发送；announce 的出口网卡用 `IP_MULTICAST_IF` 显式指定，避免组播被 VPN 抢走路由后发不出物理网卡。若两端被 VPN 隔离到连单播都不通，关掉 VPN 或让 VPN 放行本地局域网即可
- 发送：一次可选多个文件，逐文件流式上传，面板按文件显示进度；接收方点「接受」后后端才生成与会话绑定的一次性上传 token，`upload` 校验 session / 文件 id / token 与登记大小
- 接收目录默认就是系统「下载」目录（macOS `~/Downloads`，Windows `%USERPROFILE%\Downloads`），可在页顶状态卡里改（会做一次写探测）；重名文件自动加 ` (1)` 后缀，不覆盖已有文件；文件名只取 basename，路径分隔符 / `..` 会被清洗
- 页顶状态卡里可以直接改设备名（铅笔图标就地编辑）和接收目录；附近的设备以卡片网格展示（设备图标、在线状态、`IP:端口`、发送按钮）
- 只做本应用之间的互传：自定义协议，不与官方 LocalSend 互通；传输不加密，建议只在可信局域网使用
- macOS 首次使用会请求「本地网络」权限（`src-tauri/Info.plist` 里写了用途说明）；Windows 首次监听会弹防火墙放行提示，允许后即可被其他设备发现

### 设置

顶部「检查更新」卡片显示当前版本，只有查到更高版本时才多显示一行高亮的「最新版本」并出现「立即更新」（右上角「重新检测」会顺带检查一次更新）；下载/安装进度就地显示，下载中断后点「重试」会从断点续传（见 [自动更新](release.md)）；下面显示运行环境（操作系统、架构、可执行文件后缀、是否需要提权、配置文件位置、各网卡的 MAC 与全部 IP —— 多 IP 逐条列出），以及附件实际落盘位置（可直接打开目录）。

「用编辑器打开项目」使用的应用查找优先级为专用路径 > 标准安装位置 > 记住的路径 > 内置资源 > PATH（Windows 上还有注册表，位置在标准安装位置之前）；VPN 连接器探测不走这条：macOS 与 Windows 都只认安装包内置副本，不向外找。其中「选择安装目录」入口在 macOS / Windows 上都不再提供。macOS 与 Linux 下分别检查 `/Applications`、Homebrew 前缀以及 PATH 里的 `/usr/bin`、`/usr/local/bin`（这是「打开方式」的应用定位，与 VPN 连接器无关）。

### 托盘

**左键单击**图标直接显示主界面（面板开着时先收起面板）；**右键单击**图标弹出快捷面板（VPN / 休眠两个页签，见上），面板底部固定「显示主界面 / 退出」两个入口。托盘不再挂原生右键菜单：面板本身就是原来的菜单入口，「检查更新」已经收进主窗口「设置」页的更新卡片。

点「退出」等于结束整个应用和它拉起的进程：先给 worker 写停止标记（`stop.flag`），然后主进程立刻退出，托盘和任务栏图标当场消失；worker 是独立进程，会自己杀掉 openconnect、还原路由和 DNS 后退出（见上面的 `main.pid` / 心跳兜底），因此不需要卡住界面等它收尾，也就不用承受「点了退出好几秒图标才消失」。唯一代价是：从解压目录直接运行便携版时，退出后立刻删目录可能会被 worker 短暂占用，等一两秒再删即可。关闭主窗口只是隐藏，应用继续在托盘里运行。

快捷面板是独立的透明无边框窗口：点面板外部（失焦）或再右键一次托盘图标收起，面板上没有关闭按钮，底部只有「显示主界面 / 退出」两个操作；窗口高度按「当前页签内容 + 底部操作栏」自适应，切换 VPN / 休眠会重新量一次，不会留固定高度那种底部空白。macOS 的透明窗口依赖 Tauri 的 `macos-private-api` feature（`Cargo.toml` + `tauri.conf.json` 的 `macOSPrivateApi`），**直装/DMG 渠道没问题，但用不了 Mac App Store**。

启动顺序也按「别让窗口先出来干等」来排：主窗口先以隐藏状态创建，前端首屏渲染完再调 `app_ready` 让它显示；托盘面板和剪贴板面板不在启动路径里创建，等主窗口就绪后 600ms 在主线程预热（真抢在预热前用，`ensure_*` 会现场建）。这样 Windows 冷启动时既不会出现还没画完就被系统标成「未响应」的白窗口，也少两个 WebView2 抢启动时间。兜底：前端 8 秒内没就绪也会把主窗口显示出来，不会只剩托盘图标。启动期间重复双击也不会再弹一个白窗口：单实例的第二份进程要等首屏就绪后才去激活已有窗口。

macOS 的应用菜单做了汉化，点 Dock 图标会把窗口重新显示出来。

## 日志

不做全局日志页：哪个模块产生日志，就显示在哪个模块里。VPN 的连接 / 分流日志在「代理」页右侧抽屉（含清空）；项目打包的进度与结果（含错误明细）在「项目」页底部。这样出问题时看到的内容和操作上下文绑在一起，也不用再维护一份全局日志缓冲。
