# ws-tools

基于 Tauri 2 的桌面工具，把日常交付里的三件事收在一个窗口：项目打包、VPN 连接与分流、运行环境自检。界面是一级三标签 —— 项目打包 / VPN / 设置；关闭窗口不退出应用，常驻系统托盘。

## 功能

### 项目打包

先识别项目类型（Layui / Vue2 / Vue3 / React / SpringBoot 等）并读出 Git 或 SVN 的远端地址，再决定能不能打包。

打包只对 Layui 项目开放：

```ts
// src/components/ProjectConfig.vue
const canPack = computed(() => projectType.value === 'layui')
```

其他类型不渲染打包按钮；识别为未知类型时提示「未识别到 Layui 项目特征，打包功能不可用。请确认源目录选择正确。」

产出方式有两种：

- **开始打包**：逐个复制到输出目录，复制前会清空输出目录
- **打包 ZIP**：流式 Deflate 直接写 zip，不落中间产物；zip 内会多包一层输出目录名，条目名按 UTF-8 写入并置位 bit 11

排除规则走 gitignore 语法，默认规则集如下，用户可在「排除规则」里追加：

```
.DS_Store   *.md        .git      .git/**     .svn      .svn/**
.idea       .idea/**    .vscode   .vscode/**  .claude   .claude/**
.trae       .trae/**    *.scss    *.less      .gitignore
node_modules  node_modules/**   *.zip
```

规则集带版本号（当前 `DEFAULT_EXCLUDE_VERSION = 2`），老配置在加载时自动补齐 v2 新增的 `.*` 与 `.*/**`，不会重复添加已有规则。

遍历由 `ignore::WalkBuilder` 完成，标准过滤全部关闭、不跟随符号链接；项目自定义规则以 `!pattern` 形式叠加。另外相对路径中任一段含空格的文件会被跳过，注意这类文件不会出现在产物里。输出目录与 zip 自身通过 `extra_excludes` 排除，避免上一次的产物被再打一遍。

打包过程中通过 `pack-progress` 事件推送进度，阶段依次为 `scanning` → `cleaning` → `copying` / `zipping` → `done`。

### VPN

底层只调 openconnect，不依赖 Cisco AnyConnect 客户端或 OpenConnect GUI。

连接器按优先级查找：手动指定 > 已导入副本 > 内置资源 > 注册表（仅 Windows）> 常见安装目录 > PATH。判定可用性的方式是真正执行一次 `openconnect --version` 并解析版本号，而不是看文件存在与否 —— Wintun 版解压后缺依赖 DLL 是很常见的情况，光看路径会把跑不起来的二进制当成可用。

支持的协议：`anyconnect`、`nc`、`gp`、`pulse`、`f5`、`fortinet`、`array`。

两种分流模式：

| 模式 | 默认走向 | 例外规则 |
| --- | --- | --- |
| 以客户网络为主 `customer_first` | 默认走本地，只有命中的网段走 VPN | include 规则 |
| 以公司网络为主 `company_first` | 默认走 VPN，本地子网与排除网段走本地 | exclude 规则 |

路由做最长前缀匹配后下发。切换模式不会热生效，需要断开重连。

#### 权限与进程拆分

建虚拟网卡、改路由表都要管理员权限，但让 GUI 常驻管理员既危险又每次启动弹窗，因此拆成两个进程：

- 主进程（普通权限）读配置、探测连接器、把请求写进会话目录、拉起 worker、轮询状态文件
- worker（管理员权限）独立进程，负责建隧道、按分流模式下发路由、守护、退出时回滚

提权方式按平台区分：

- Windows：`schtasks /Create /RL HIGHEST /F` 注册计划任务 `WsToolsVpnWorker`，之后免 UAC；注册失败时回退到 `Start-Process -Verb RunAs`，弹一次系统授权框
- macOS：`osascript ... with administrator privileges`

两个进程之间没有管道可用（提权进程拿不到父进程的句柄），IPC 全部靠会话目录里的文件。

#### 路由与脚本

下发路由不解析 `netsh` / `route print` 的文本输出，本地化后那套输出会解析失败。Windows 侧用 JScript（cscript）执行脚本配合 PowerShell 的 `Get-NetRoute` / `Get-NetIPAddress` / `Get-NetAdapter` 拿 JSON；macOS 侧用 `route` / `ifconfig` / `netstat`。

生成的 vpnc-script 必须是纯 ASCII：Windows 那份由 cscript 按 ANSI 代码页解码，混入非 ASCII 字节会直接编译失败。脚本里用 `WS-TOOLS` 作为标记行，清理时据此定位自己写入的条目。

Windows 上的虚拟网卡由 Wintun 驱动提供（`src-tauri/resources/apps/openconnect/wintun.dll`），macOS 用系统自带的 utun。

