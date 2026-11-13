# ws-workbench

基于 Tauri 2 的桌面工作台，把交付日常收进一个窗口：分组/项目管理、Markdown 笔记与加密附件、Layui 项目打包、VPN 规则分流、Hosts 在线编辑、运行环境自检；还带一份加密落盘的剪贴板历史（文本 / 图片 / 图文，macOS / Windows）。主页左侧像文件管理器一样摆项目：分组（文件夹）和一级项目（文件）混在一层，项目也可以拖进分组，系统 / 代理 / 设置从左侧图标栏进入；关闭窗口不退出应用，常驻系统托盘（左键显示主界面，右键弹快捷面板，面板底部有「显示主界面 / 退出」）。

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

复制 / 剪切给的是**屏幕上看到的内容**（`text/plain` + `text/html`），不是 Markdown 源码：Vditor 自己的 copy 会把选区序列化成 Markdown，隐藏标记以 `\_`、`**`、反引号的形式进剪贴板，粘到公众号 / 密码管理器里就多出反斜杠（现场反馈：`Risen_typt@2022` 复制出来变成 `Risen\_typt@2022`）。现在复制走 IR DOM 的渲染层：去掉语法标记、还原代码块与链接（代码语言一并带上），应用内复制再粘贴能保住格式与围栏语言；源码模式下仍是原生的源码复制。另外从浏览器 / Word / 微信这类带 HTML 的来源粘贴时，Lute 的 `HTML2VditorIRDOM` 会把文本里的 `_` 转成转义节点存进正文；下划线夹在单词中间（`Risen_typt@2022`）这种多余转义会在粘贴和打开笔记时还原成普通字符，词边界上的下划线（`_init and foo_`，去掉反斜杠会被当成强调）保持转义不动。

Vditor 的运行时资源（lute / highlight.js / i18n / 图标 / 内容主题）不走 CDN：`vite.config.ts` 里的 `vditorOfflineAssets` 插件在 dev 时从 `node_modules/vditor/dist` 直出 `/vditor/dist/*`，构建时拷进 `dist/vditor/dist/*` 随安装包一起发布。

笔记页右侧是「大纲 / 附件」栏（默认显示大纲）：

- **附件**自动列出正文里引用的加密图标 / 图片（`![]()` 与 `<img>` 两种写法都认），显示缩略图、名称、大小，可复制 Markdown、预览大图、下载（解密另存）、重命名（改 alt）和删除（确认后同时移除正文引用与密文 blob）。附件栏不提供手动上传：资源只在笔记正文引用后出现，新增图片请用编辑器工具栏 / 粘贴 / 拖入正文
- 资源清单从正文的 `wst-img` 地址解析（地址里带着目录 / id / 扩展名），不额外存元数据，老笔记打开即生效；正文里没有的资源不显示
- 大小通过后端 `stat_attachment` 读取 WST1 明文头（magic / 版本 / 原始大小），不解密内容；密文文件丢失时卡片标记「文件缺失」，仍可从正文移除引用
- **大纲**解析 Markdown 标题，点击滚动到编辑器对应位置；窗口变窄时侧栏自动收起，右下角悬浮按钮可重新展开

附件从上传开始就是密文：