#### 学习模式

只拿到域名、拿不到网段时用它反推：worker 每 15 秒采样一次 `Get-NetTCPConnection -State Established`，记录实际连通的远端地址，再聚合成候选网段交给用户判断。

- 仅 Windows 可用，采样连续失败 3 次后自动停用
- 按 `/24` 聚合；同一个 `/16` 内观察到至少 4 个 `/24` 才升级为 `/16`，同一个 `/8` 内观察到至少 4 个 `/16` 才升级为 `/8`
- 单次上限：2000 个主机、每主机 8 个端口、8 个时间样本、4 个进程
- **只观察、只建议**：候选网段要点「采纳」才会写进规则表，程序不会自己改规则

摘要字段写进 `status.json`（前端每 600 毫秒轮询，不能塞几十 KB 的明细），完整数据单独落 `learn.json`，只在展开明细时按需读取。清空走标志文件 `learn.clear`，worker 重置内存中的报告后删掉这个文件，否则下一轮 15 秒采样会把内容原样写回。

#### 凭据

密码不在命令行里传（进程列表可见），走 stdin 交给 openconnect 的 `--passwd-on-stdin`。落盘前加密：

- Windows：DPAPI，密文形如 `dpapi:<hex>`，解密时带 `CRYPTPROTECT_UI_FORBIDDEN`
- macOS：钥匙串，密文形如 `keychain:<key>`，服务名 `ws-tools-vpn-<key>`

### 设置

显示运行环境（操作系统、架构、可执行文件后缀、是否需要提权、配置文件位置），以及外部启动文件的定位结果：VS Code、IDEA、Trae、openconnect。查找优先级为手动指定 > 已导入副本 > 内置资源 > 注册表 > PATH。

手动指定路径仅 Windows 提供；导入副本只对 openconnect 开放。macOS 与 Linux 下分别检查 `/Applications`、Homebrew 前缀以及 PATH 里的 `/usr/bin`、`/usr/local/bin`。

### 托盘

菜单项：显示主窗口 / 连接 VPN / 断开 VPN / 切换分流模式 / 退出。左键单击图标直接打开窗口，菜单只在右键时出现。点「退出」会先请求 worker 收尾再结束进程，否则可能留下没人回收的路由。关闭主窗口只是隐藏，应用继续在托盘里运行。

macOS 的应用菜单做了汉化，点 Dock 图标会把窗口重新显示出来。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Tauri 2（`tauri` 开启 `tray-icon`） |
| 前端 | Vue 3.5 + TypeScript 5.6 + Vite 6 |
| 后端 | Rust edition 2021，库名 `ws_tools_lib` |
| 插件 | dialog、shell、store、updater、process |
| 关键 crate | `ignore`、`walkdir`、`zip`、`chrono`、`uuid`、`tempfile`、`serde` |

## 环境要求

- Node 22 与 pnpm（`package.json` 声明 `packageManager: pnpm@12.5.1`，CI 用 pnpm 10）
- Rust stable 工具链
- 仅在需要 VPN 时：可用的 openconnect。Windows 直接用 `src-tauri/resources/apps/openconnect/` 里的内置副本，macOS 建议 `brew install openconnect`
- VPN 连接需要管理员授权，见上文「权限与进程拆分」

## 开发

```bash
pnpm install          # 安装前端依赖
pnpm dev              # 只起前端，http://localhost:8790
pnpm exec tauri dev   # 起完整桌面应用
```

Vite 固定在 8790 端口并开启 `strictPort`，端口被占用会直接失败而不是换端口；`@` 别名指向 `./src`，监听忽略 `src-tauri/`。

注意别用 `pnpm tauri dev`。`package.json` 里有一个名为 `tauri` 的脚本（内容是 `tauri dev`），`pnpm tauri dev` 会被解析成 `pnpm run tauri dev`，最终执行 `tauri dev dev` 直接落到开发模式。

### 重启

改完代码要重启时用 `scripts/dev-restart.ps1`，它会先结束残留进程再拉起一份：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\dev-restart.ps1
```

脚本按仓库路径匹配进程命令行，只结束本仓库的 `ws-tools`、vite、tauri 进程，不误伤其他项目；检测到 `openconnect` 仍在运行会直接退出，避免留下无人回收的路由。

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
| `TAURI_SIGNING_PRIVATE_KEY` | 更新签名私钥内容，不设时回退读 `~/.tauri/ws-tools.key` |
| `GITEE_TOKEN` | Gitee 私人令牌（权限含 projects），创建发行版与上传附件必需 |
| `GITHUB_TOKEN` | 把安装包镜像到 GitHub 发行版，不设则跳过镜像 |
| `TAURI_BUNDLES=app` | 只打 app、跳过 Tauri 的 dmg 打包脚本，受限环境配 `--dmg` 使用 |

版本的唯一来源是 `src-tauri/tauri.conf.json` 的 `version`。发布流程：构建（含更新签名）→ 上传安装包到 Gitee 发行版（tag 为 `v{version}`）→ 合并生成本平台的 `updates/latest.json` 条目并提交。安装包会同时镜像到 GitHub 发行版，但清单里的 `url` 始终指向 Gitee，因为已装客户端读的就是这个地址。发布只推提交，不打本地 git tag。

仓库里没有版本号手工递增这一步：CI 取 `tauri.conf.json` 的 major.minor，patch 位用 GitHub 运行序号，即 `1.0.0` → `1.0.<run_number>`。

### CI

`.github/workflows/release.yml` 在推送到 `main` 或手动 `workflow_dispatch` 时触发，macOS 与 Windows 两个 job 并行，都跑在 `macos-14` 上；Windows 侧是交叉编译（`cargo-xwin` + `nsis` + `llvm`）。

跑之前必须配两个仓库密钥：`TAURI_SIGNING_PRIVATE_KEY` 和 `GITEE_TOKEN`，缺任意一个都会在预检步骤直接退出。

两个 job 会同时改 `updates/latest.json`，脚本用「快进到远端最新 → 以远端为基线合并本平台条目 → 推送被拒就撤销重试」来避免互相覆盖。CI 自己推的清单提交带 `[skip ci]`，job 上还有一层守卫，不会自触发。

## 自动更新

更新端点为 `https://gitee.com/feng_wenshan/ws-tools/raw/main/updates/latest.json`，用 minisign 公钥（指纹 `9CB4E4B1ECEF3353`）校验签名。之所以只走 Gitee：GitHub 的 `raw.githubusercontent.com` 在目标网络下被拦，把地址换过去已装客户端会取不到更新。

`updates/latest.json` 的结构：

```json
{
  "version": "1.0.14",
  "notes": "ws-tools v1.0.14",
  "pub_date": "2026-09-17T15:38:15Z",
  "platforms": {
    "windows-x86_64": { "signature": "...", "url": "https://gitee.com/.../ws-tools-setup.exe" },
    "darwin-aarch64": { "signature": "...", "url": "https://gitee.com/.../ws-tools.app.tar.gz" }
  }
}
```

客户端每天 10:00 与 15:00 各校验一次；用户选择忽略的版本记在 localStorage 的 `ws-tools:dismissed-update-version` 下，不再重复提示。

## 配置与数据

配置走 `tauri-plugin-store`，文件 `config.json`，键名 `app_config`。Store 不可用时按以下顺序找可写目录，逐个做真实写入探测：`app_data_dir` → `app_config_dir` → `~/.ws-tools` → `$HOME/.ws-tools` → `$TMPDIR/ws-tools` → `/tmp/ws-tools` → `cwd/.config`；全部失败则退回进程内存，此时重启会丢数据。

加载时的迁移：旧版顶层 `projects` 字段收进「默认分组」；默认排除规则补齐到最新版本。

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
| 学习模式 | 支持 | 不支持 |
| 应用路径手动指定 | 支持 | 不支持 |
| 路由查询 | cscript + PowerShell JSON | `route` / `ifconfig` / `netstat` |

其余已知限制：

- 打包只支持 Layui 项目
- 相对路径含空格的文件会被跳过
- 分流模式切换需断开重连才生效
- 学习模式只给建议，采纳候选网段需要手动点击

## 目录结构

```
src-tauri/src/
  commands.rs      前端调用的命令入口
  config.rs        配置读写与迁移，含多级回退
  packer.rs        打包：遍历、排除、复制、ZIP
  apps/            外部应用定位（finder / registry）
  vpn/
    commands.rs    主进程侧命令与状态轮询
    worker.rs      提权进程，连接生命周期主体
    connector.rs   拉起 openconnect
    route.rs       路由表读写
    script.rs      生成 vpnc-script
    learn.rs       学习模式：采样与网段反推
    credential.rs  密码加密存储
    elevate.rs     提权启动
    session.rs     会话目录 IPC
src/
  components/      ProjectList / ProjectConfig / PackProgress / LogPanel / ExcludeRules
    vpn/           VpnPanel / VpnProfileForm / VpnRuleTable / VpnRoutePreview / VpnLearnCard / VpnInstallGuide
    settings/      SettingsPanel
  composables/     useConfig / usePack / useVpn / useApps / useLog / useUpdater
  types/           前后端共享的数据结构
scripts/release.mjs  构建、上传、更新清单合并
updates/latest.json  更新清单（由发布脚本生成并提交）
```