- 选文件，或把文件拖到「上传」页任意位置（整个内容区都是投放区）；上传时加密成 `<upload_dir>/<uuid>.wst`，先写 `.tmp` 再原子改名，磁盘上不会出现明文副本
- 容器格式 `WST1`：明文头（magic / 版本 / 原始大小）+ 若干 `SM4-CBC(PKCS7)` 帧 + HMAC-SM3 尾。分帧读写，内存占用近似常数，大文件不会整份读进内存
- 上传目录：设置页全局默认目录 → 数据目录下的 `attachments/`（未设置时按需创建；默认目录跟数据库同一个「数据仓库」，不可写时一起回退，见「数据与配置」）
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
*.cab                  # NTKO 正文控件的 ActiveX 安装包（IE）
*.ftl                  # NTKO 控件模板（服务端 FreeMarker 片段）
```

- 只列运行时真的会被请求的类型；`scss`/`ts`/`vue`/`map`/`md`、构建配置（`package.json`、`vite.config.*`）、设计源文件、压缩包这些开发期产物都不在里面
- layui 特有：`.thm`（表单/列表模板，`rsViewRender.js` 按 `APP_ROOT + '/xmodules/.../**.thm'` 拉取）、`.tjs`（模板脚本）、`.dotm`（WPS 控件的 `Normal.dotm`）都是运行时资源，默认收
- `.doc`/`.docx` 只收运行时真会 GET 的那类模板（`rsWebOffice.js` 用 `$.ajax` 拉 `weboffice/newDocument.doc(x)` 当新建正文模板，旧版 Word 模板是 `.doc`）；上传控件 accept 里出现的 `.pdf`/`.wps` 等字符串不算引用，工程里没有对应资源的仍不收
- NTKO 正文控件相关：`.cab`（`rsOfficeRender.js` 的 `codebase` 指向 `xmodules/pcip/module/lib/ntko/ntkoControl.cab`，IE 下由浏览器请求安装）、`.ftl`（控件模板 `ntko_lib.ftl`，前端不直接请求，但这类工程常与静态资源同目录部署、由后端 include，漏了正文控件起不来）
- **`.` 或 `_` 开头的路径段一律跳过**（任意层级，文件或目录都算）：`.svn`/`.git`/`.idea`/`.vscode`、`.DS_Store`、`_draft.html`、`partials/_header.html` 都由这一条硬规则兜住，不占规则名额
- 匹配语义：不带 `/` 的规则按**文件名**匹配、任意层级都算（`*.png`）；带 `/` 的按**相对源目录的路径**匹配（`assets/**`、`js/app.js`）
- 规则清空 = 视为未配置：除 `.`/`_` 开头的条目外全部打包（避免误删规则后打出空包）
- 路径里含空格的文件始终跳过

默认规则定义在 `src-tauri/src/models.rs` 的 `DEFAULT_INCLUDE`，带版本号（当前 `DEFAULT_INCLUDE_VERSION = 5`）：老配置（没有 include 字段）加载时自动补上默认集，之后用户自己删掉的规则不会被重新塞回去。「恢复默认」按钮调用 `get_default_include_rules` 取同一份清单。

遍历由 `ignore::WalkBuilder` 完成，标准过滤全部关闭、不跟随符号链接；`.`/`_` 开头的条目在 `filter_entry` 里直接剪掉，包含规则用 `globset` 在遍历结果上过滤（文件名匹配 + 路径匹配两个集合）。相对路径中任一段含空格的文件会被跳过。输出目录与 zip 自身通过 `extra_excludes` 整棵子树剪掉，避免上一次的产物被再打一遍。

打包过程中通过 `pack-progress` 事件推送进度，阶段依次为 `scanning` → `cleaning` → `copying` / `zipping` → `done`。

### 代理（VPN）

底层只调 openconnect，不依赖 Cisco AnyConnect 客户端或 OpenConnect GUI。

连接器来源按平台区分：

- **Windows 只认安装包内置的 openconnect**（`src-tauri/resources/apps/openconnect/openconnect.exe` + 同目录 `wintun.dll`）。不扫描注册表、常见安装目录或 PATH，也不提供「指定外部 openconnect」入口，避免把来源不明的二进制拉进提权进程。内置副本缺失时直接报错并提示重装。
- **macOS 只向外找**，按「专用路径 > 标准前缀 > 记住的路径 > PATH」查找（不探测内置副本 —— 包里的内置 openconnect 只有 Windows 版，macOS 上跑不起来）。自动识别只覆盖标准前缀（Apple Silicon `/opt/homebrew/bin`、Intel `/usr/local/bin`，外加 `/opt/local/bin`）；`brew` 装在自定义前缀（例如 `~/hb`）时不会被自动识别，要用引导里的「选择安装目录」记住路径。反过来，**标准前缀里一旦出现能跑的 openconnect，就按系统默认处理，并直接清掉之前记住的路径** —— 界面上那行「已指定 openconnect + 路径」随之消失。留着旧记录只会在系统那份被卸载后悄悄顶上来，所以干脆不存：用户把 openconnect 从自定义前缀换到标准安装后点一次「检测」即可；将来系统那份没了，就回到安装引导让用户重新指定。
- **「选择安装目录」在 macOS/Linux 上不复制二进制、也不建软链**，只在 `<data>/vpn/openconnect-path.txt` 里记一行路径，执行时直接用原来的可执行文件：复制件会在 `brew upgrade` 后失配；复制还会先读用户目录里的文件，自定义前缀经常被权限/隐私策略挡住（现场报过 `复制 openconnect 失败: Permission denied (os error 13)`）；而数据目录里出现可执行文件/软链，又会被当成「程序自带的副本」（现场已经有人把它误认成 Windows 内置文件）。旧版本遗留的 `<data>/apps/openconnect/`（或 1.0.25 的 `connectors/openconnect/`）会在下次检测时自动清理：软链的真实目标转成路径记录，旧复制件直接删除（只靠复制件的机器会回到安装引导，重新选一次目录即可）。界面只让选目录、不让选文件：`bin` 目录、Cellar 里带版本号的 `9.21` 版本目录（里面有 `bin/openconnect`）、自定义前缀的 `bin` 都认，后端在目录里自己找可执行文件；Cellar 路径会改指到稳定的 `prefix/bin`。登记成功后在引导卡片下方回显记住的**绝对路径**和版本号（自定义前缀名字五花八门，只能靠这行确认认的是哪个文件），提示停留 6 秒；失败提示一律常驻，不自动消失，免得用户还没看清原因就没了。之后这个位置会长期显示在详情区顶部（`已指定 openconnect` + 路径 + 版本），旁边带「更改」（重新选目录登记）和「移除」（清掉登记、回到安装引导）两个文字按钮——指定错了不用重装或手动清数据目录；**系统默认路径不显示** —— 装好就有的东西摆出来只是噪音，只有手动指定的那份才需要在界面上认领。

判定可用性的方式是真正执行一次 `openconnect --version` 并解析版本号，而不是看文件存在与否 —— 解压后缺依赖 DLL 是很常见的情况，光看路径会把跑不起来的二进制当成可用。

**子进程统一钉死 C locale**（`process::apply_c_locale` / worker 入口的 `pin_child_locale`）：openconnect 9.x 会按系统语言翻译输出，中文机器上 `--version` 打的是「OpenConnect 版本 v9.21」，worker 靠的 `Login failed` / `CSTP connected` / `Set up tun device failed` 也会跟着变中文。终端里跑（`LANG=C.UTF-8`）一切正常，打包成 .app 从 Finder 启动时环境里没有 `LANG` / `LC_ALL`，gettext 退回系统语言，版本行解析不到英文前缀，界面就误报「未找到 openconnect，请先安装」——`which openconnect` 却指向 `/opt/homebrew/bin/openconnect`。所以凡是输出要被解析的子进程都显式带 `LC_ALL=C`（并删掉优先级更高的 `LANGUAGE`），版本号解析也不再依赖英文 `version` 单词；拉起用户的编辑器 / 终端这类可见程序不带这个环境，保留用户自己的语言设置。

支持的协议：`anyconnect`、`nc`、`gp`、`pulse`、`f5`、`fortinet`、`array`。

#### 规则分流

分流的配置单位是**规则集**，每个 VPN 配置绑定其中一个。

- 规则类型有三类：`DOMAIN`、`DOMAIN-SUFFIX`、`IP-CIDR`；出口只有 `DIRECT`（直连）与 `VPN` 两种。`IP-CIDR` 的值可以是单个 IP（自动按 `/32` 处理）或 CIDR 网段，例如 `192.168.6.115`、`192.168.6.0/24`
- 路由注入模式没有「按连接判定」这一步：`VPN` 规则只负责把命中的域名送进隧道，未命中的一律直连
- `DIRECT` 规则是**例外**：同一域名下取最具体的规则（后缀越长越具体；长度相同时 `DOMAIN` 比 `DOMAIN-SUFFIX` 具体）。所以 `DOMAIN-SUFFIX example.com → 走 VPN` 再加 `DOMAIN hlwapi.example.com → 直连`，就只有 hlwapi 不进隧道；规则在列表里的先后不影响判定。例外域名仍由本机转发器代答（答案来自 VPN DNS），只是不会被注入隧道路由
- `IP-CIDR` 不需要 DNS：连接完成就立刻把命中网段装成隧道路由，适合「内网网站只给 IP、没有域名」的场景。判定按最长前缀（`/24` 比 `/16` 具体），前缀相同时先出现的规则胜出；`DIRECT` 例外用更具体的直连路由表达（例如 `192.168.6.0/24 → VPN` 再加 `192.168.6.9/32 → DIRECT`，只有 `.9` 直连）
- 显式配置优先：规则里写了的网段一律按规则注入，不会因为「看起来像本机局域网」而自动跳过。如果规则恰好覆盖本机所在网段，那个网段的流量会进隧道（通常就访问不到了），断开连接时路由自动还原——所以别把本机局域网写成 `VPN`
- 历史 `IP-CIDR` 规则从本版本起生效；`DOMAIN` / `DOMAIN-SUFFIX` 里写 IP 的旧写法仍然不生效（弹窗会提示改用 IP 类型）
- 规则热更新不需要重连：改完点「完成」立刻重新布线。之前经本机转发器解析过的名字会按记下的 DNS 答案补注入 `/32`，所以即使应用缓存着旧答案、不再发查询，「改成走 VPN / 改成直连」也会立即生效
- **严格规则模式**：走 VPN 的唯一依据是规则。服务端下发的默认域名（`CISCO_DEF_DOMAIN`）与 split-include 网段**不会自动并入**，只在「当前生效的分流」里展示（仅展示、不生效），方便把「服务端说内网在哪」和「客户端实际接管了什么」对照起来看；规则为空时不接管任何流量
- 保存前校验规则语法，可以预览一条规则对指定「域名 / IP」的判定结果
- 右侧规则列表可编辑（点编辑按钮或双击整行，原地改，顺序不变）、可逐条启用/禁用（默认启用）；禁用的规则不参与分流但保留在列表里。编辑一条被禁用的规则并保存后，它会变成启用

三种代理模式，已连接时也能运行期切换（都是本地布线，点了就生效）：

- **规则模式（默认）**：不抓包、不抢默认路由、不改系统 DNS。命中「走 VPN」的域名交给 VPN DNS 解析，解析出来的内网 IP 装 `/32` 路由指向隧道；`IP-CIDR` 规则命中的网段直接装成隧道网段路由，不经过 DNS。默认路由和系统 DNS 都不动，结构上不可能「一连上就全网断」。服务端下发的内网范围只做展示（见上方「严格规则模式」），不参与布线。
  - **macOS**：写 `/etc/resolver/<域名>` + 本机 DNS 转发器，转发器把应答回给应用**之前**就装好 `/32`，没有时间差。转发器优先绑 `127.0.0.1:53`；53 被 Clash/dnsmasq 之类的 DNS 服务占用时，改绑 loopback 随机高位端口，并在 resolver 文件里用 `port` 字段指过去（现场撞过 `Address already in use`，直接失败会变成「只建隧道、不接管流量」）。
  - **Windows**：用 NRPT（`Add-DnsClientNrptRule`）把这些域名的解析交给本机 `127.0.0.1:53` 转发器（转发器在回包前装好 `/32`）；NRPT 指向 loopback 不生效时退回「NRPT 直连 VPN DNS + 后台解析」，再不行才退回「只给隧道网卡配 DNS」，每一步都会在日志里写明原因。回退路径下子域的首个请求可能失败一次，`/32` 补齐（约 3 秒）后重试即通。回退路径的缓存扫描同样会把看到的答案记进「DNS 答案账本」：热更新时 teardown 要清空 DNS Client 缓存，没有这份记账，删掉直连例外就又要等应用重新解析。
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

openconnect 探测失败时，代理页给出按平台区分的安装引导：macOS 明确要求 `brew` 装在标准前缀（`/opt/homebrew`，Intel 是 `/usr/local`）并给出可复制的安装命令与官方文档；Linux 给发行版安装命令；Windows 只说明内置副本缺失并提示覆盖安装，不引导下载外部二进制。已经装在自定义前缀里的用户走引导下方的「选择安装目录」，选中 openconnect 所在的目录（里面有 `bin/openconnect`）即可，不需要重装。

### 系统

左侧图标栏第四个工具（`⌘/Ctrl+4`），四个页签（弹窗拦截仅 Windows 可见）：

- **软件卸载**：Windows 读注册表 `Uninstall` 键（HKLM 64/32 位、HKCU），macOS 扫 `/Applications` 与 `~/Applications`；支持按名称/发布者/位置搜索，按名称、大小、安装日期排序（顺序记在本地）。行内「卸载」打开独立卸载窗口（820×560，macOS 隐藏标题、红绿灯浮在内容上；Windows 自绘标题栏，与主界面一致）：按「程序文件 / 支持文件 / 缓存文件」列出关联文件——程序文件 = 主 `.app` + Spotlight/兜底目录找到的同 id 副本，支持/缓存复用残留规则；带大小、版本、置信度：默认只勾缓存/日志/崩溃报告（可重建）和必须卸载的应用本体；设置、账号、插件配置等用户数据默认保留，点「彻底删除（含用户数据）」或手动勾选才会清理，low 只列出不勾选，底部「全选」可全选，应用本体始终必选。点「确认删除」先只读检测是否在运行：不在运行直接执行；在运行弹二次确认（进程数与进程名），点「强制退出」先按平台正常退出（macOS `NSRunningApplication.terminate()`、Windows `CloseMainWindow()`），3 秒内退不干净就逐进程强杀（macOS `SIGKILL`、Windows `taskkill /PID … /T /F`），每 250ms 重新检测并补杀重启出来的新进程，全部进程消失后才继续（总上限约 20 秒，退不干净即中止并提示）。随后先跑应用自带卸载程序（macOS 主 `.app` 移入废纸篓；MSI 的 `/I` 自动换 `/X`，需要时一次 UAC），卸载完成后自动再扫一次残留——仍有残留就回到勾选页（提示「卸载已完成。重新扫描到 N 项残留」，主按钮变「清理残留」），没有残留直接进结果页——再按本次扫描白名单删除勾选的关联文件：卸载程序已删掉的路径记「跳过」不算失败，需 root 的条目走一次合并授权；结果页给出删除/跳过/失败明细，失败项可在结果页「重试清理」，卸载/删除后主列表自动刷新。运行中的应用本体不出现在列表里，命令层也会再拦一次——不能卸载自己。
- **卸载 ws-workbench 自己**：macOS 安装包（DMG）里带「卸载 ws-workbench.command」。DMG 根目录另外还有「安装说明.txt」和「安装 ws-workbench.command」：双击后者会自动把 app 复制到 `/Applications`、执行 `sudo xattr -cr /Applications/ws-workbench.app` 清除隔离属性（未公证的包直接双击会被 Gatekeeper 拦成「已损坏」/「无法验证开发者」）并启动。卸载双击后先正常退出应用，退不掉会询问是否强制退出；**所有相关进程归零后才继续**，否则直接报「卸载失败」且不删除任何文件。继续后列出应用本体、数据目录、附件上传目录、剪贴板图片和 WebView 缓存，并弹出多选勾选：设置/数据库/剪贴板历史、附件目录、缓存/日志/偏好设置三类，默认**全部不勾（保留）**，勾选的类别二次确认后删除（附件/数据库删除后不可恢复）。自定义到应用数据目录之外的附件目录只提醒、不自动删；真实路径由应用启动/保存设置时写入 `uninstall-info.txt`（纯路径，不含密钥）。Windows 继续使用安装器自带的标准卸载器（设置 → 应用 → 卸载），安装包里的 NSIS 钩子会在删文件前检查 `ws-workbench.exe`（在运行就问是否强制退出，退不掉直接中止）、读取同一份 `uninstall-info.txt`，并调用 App 写在数据目录里的 `uninstall-choose-data.ps1` 显示同样的三类多选框；勾选结果写在 `%APPDATA%\ws-wb-uninstall-selection.txt`，NSIS 按结果删除、未勾选一律保留，自定义附件目录只提示不删；静默卸载/勾选脚本缺失/调用失败都按保留数据处理。
- **弹窗拦截**（仅 Windows）：火绒式的规则拦截。默认关闭，总开关持久化，开启后下次启动自动恢复；后台线程每 500ms `EnumWindows` 扫可见顶层窗口——**先匹配拦截规则**（进程必须一致，窗口类 / 标题子串可选，命中直接 `WM_CLOSE` / `SW_HIDE` 并记一条拦截记录），没有规则才走启发式打分（工具窗口 / 不抢焦点 / 始终置顶 / 无标题 / 尺寸异常 / 类名与标题广告线索 / 进程不在前台），可疑窗口进「可疑记录」等用户处理。对可疑弹窗点「拦截此类」会生成一条持久化规则（默认按「进程 + 窗口类」，同一个广告位换标题/素材也能拦；也可选「仅此弹窗」或「该程序全部」），之后同类弹窗出现即自动关闭；「从窗口添加」直接列出当前可见窗口挑选一个加规则，规则可停用 / 删除 / 立即执行，命中次数与最近命中时间落 `<data_dir>/popup-rules.json`。可选「智能拦截」（默认关）：没有命中规则时，按分数阈值 + 至少两条命中原因 + 系统保护表 + 忽略名单 + 每分钟上限自动处理，设置存 `popup-settings.json`；忽略名单（白名单）存 `popup-ignore.json`。界面用 exe 的版本信息取应用名和图标（取不到退回进程名 + 首字母色块）。不注入、不装驱动、不杀进程。
- **屏幕休眠**：阻止系统空闲睡眠（参考 Amphetamine 的公开电源断言方案），并支持「自动保持规则」（Triggers）：指定 App 运行时 / 接通电源时 / 连接外接显示器时自动保持，条件失效自动释放；手动开启优先，规则只释放自己开的会话。规则存 `<data_dir>/sleep-triggers.json`，后台 5 秒探测一次（`sysinfo` 进程列表、`pmset -g batt` / `GetSystemPowerStatus`、CoreGraphics 活动屏幕数 / `GetSystemMetrics(SM_CMONITORS)`），全部只读、不需要管理员权限。macOS 用 IOKit `IOPMAssertionCreateWithName`（`PreventUserIdleSystemSleep`，不勾「允许屏幕关闭」时再加 `PreventUserIdleDisplaySleep`，断言名 `ws-workbench Keep Awake`）；Windows 用专用线程持有 `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED [| ES_DISPLAY_REQUIRED])`。时长按会话式菜单选（参考 Amphetamine 的「开启新会话」）：不限时 / 5·10·15·20·25·30·45 分钟 / 1·2·3·4·6·8·12 小时，或自定义小时+分钟，点时长立即开始一个会话，到点自动释放（状态查询还有一道惰性回收）；不需要 root / 管理员，也不做登录级保活。
- **Hosts**：把系统 hosts 的完整内容直接读进等宽编辑器，想加什么行就加什么行，`⌘/Ctrl+S` 保存整份文件（保存前自动备份 + 一次系统授权，取消不会改动文件）。路径固定：macOS `/etc/hosts`，Windows `%SystemRoot%\System32\drivers\etc\hosts`；「备份 N」可展开最近 30 份备份并还原（路径见「打开目录」按钮的提示）。编辑器是 Monaco（VS Code 内核，本地打包、首次打开 Hosts 页签时懒加载）：行号对齐、注释 / IPv4 / IPv6 / 主机名高亮、非法地址标红，支持撤销重做、查找替换（`⌘/Ctrl+F`）、多光标与右键菜单。非 UTF-8 的 hosts 会拒绝编辑，避免写坏原有内容；CRLF 文件会保持原来的换行风格。
安全约束：普通删除只进废纸篓；需 root 的条目走一次提权批处理（`rm -rf` / `Remove-Item`，不经过废纸篓，删除前会明确提示）；所有路径过白名单校验，`/`、主目录、`/System`、`/usr`、`C:\Windows` 等硬拒绝；`pkgutil` 安装收据只读展示；系统应用（SIP / `com.apple.*`）不提供卸载。macOS 的废纸篓走 `NSFileManager` API，不会弹「控制 Finder」的自动化授权。弹窗拦截不做注入/驱动/进程结束；屏幕休眠不碰合盖与主动睡眠。

已知限制：Windows 侧只做了代码审查与交叉编译尝试（本机缺 MSVC SDK，第三方 `ring` 构建失败），未实机跑卸载/UAC/强制退出/弹窗检测；残留以精确匹配为主，深扫可能漏报（不认识的自定义目录名）也可能多报（通用名字的目录），所以 low 条目默认不勾选；强制退出会连同 bundle 内的 helper/XPC 一起 `SIGKILL`、可能丢掉未保存的数据（二次确认里已提示），仍有进程退不掉时卸载会中止并提示手动退出；卸载窗口的关联文件清单已过滤 `~/.Trash`，不会顺手清空废纸篓；弹窗检测是「显示后处理」（弹窗会闪一下），UIPI 下关不掉管理员窗口，UAC / 锁屏 / 网页内嵌广告看不到；macOS 登录项在未授权「自动化」时只读。

**托盘快捷面板**：右键点菜单栏/任务栏图标弹出柠檬式的无边框小面板（`quick.html`，360 宽，高度随当前页签内容 + 底部操作栏自动调整，失焦或再右键一次托盘图标收起），两个页签：

- **VPN**：当前状态与配置名、「连接/断开」、规则/全局/直连三种代理模式即切。
- **休眠**：一键保持唤醒 / 停止，15 分钟 / 30 分钟 / 1 小时 / 不限四档时长，状态点与剩余时间实时刷新（复用系统页「屏幕休眠」的同一套命令与 composable）。

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

### 设置

顶部「检查更新」卡片显示当前版本，只有查到更高版本时才多显示一行高亮的「最新版本」并出现「立即更新」（右上角「重新检测」会顺带检查一次更新）；下载/安装进度就地显示，下载中断后点「重试」会从断点续传（见「自动更新」）；下面显示运行环境（操作系统、架构、可执行文件后缀、是否需要提权、配置文件位置、各网卡的 MAC 与全部 IP —— 多 IP 逐条列出），以及附件实际落盘位置（可直接打开目录）。

「用编辑器打开项目」使用的应用查找优先级为专用路径 > 标准安装位置 > 记住的路径 > 内置资源 > PATH（Windows 上还有注册表，位置在标准安装位置之前）；VPN 连接器探测不走这条：Windows 只认内置副本，macOS 只向外找（专用路径 > 标准前缀 > 记住的路径 > PATH，内置副本里只有 Windows 版可执行文件，不参与探测）。其中「选择安装目录」入口只对 openconnect 开放（Windows 版用内置副本，不提供该入口）。macOS 与 Linux 下分别检查 `/Applications`、Homebrew 前缀以及 PATH 里的 `/usr/bin`、`/usr/local/bin`。

### 托盘

**左键单击**图标直接显示主界面（面板开着时先收起面板）；**右键单击**图标弹出快捷面板（VPN / 休眠两个页签，见上），面板底部固定「显示主界面 / 退出」两个入口。托盘不再挂原生右键菜单：面板本身就是原来的菜单入口，「检查更新」已经收进主窗口「设置」页的更新卡片。

点「退出」等于结束整个应用和它拉起的进程：先给 worker 写停止标记（`stop.flag`），然后主进程立刻退出，托盘和任务栏图标当场消失；worker 是独立进程，会自己杀掉 openconnect、还原路由和 DNS 后退出（见上面的 `main.pid` / 心跳兜底），因此不需要卡住界面等它收尾，也就不用承受「点了退出好几秒图标才消失」。唯一代价是：从解压目录直接运行便携版时，退出后立刻删目录可能会被 worker 短暂占用，等一两秒再删即可。关闭主窗口只是隐藏，应用继续在托盘里运行。

快捷面板是独立的透明无边框窗口：点面板外部（失焦）或再右键一次托盘图标收起，面板上没有关闭按钮，底部只有「显示主界面 / 退出」两个操作；窗口高度按「当前页签内容 + 底部操作栏」自适应，切换 VPN / 休眠会重新量一次，不会留固定高度那种底部空白。macOS 的透明窗口依赖 Tauri 的 `macos-private-api` feature（`Cargo.toml` + `tauri.conf.json` 的 `macOSPrivateApi`），**直装/DMG 渠道没问题，但用不了 Mac App Store**。

启动顺序也按「别让窗口先出来干等」来排：主窗口先以隐藏状态创建，前端首屏渲染完再调 `app_ready` 让它显示；托盘面板和剪贴板面板不在启动路径里创建，等主窗口就绪后 600ms 在主线程预热（真抢在预热前用，`ensure_*` 会现场建）。这样 Windows 冷启动时既不会出现还没画完就被系统标成「未响应」的白窗口，也少两个 WebView2 抢启动时间。兜底：前端 8 秒内没就绪也会把主窗口显示出来，不会只剩托盘图标。启动期间重复双击也不会再弹一个白窗口：单实例的第二份进程要等首屏就绪后才去激活已有窗口。

macOS 的应用菜单做了汉化，点 Dock 图标会把窗口重新显示出来。

## 日志

不做全局日志页：哪个模块产生日志，就显示在哪个模块里。VPN 的连接 / 分流日志在「代理」页右侧抽屉（含清空）；项目打包的进度与结果（含错误明细）在「项目」页底部。这样出问题时看到的内容和操作上下文绑在一起，也不用再维护一份全局日志缓冲。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Tauri 2（`tauri` 开启 `tray-icon`、`image-png`、`macos-private-api`） |
| 前端 | Vue 3.5 + TypeScript 5.6 + Vite 6；ant-design-vue 4 + Vditor 4（组件与图标按需自动引入） |
| 后端 | Rust edition 2021，库名 `ws_workbench_lib` |
| 数据面 | 路由注入（隧道 /32 + 按域名 DNS），不抓包 |
| 存储加密 | SM4-CBC + HMAC-SM3；主密钥由 macOS 钥匙串 / Windows DPAPI 托管 |
| 插件 | dialog、shell、store、updater、process；clipboard-manager（剪贴板历史）、global-shortcut（默认 `⌘⇧V` 呼出，可在管理页改） |
| 关键 crate | `ignore`、`zip`、`chrono`、`uuid`、`tempfile`、`sm4`、`hmac`、`cbc`、`nix`、`reqwest`、`minisign-verify` |

## 环境要求

- Node 22 与 pnpm（CI 固定 pnpm 10；`pnpm-lock.yaml` 为 9.0 格式）
- Rust stable 工具链
- 仅在需要 VPN 时：可用的 openconnect。Windows 必须使用 `src-tauri/resources/apps/openconnect/` 里的内置副本（安装包会一起打进去），macOS 用 `brew install openconnect` 装到标准前缀（`/opt/homebrew`，Intel 是 `/usr/local`）；装在自定义前缀里的用引导里的「选择安装目录」记住路径（只记路径，不复制文件）。mac 包里**不带**内置 openconnect（`resources/apps` 只在 Windows 包里，见「构建与发布」）
- VPN 连接需要管理员授权，见「权限与进程拆分」；两个平台都用路由注入做规则分流，不抓包，macOS 用系统 utun，Windows 用内置 `wintun.dll`

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

## 构建与发布

```bash
pnpm build                                  # vue-tsc --noEmit + vite build
pnpm exec tauri build                       # 打当前平台安装包
node scripts/release.mjs                    # 构建 + 发布
node scripts/release.mjs --no-publish       # 只构建，产物落在 release/
node scripts/release.mjs --dmg              # macOS 额外生成 dmg（Intel / Apple Silicon 各一份）
node scripts/release.mjs --target aarch64-apple-darwin --dmg   # Apple Silicon 版
node scripts/release.mjs --target x86_64-apple-darwin --dmg    # 在 Apple Silicon 上交叉编 Intel 版
node scripts/release.mjs                    # Windows 版直接在 Windows 主机上跑，产出 exe + msi
```

Windows 安装包（NSIS 的 `exe` 与 WiX 的 `msi`）必须在 Windows 主机上构建：Tauri 打 MSI 依赖 WiX Toolset，而 WiX 只有 Windows 可执行文件，交叉编译拿不到。

**安装包内容按平台裁剪**：`src-tauri/` 下两份平台配置都是 JSON Merge Patch（RFC 7396，`null` 即删除），Tauri 2 构建时自动和 `tauri.conf.json` 合并：`tauri.macos.conf.json` 把 `bundle.resources` 里的 `resources/apps` 去掉，所以 mac 包不含内置的 Windows openconnect 运行时（约 9 MB）；`tauri.windows.conf.json` 把 `resources/zh-Hans.lproj` 去掉，所以 Windows 包不带 macOS 的本地化资源（`resources/apps` 继续由 `tauri.conf.json` 打进 Windows 包）。VPN 探测逻辑也按平台拆文件：Windows 只探内置副本、不向外找；macOS 只向外找、不探内置（见上文「连接器来源」）。改一边不会把另一边的行为带偏。

Windows 安装器启动时，NSIS 会先把自解压内容写到 `%TEMP%`。如果 `%TEMP%` 不可写（受限环境、安全软件拦截、沙箱），会弹出 `NSIS Error: Error writing temporary file. Make sure your temp folder is valid.`——这是运行环境问题，不是安装包损坏。把 `TEMP` / `TMP` 指到一个可写目录再从资源管理器运行安装包即可；构建产物旁的 `安装（临时目录重定向）.cmd` 就是做这件事（把 `TEMP` 设到同目录 `install-tmp` 后启动安装器）。

还有一种更隐蔽、更容易被误判成「安装包坏了」的情况：安装包和程序放在**被运行沙箱/受限策略圈住的目录**里（例如把项目工作区限制成「只能写本目录」的环境）。表现是安装器能启动，但静默或秒退、什么提示都没有——因为它在解压到 `%TEMP%`、写 `%LOCALAPPDATA%` 时被拒绝；程序本身跑起来后，写 `%APPDATA%`、`~/.dsh` 也会报 `os error 5（拒绝访问）`。判断方法：把同一个 exe 复制到 `%TEMP%`、桌面、下载目录各跑一次，只有工作区里失败就是目录限制，跟代码签名、杀毒软件都无关。处理：**把安装包复制到工作区外的普通目录再运行**。

```bash
pnpm package:local                        # 复制最新安装包到 %USERPROFILE%\Downloads\ws-workbench-build，并打印 SHA-256
pnpm package:local -- --out D:\dist       # 指定输出目录（不能在仓库里）
pnpm package:local -- --open              # 复制完打开所在文件夹
```

发布脚本用到的环境变量：

| 变量 | 用途 |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | 更新签名私钥内容，不设时回退读 `~/.tauri/ws-workbench.key` |
| `GITHUB_TOKEN` | GitHub 令牌（CI 里由 Actions 注入），创建发行版、上传附件、推送更新清单必需 |
| `TAURI_BUNDLES=app` | 只打 app、跳过 Tauri 的 dmg 打包脚本，受限环境配 `--dmg` 使用 |

发布版本的唯一来源是 git tag：CI 把 `vX.Y.Z` 里的 `X.Y.Z` 通过 `tauri build --config` 注入，不改源文件。发布流程：打 tag → 构建（含更新签名）→ 上传安装包到 GitHub 发行版（同名 tag）→ 合并生成本平台的 `updates/latest.json` 条目并提交回 `main` → 串行跑一次 Gitee 镜像（`gitee-sync` job），把三个更新载荷传到 Gitee 发行版并把清单里的下载地址换成 Gitee。客户端先从 Gitee 读清单，读不到再用 GitHub 清单兜底。

```bash
git tag v1.0.16
git push origin v1.0.16   # 只有推 tag 才会触发 GitHub 构建
```

### CI

`.github/workflows/ci.yml` 是常驻护栏：push 到 `main`、pull request 与手动 `workflow_dispatch` 都会跑，`test-macos` 和 `test-windows` 两个 job 分别在 `macos-latest`、`windows-latest` 上原生编译，各自 `pnpm build` + 在 `src-tauri` 下 `cargo test --lib`，防止改坏对面平台的代码合进来。两个 job 都先构建前端：tauri 的 `generate_context!` 在编译期要求 `frontendDist`（`../dist`）存在。发版仍只由 tag 触发 `release.yml`。

`.github/workflows/release.yml` 只在推送 `v*` tag 时触发（直接 push `main` 不会构建），三个 job 并行，产出：

| 平台 | 自动更新载荷（带签名） | 手工安装包 |
| --- | --- | --- |
| macOS Apple Silicon | `ws-workbench_aarch64.app.tar.gz` | `ws-workbench_<ver>_aarch64.dmg` |
| macOS Intel | `ws-workbench_x86_64.app.tar.gz` | `ws-workbench_<ver>_x64.dmg` |
| Windows x64 | `ws-workbench-setup.exe`（NSIS） | `ws-workbench_<ver>_x64_en-US.msi` |

macOS 两个架构都在 `macos-latest`（arm64）上跑，Intel 版用 `--target x86_64-apple-darwin` 交叉编译；Windows 在 `windows-latest` 上原生构建（MSI 需要 WiX）。版本号直接从 tag 解析，因此 tag 必须是 `vX.Y.Z` 形式，否则第一个预检步骤就会失败。

跑之前必须配一个仓库密钥：`TAURI_SIGNING_PRIVATE_KEY`（本机 `~/.tauri/ws-workbench.key` 的全文），没配会在预检步骤直接退出。

两个构建 job 会同时改 `updates/latest.json`，脚本用「快进到远端最新 → 以远端为基线合并本平台条目 → 推送被拒就撤销重试」来避免互相覆盖。CI 自己推回 `main` 的清单提交带 `[skip ci]`，且工作流只由 tag 触发，不会自触发。

三个构建 job 之后还有一个串行的 `gitee-sync` job（`needs: [macos, windows]`）：`scripts/gitee-sync.mjs` 把三份更新载荷**和手工安装包（两个 dmg + msi）**传到 Gitee 同名 tag 的发行版，再把 Gitee 仓库里和 GitHub `main` 上的两份 `updates/latest.json` 的下载地址都改成 Gitee。dmg / msi 不在更新清单里，只是给国内用户手动下载安装用，缺了只告警、不影响发版。它需要：

- 仓库密钥 `GITEE_TOKEN`：Gitee → 设置 → 私人令牌，勾选 `projects`（仓库读写）权限
- 一个**公开**的 Gitee 仓库：默认 `feng_wenshan/ws-workbench`，可用仓库变量 `GITEE_REPO` 覆盖；清单分支默认取仓库的默认分支（Gitee 新建仓库默认是 `master`），可用 `GITEE_BRANCH` 覆盖。**客户端端点里的分支名必须和它一致**

Gitee 仓库只需要建好、勾选「使用 Readme 文件初始化」保证默认分支存在（代码不用镜像过去）；第一次发版时脚本会自动建 tag 和发行版。`GITEE_TOKEN` 没配、仓库是私有的、或附件上传失败时，这个 job 会失败并说明原因，但 GitHub 侧的发行版与清单已经发布，不影响原有流程（清单里失败的平台保留 GitHub 地址）。清单里的 `signature` 不随镜像改动（只改 `url`），Gitee 上的包和 GitHub 上的包是同一份构建产物，插件用同一把 minisign 公钥校验。

还没配好 Gitee 时发布不受影响：Gitee 那一步会失败（GitHub 侧照旧）。配好之后可以在 Actions 里手动跑一次 **Gitee Backfill**（`.github/workflows/gitee-backfill.yml`，`workflow_dispatch`）：它按当前 `updates/latest.json` 的版本从 GitHub 发行版下载三个更新载荷和手工安装包补传 Gitee（加 `--skip-existing`，已经传好的附件按大小跳过），并把清单地址改成 Gitee——已经在用旧版本的客户端下次检查更新就能直接从 Gitee 下包，不用等下一个版本；手工安装包也能从 Gitee 发行版里直接下。

本地也可以手动补跑某次发版（需要 `GITEE_TOKEN`）：

```bash
node scripts/gitee-sync.mjs --version 1.0.19 --dir release --dry-run   # 只看计划
GITEE_TOKEN=xxx node scripts/gitee-sync.mjs --version 1.0.19 --dir release
```

## 自动更新

更新端点按顺序尝试（`src-tauri/tauri.conf.json` 的 `plugins.updater.endpoints`）：

1. `https://gitee.com/feng_wenshan/ws-workbench/raw/master/updates/latest.json`（Gitee 镜像，国内优先）
2. `https://raw.githubusercontent.com/fengwenshan/ws-workbench/main/updates/latest.json`（GitHub 清单兜底）

Gitee 仓库的默认分支可能是 `master`（Gitee 新建仓库默认）或 `main`，所以 Gitee 的 `master`、`main` 两个 raw 地址都列上了：不存在的那个分支直接 404，插件会顺延到下一个（客户端清单只会写在仓库的默认分支上）。

`check()` 带 10 秒超时（`src/composables/useUpdater.ts` 的 `CHECK_TIMEOUT_MS`）：第一个端点超时或 404 才会切到下一个，否则第一端点被网络卡住时会一直挂在「检查中」。更新包用 minisign 公钥（指纹 `9CB4E4B1ECEF3353`）校验签名；清单里的下载地址指向哪台机器都可以（清单本身不签名，换镜像不用重新签包）。

`updates/latest.json` 的结构：

```json
{
  "version": "1.0.17",
  "notes": "ws-workbench v1.0.17",
  "pub_date": "2026-11-09T08:20:35Z",
  "platforms": {
    "windows-x86_64": { "signature": "...", "url": "https://gitee.com/feng_wenshan/ws-workbench/releases/download/v1.0.17/ws-workbench-setup.exe" },
    "darwin-aarch64": { "signature": "...", "url": "https://gitee.com/feng_wenshan/ws-workbench/releases/download/v1.0.17/ws-workbench_aarch64.app.tar.gz" },
    "darwin-x86_64": { "signature": "...", "url": "https://gitee.com/feng_wenshan/ws-workbench/releases/download/v1.0.17/ws-workbench_x86_64.app.tar.gz" }
  }
}
```

镜像发布后清单里的 `url` 指向 Gitee；某个平台镜像失败时保留 GitHub 地址。

> `updates/latest.json` 由发布脚本生成；仓库里这份仍是旧记录，下次发版会按平台条目重写。

客户端启动时静默校验一次，之后每天 10:00 与 16:00 各校验一次；打开「设置 → 检查更新」可以看到当前版本 / 最新版本，点右上角「重新检测」立刻检查一次（进设置页时若距上次检查超过 5 分钟也会静默补查一次）。网络不通导致检查失败时，设置页卡片会直接把失败原因显示出来（静默补查失败也显示，不会悄悄吞掉）。平时标题栏不显示更新入口，只有发现新版本时才在原位置显示「更新」按钮，点它下载安装（装好后按钮变成「重启生效」）。不弹横幅、不额外提示。

安装包下载走 `src-tauri/src/updates.rs` 的自研断点续传，不用插件自带的 `Update::download`：插件把整包读进内存，网络中断后点重试只能从 0 开始。这里按 HTTP `Range` 把数据直接写进 `<app_cache_dir>/updater/<版本>-<下载URL哈希>/update.part`，失败保留半成品，下次同版本重试从断点继续（服务端不支持 Range 时退化为整包重下）；连接 30 秒建不起来、或 120 秒收不到数据就报错，避免界面卡在某个百分比不动。下完用清单里的 minisign 签名校验，通过才落成 `update.pkg` 并交给插件的 `Update::install` 安装。缓存目录把下载 URL 也算进去（同版本换镜像时不会把两个源的半成品拼在一起），安装成功后会清掉其他版本的缓存。失败后点「重试」会先重新确认服务端版本：失败期间如果已经发了更高版本，就改下最新的（不复用旧版本的 `.part`）；只有这次确认也失败（网络不通）时才继续从原断点续传。开发构建允许检查版本但不提供安装入口（避免覆盖 `target/debug` 的 dev 可执行文件），Rust 侧安装命令也会拒绝。

更新与 VPN 的边界：点「更新」时如果 VPN 正在连接 / 已连接，先弹确认框，确认后**先下载安装包、再断开 VPN、最后才安装**——顺序不能反。原因是安装阶段 worker 必须已经退出：Windows 上 worker 和主程序是同一个 `ws-workbench.exe`，它没退时安装包替换不了文件（而且 worker 是提权的，安装程序未必收得掉）；macOS 上旧 worker 还活着的话，更新重启后新界面显示「未连接」，但隧道和路由还在旧进程那边。断开走 `vpn_prepare_update`：写停止标记后最多等 10 秒让 worker 杀 openconnect、还原路由和 DNS，超时直接取消这次更新并提示手动断开，绝不把安装做到一半。VPN 没连时这一步直接放行。

## 配置与数据

业务数据（分组、项目、笔记、附件元信息、VPN 配置、规则集）存在应用私有目录的 `ws-workbench.db`（SQLite）中：表里只留 `id`、`group_id`、`sort_order` 等少量明文列，每行内容序列化成 JSON 后用 SM4 信封加密，存进 `payload` 列。一级项目的 `group_id` 记为空串（列本身 NOT NULL，不需要改表结构）；分组与一级项目的混排顺序存在 `config.json` 的 `tree_order`（一级 id 序列）里。`AppConfig` 与前端命令保持不变，加载时从多表组装，保存时与内存快照做 diff、事务内只写变化的行。

应用设置（layui 项目打包规则、上次选中的 VPN 配置、默认上传目录、worker 会话目录）仍存在 `config.json`（`tauri-plugin-store`，键名 `app_config`），**整份加密**：SM4-CBC + 随机 IV + HMAC-SM3，信封形如 `v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`。主密钥（32 字节）首次启动时随机生成，macOS 存系统钥匙串（`ws-workbench-vault` / `ws-workbench`），Windows 用 DPAPI 加密后写进 `<app_data_dir>/.vault_key`。

前端写 localStorage 也一律走 `safeStorage`：值经 `vault_encrypt` 变成密文再落盘。

数据库与设置文件统一放在「标准数据仓库」：优先 `app_data_dir`（macOS `~/Library/Application Support/com.ws.workbench`，Windows `%APPDATA%\com.ws.workbench`），不可写时按 `app_config_dir` → 主目录点目录 → `cwd/.config`（开发兜底）逐级回退，全部失败则退回进程内存，此时重启会丢数据。

旧版本的数据目录（产品名目录 `ws-workbench`、`~/.ws-workbench`、`%LOCALAPPDATA%\ws-workbench`）由 `storage` 模块在启动时自动识别并迁移：只搬 `config.json`、`ws-workbench.db`、`attachments/`、`clipboard-attachments/`、`vpn/` 这些业务数据，目标已有同名数据时不覆盖，搬空后删除旧目录；平台差异只在旧目录候选（`storage/macos.rs` / `storage/windows.rs`），搬运逻辑共享。

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
| `main.pid` | 主进程 PID，worker 每个循环检查一次，界面进程被强杀时秒级收尾 |
| `main.alive` | 父进程心跳，`main.pid` 拿不到时的兜底（超过 10 秒没刷新就判主进程已死） |
| `script/` | 生成的 vpnc-script |

超时与轮询节奏：连接超时 120 秒，守护周期 15 秒，状态轮询 600 毫秒。worker 退出码 `0` 正常、`2` 无请求、`3` 失败。

主进程被强杀（任务管理器结束任务、崩溃）时，worker 会在 200 毫秒内从 `main.pid` 发现父进程消失，按正常流程结束 openconnect、还原路由与 DNS 后退出；`main.alive` 心跳过期是兜底路径（约 12 秒）。如果连 worker 一起被杀，Windows 上 openconnect 挂在带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的作业对象里，worker 句柄一关就会被系统结束，隧道不会留在后台（残留的路由/NRPT 规则会在下次连接时由 `cleanup_stale` 清掉）。

## 平台与限制

| 项 | Windows | macOS |
| --- | --- | --- |
| VPN 提权 | 计划任务免 UAC，失败回退 UAC 弹窗 | 系统授权框 |
| 虚拟网卡 | Wintun 驱动 | 系统 utun |
| 分流实现 | 路由注入：NRPT 按域名解析 + 隧道路由（不抓包） | 路由注入：`/etc/resolver` + 本机转发器 + 隧道路由（不抓包） |
| 按域名解析 | NRPT（`Add-DnsClientNrptRule`） | `/etc/resolver` + 本机转发器（53 优先，被占用则随机高位端口 + `port` 字段） |
| 路由查询 | PowerShell JSON（`Get-NetRoute` 等） | `route` / `ifconfig` / `netstat` |

其余已知限制：

- 打包只支持 Layui 项目
- 相对路径含空格的文件会被跳过
- 分流只处理 IPv4：域名解析可能返回 AAAA，IPv6 目标不进隧道、仍走系统默认路由
- 域名分流只覆盖系统解析器（getaddrinfo）：自带 DoH/DoT 的应用不受控
- Windows 上按域名解析出来的 `/32` 由本机转发器在回包前注入；NRPT 退回旧路径时最多晚约 3~15 秒生效。服务端下发的内网范围（默认域名 / split-include）只展示、不参与分流，所以不会再有「服务端一推就接管」的隐式路由
- 规则与代理模式的改动都会热生效（worker 轮询 `rules.json` / `mode` 后重新布线），不需要断开重连
- 系统清理的 Windows 运行时行为（注册表枚举、UAC 卸载、工作集/待机列表）只做了交叉编译检查，需按设计文档第 9 节的 Windows 清单实机验收；安装包的 NSIS 卸载钩子已实现并通过 `makensis` 编译检查（含新的 PowerShell 三类勾选界面），进程未退出则中止、勾选删除/保留数据、自定义附件目录提示仍需在 Windows 实机走一遍（计划见 `docs/superpowers/plans/2026-11-10-installer-uninstall.md` 的 Phase W）
- `~/.Trash` 内容受 TCC 保护，删除结果以「原路径消失 + trash API 成功」为准
- 卸载残留只做精确匹配（bundle id / 注册表键 / 精确应用名），存在非标准目录的数据需要手动清理
- 托盘快捷面板的原生弹出（真实点菜单栏图标）受 macOS 辅助功能/屏幕录制权限限制，自动化测试只覆盖了页面渲染、位置计算与 `cargo check`，首次使用请手动点一下确认弹窗位置与失焦收起
- 快捷面板的透明圆角窗口在 macOS 上依赖 `macos-private-api`，因此不支持 Mac App Store 渠道
- 剪贴板历史在 macOS / Windows 启用（Linux 不启用）；历史加密落 SQLite、图片附件单独密文存放，**不联网**；默认快捷键在应用运行期间全局生效，会占用其它 app 的同款组合，可在「系统 → 剪贴板」齿轮里改。富文本回贴是尽力而为：依赖 source app 是否写入 HTML、目标 app 是否接受 HTML，图片过多或过大时可能降级成纯文本
- Hosts 在线编辑需要管理员授权；Windows 的提权写入 / UAC 取消 / DNS flush 只做了代码审查，需实机验收；非 UTF-8 的 hosts 文件会被拒绝编辑；没有语法校验，写错条目需自己负责（可随时从备份还原）
- 更新包的断点续传只在「同版本 + 同下载 URL」内有效；服务端不支持 Range 时会退化为整包重下，系统清掉应用缓存目录后重试也会从 0 开始。Windows 上的「下载 → 退出 → 安装 → 重启」全链路只做了代码审查，需实机验收

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
    quick.rs       托盘快捷面板：隐藏、按页签内容调整高度、底部「显示主界面 / 退出」
    clipboard.rs   剪贴板历史命令层：changeCount 采集 / 运行时状态 / 面板显隐
    hosts.rs       Hosts 在线编辑：读取 / 保存 / 备份列表 / 还原（整文件写入）
  util/            通用能力层
    fs.rs          目录创建、文本读写、可写性探测
    time.rs        本地 RFC3339 / UTC 毫秒时间戳
  config.rs        配置读写：整份加密、写入目录多级回退、历史字段迁移
  crypto.rs        字符串密文信封（SM4-CBC + HMAC-SM3）与主密钥托管
  filecrypto.rs    附件流式加密容器 WST1
  packer/          打包：遍历、包含规则过滤、复制、流式 ZIP；svn_platform/ 按平台定位 svn
  updates.rs       更新包断点续传下载 + minisign 校验（安装交给 updater 插件）
  models.rs        AppConfig / 分组 / 项目 / 附件元信息 / 默认包含规则
  platform.rs      操作系统判定
  netinfo.rs       本机网卡 MAC / IP（Windows 走 PowerShell，macOS/Linux 走 getifaddrs）
  process.rs/winps.rs  进程管理与 PowerShell 调用
  apps/            外部应用定位（finder / registry）
  clipboard/       剪贴板历史：mod.rs 共享采集/分类/快捷键/面板门面，
                   macos.rs / windows.rs / unsupported.rs 平台实现；
                   rich.rs（HTML 图文抽取 / 还原 / 哈希 / 缩略图）、store.rs（SQLite 读写与附件路径）
  hosts/           Hosts 在线编辑：内容校验（mod.rs）、备份（store.rs）、
                   提权写入在 file/ 下按平台拆（scripts.rs 放纯脚本文本生成）
  cleaner/         系统工具：安装列表/卸载/关联文件/残留扫描 + 屏幕休眠、弹窗拦截的后端
    related.rs     卸载窗口关联文件清单组装（分组 / 默认勾选 / 使用情况文案）
    installer_macos/ / installer_windows/  卸载实现，按平台拆
    running/       运行检测、优雅退出与强制退出（macos.rs / windows.rs 各一份）
    sleep/         防休眠（macos.rs IOKit 断言 / windows.rs SetThreadExecutionState；triggers/ 同结构）
    trash_guard/   删除安全层（白名单 + 废纸篓 + 提权删除），平台实现已拆
    popup.rs       弹窗拦截（仅 Windows：规则匹配 + EnumWindows 轮询 + 启发式打分）
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
    platform/      连接器探测，按平台拆文件（mod.rs 只做 cfg 分派）
      common.rs    共享：执行 `--version` 探测、版本解析、探测结果模型
      macos.rs     macOS：向外查找、导入（记住路径）、清理旧副本
      windows.rs   Windows：只认安装包内置副本、拒绝导入
    elevate.rs     提权启动
    guide.rs       openconnect 安装引导（Windows 只提示重装内置副本）
    session.rs     会话目录 IPC
src/
  components/      ProjectList / GroupDetail / ProjectDetail / ProjectConfig /
                   NoteEditor / AttachmentPanel / PackProgress / ExcludeRules / LogDrawer（VPN 日志抽屉复用）
    vpn/           VpnPanel / VpnProfileDialog / VpnRuleDialog / VpnRoutePreview /
                   VpnStatusDot / VpnLogDrawer / VpnInstallGuide
    settings/      SettingsPanel
    cleaner/       CleanerPanel / InstalledAppsPanel / MemoryPanel / AppIcon /
                   PopupPanel / SleepPanel / StartupPanel / DefaultAppsPanel
    hosts/         HostsEditor（整文件在线编辑 + 备份还原，编辑区是 Monaco）
    clipboard/     ClipboardManager（「系统 → 剪贴板」管理页签）
  composables/     useConfig / usePack / useVpn / useCleaner / useApps / useUpdater /
                   useWorkbenchLayout / useEntitySelection / useProjectForm /
                   useDeleteConfirm / safeStorage / useKeepAwake / useStartup / usePopup /
                   useDefaultApps / useHosts / useClipboardHistory
  api/             后端命令的类型化封装（config / project / system / attachments / apps / cleaner /
                   sleep / startup / popup / defaultapps / clipboard / hosts）
  editor/          Monaco 集成（hosts 语言 / 主题 / worker，首次打开 Hosts 页签才加载）
  utils/           通用小工具（time / dialog / path / error / hostsTokens / note-text /
                   note-assets / note-clipboard）
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
