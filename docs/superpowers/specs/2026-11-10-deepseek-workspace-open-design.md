# 「用 DeepSeek 打开」并落到左侧工作区 设计

- 日期：2026-11-10
- 状态：待评审（方案：ws-workbench 打开方式 + DSH 本地插件 + 自动安装）
- 关联：`src-tauri/src/commands/openwith.rs`、`src-tauri/src/apps/models.rs`、
  `src/components/OpenWithButton.vue`、`src/api/system.ts`、新增 `src-tauri/src/commands/deepseek.rs`、
  新增 `dsh-plugin/dsh-open-workspace/`

## 1. 背景与目标

ws-workbench 的「打开方式」分体按钮（`OpenWithButton`）会列出本机探测到的外部应用，
点了就用该应用打开当前项目目录。现在用户要把 DeepSeek Harness 桌面版加进这个菜单：
点 `DeepSeek` 打开当前项目目录，并且：

- DeepSeek 左侧工作区列表里**没有**这个目录 → 新增一个工作区；
- **已经有** → 直接选中它，打开该工作区的空白新会话；
- 用户电脑上装了 DeepSeek、但没装配套插件时，ws-workbench 能**自动安装**，安装前必须
  弹窗告知并征得同意。

### 调研结论（决定方案的事实）

1. DeepSeek Harness 桌面版（本机 0.2.1-rc.5，Electron）**没有**「用指定目录打开工作区」的
   外部入口：`dsh://` 深链只处理 `dsh://open`（前置窗口）；`dsh` 命令行启动的是
   web / headless / sdk 等 profile，不认识 `--workspace`；`info.plist` 没有目录文档类型，
   `open -a "DeepSeek Harness" <dir>` 只会激活窗口。
2. DSH 支持第三方插件（本机 desktop profile 已装 `dsh-chime`：host 半注册 HTTP 路由、
   client 半在页面轮询），插件可以拿到：
   - host：`ctx.webServer.register({ kind:'exact', path, handler })`；
   - client：`ctx.workspaces.create({ path })`（DSH 侧幂等：存在就返回已有的）、
     `ctx.uiWorkspace.openWorkspace(id)`（等价 DSH 自己「添加工作区 → 打开空白新会话」）。
3. 因此不改 DSH 源码、不碰 `app.asar`，用一个本地小插件就能实现；代价是 DSH 要安装一次
   插件并**重启一次**才加载。

### 目标（In Scope）

1. macOS + Windows 的打开方式菜单里新增 `DeepSeek`（仅当检测到 DSH 桌面版已安装）。
2. 点击后：把项目目录作为工作区交给 DSH——没有就新增，有就复用并选中，打开空白新会话。
3. 插件随 ws-workbench **生产包**一起分发；目标机器缺插件时能自动安装，安装前弹窗确认。
4. 安装/更新幂等、可回滚，全程离线，不改 DSH 安装目录、不联网。
5. 相关 Rust 逻辑有单测；DSH 插件用临时 `DSH_HOME` 做端到端验证。

### 非目标（Out of Scope）

- 修改 DeepSeek Harness 源码、给 `app.asar` 打补丁、向 DSH 官方提 PR。
- 自动退出/重启用户正在运行的 DSH（只在插件安装后提示用户重启）。
- 远程/跨机器打开；非桌面版 DSH（`dsh web` / `headless`）的工作区联动。
- DSH 插件市场发布、插件独立升级通道（插件版本跟随 ws-workbench 发版）。

## 2. 数据流

```
ws-workbench 点「DeepSeek」
  → 检查 DSH desktop profile / 插件状态
      ├─ profile 未初始化 → 报错：先打开一次 DeepSeek
      ├─ 插件未安装/版本旧 → 弹窗确认 → 写插件文件 + 改 profile 清单（备份）
      └─ 插件就绪
  → 原子写 $DSH_HOME/open-workspace.json  {requestId, path, createdAt}
  → 唤起 / 前置 DeepSeek Harness
  → DSH 插件 client 轮询 GET /dsh-open-workspace/poll
      → 消费请求文件
      → ctx.workspaces.create({path})
      → ctx.uiWorkspace.openWorkspace(workspaceId)
  → 左侧工作区出现并选中，主区域是空白 New Session
```

请求文件是 ws-workbench 与 DSH 之间唯一的桥：不用端口、不用 token、不用发现协议；
DSH host 的 HTTP 路由只在 DSH 页面内可用（走宿主鉴权），外部进程碰不到。

## 3. ws-workbench 改动

### 3.1 菜单目录

- macOS：`MAC_CATALOG` 新增 `deepseek` / 展示名 `DeepSeek` / bundle `DeepSeek Harness.app`，
  顺序放在「访达」之后，其余保持现状（装了才出现）。
- Windows：`AppId` 新增 `DeepSeek`（`DeepSeek Harness.exe`，标准安装位置
  `%LOCALAPPDATA%\Programs\DeepSeek Harness`，注册表 DisplayName 关键词
  `DeepSeek Harness`），菜单项排在编辑器之后、终端之前。
- 启动分支：`deepseek` 不走「把目录当参数传给应用」的通用路径，走专用命令（见 3.3）。

### 3.2 请求文件

- 路径：`$DSH_HOME/open-workspace.json`；`DSH_HOME` 未设置时 `~/.dsh`
  （Windows：`%USERPROFILE%\.dsh`）。与 DSH 的 `resolveDshHome` 规则一致。
- 内容：`{"requestId":"<uuid>","path":"<绝对路径>","createdAt":"<RFC3339>"}`。
- 写入：先写同目录 `.tmp` 再原子 rename；目录不存在时创建；文件权限 0600（Unix）。
- 路径取 `nearest_existing_dir` 归一化后的目录（沿用现有「目录不存在退到最近上级」行为）。
- 同一个请求被消费后由插件删除；10 分钟内未被消费视为过期丢弃。

### 3.3 插件状态 / 自动安装（新增 Tauri 命令，`commands/deepseek.rs`）

命令：

| 命令 | 作用 |
| --- | --- |
| `deepseek_plugin_status` | 返回 `{dsh_home, profile_ready, plugin_installed, installed_version, bundled_version, needs_update, dsh_running}` |
| `install_deepseek_plugin` | 安装/更新插件，返回安装后的状态 |
| `open_deepseek_workspace` | 检查插件就绪 → 写请求文件 → 唤起 DSH（`open -a` / ShellExecuteW） |

安装动作（幂等）：

1. 要求 `$DSH_HOME/profiles/desktop/package.json` 已存在（DSH 至少启动过一次），否则报错提示；
2. 首次安装把 `package.json` 备份为 `package.json.ws-workbench.bak`（已存在则不覆盖）；
3. 插件文件写到两处：
   - 规范副本 `$DSH_HOME/plugins/dsh-open-workspace/`；
   - 生效副本 `$DSH_HOME/profiles/desktop/node_modules/dsh-open-workspace/`
     （**复制**而不是软链：Windows 建目录软链要权限，且这样不依赖 pnpm）；
4. 改 `profiles/desktop/package.json`：`dependencies["dsh-open-workspace"]` 写
   `"link:../../plugins/dsh-open-workspace"`，`dsh.profile.bundles` 末尾追加
   `dsh-open-workspace`（已存在则不重复）；写回同样走临时文件 + rename；
5. 任一步失败：恢复 `package.json` 备份、清掉本次新建的插件目录，返回错误。

「DSH 是否在运行」检测：macOS 读
`~/Library/Application Support/@deepseek-ai/dsh-desktop/SingletonLock` 软链里的 PID 并探活；
Windows 用 `sysinfo` 查 `DeepSeek Harness.exe` 进程。只影响提示文案，不阻塞流程。

前台交互（`OpenWithButton.vue`，ant-design-vue `Modal.confirm` / `message`）：

- 点 `DeepSeek` → 先取状态：
  - `profile_ready=false`：提示「先打开一次 DeepSeek Harness 完成初始化」；
  - 未安装 / 版本旧：弹窗说明「会一次性安装本地插件（离线、写 DSH profile、可回滚）」，
    确认后安装；取消则中止，不打开 DSH；
  - 安装后若检测到 DSH 正在运行：提示「需要退出并重新打开 DeepSeek 才生效；已排好打开
    请求，10 分钟内重启会自动打开该目录」；用户手动重启即可（不自动杀进程）；
  - 已就绪：直接走打开流程。
- 打开成功/失败沿用现有日志 + `message.error` 反馈。

### 3.4 打进生产包

- 插件源码放仓库根 `dsh-plugin/dsh-open-workspace/`（4 个文件，纯 JS，无依赖、无构建步骤）。
- Rust 侧用 `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../dsh-plugin/..."))`
  把文件编进二进制：dev 与安装包行为一致，不需要 `bundle.resources` 额外配置，也不怕
  资源目录路径差异；`build.rs` 增加 `rerun-if-changed` 让插件改动触发重编译。
- 版本号：插件 `package.json.version` 即「随包版本」；安装/更新时与已装副本比对，不一致
  即提示更新。插件行为改动随 ws-workbench 发版一起 bump 插件版本。
- 附带 `scripts/dsh-plugin-install.sh`：在不能运行 ws-workbench 的机器上（CI、本机首次
  手动装、临时 `DSH_HOME` 测试）复用同一套文件布局，便于测试与排障；生产用户走 App 内
  自动安装。

## 4. DSH 插件 `dsh-open-workspace`

包结构（照 `dsh-chime` 的最小形态）：

```
dsh-plugin/dsh-open-workspace/
  package.json          # name / version / type:module / exports / dsh.bundle.patch / dsh.client.platform
  cordis.patch.yml      # insert: [{ id: dsh-open-workspace, name: dsh-open-workspace }]
  lib/index.mjs         # host 半
  lib/client.js         # client 半（window.__ModuleLoader__.load 包装）
  README.md
```

host 半（`inject: ['webServer']`）：

- `GET /dsh-open-workspace/poll`：读请求文件 → 校验 `path` 是绝对存在的目录、`createdAt`
  未过期 → 删除文件并返回 `{"request":{...}}`；否则返回 `{"request":null}` 并清理过期文件；
- `GET /dsh-open-workspace/status`：调试用，返回插件版本、最近一次请求/结果/错误；
- 全部 try/catch，路由异常不影响宿主。

client 半（`inject: ['workspaces','uiWorkspace']`）：

- 400ms 轮询 `/dsh-open-workspace/poll`；
- 等 `ctx.workspaces.list.getSnapshot().phase === 'ready'` 再消费（启动恢复期不抢跑）；
- 串行 `busy` 防重入：`ctx.workspaces.create({path})` → `ctx.uiWorkspace.openWorkspace(id)`；
- 失败只 `console.warn` 并记录到 status 路由；不弹 DSH 自己的通知、不阻塞页面。
- 若依赖的服务不存在（DSH 升级破坏内部 API）：不轮询、不抛错，请求文件留到过期。

## 5. 兼容性与边界

- DSH 版本：本机 0.2.1-rc.5 实测路径；插件依赖 `ctx.webServer` / `workspaces` /
  `uiWorkspace` 这些一/三方插件都在用的接口，DSH 升级后若变动，退化行为是「请求不生效」，
  不会让 DSH 起不来。
- 自定义 `DSH_HOME`：两端都按 `DSH_HOME` 优先，支持多套 DSH home。
- DSH 没装：菜单不出现；插件没装：点的时候明确报错，不会静默打开空窗口。
- DSH 在托盘/未前台：`open -a` 会唤醒窗口，插件照常消费。
- 连续点击：请求文件后写覆盖前写，插件只处理最新一次。
- 插件被手动删/被 DSH 恢复模式禁用：下次点击重新走安装提示。
- DSH 正在运行时装插件：文件与清单先落盘，重启后加载；不自动杀进程。
- Windows 不做自动重启引导（只提示手动重启）。

## 6. 安全与回滚

- 只写 `$DSH_HOME` 下的文件；不写 DSH 安装目录、不写系统目录、不联网。
- profile 清单是 JSON 合并（保留其他插件与配置），首次安装留 `.bak`。
- 安装失败自动恢复备份；提供脚本 `--uninstall` 分支删除插件目录与清单项（排障用）。

## 7. 测试与验收

Rust 单测（临时目录当 `DSH_HOME`，不碰真实 profile）：

- 安装：清单依赖/bundles 正确、插件文件落两处、重复安装幂等、版本检测；
- 失败回滚：清单坏 / 目录不可写时备份恢复；
- 请求文件：内容、原子性（覆盖写）、路径归一化、过期判断（纯函数）；
- 插件文件内容完整性（include 的四个文件都存在且能被读出）。

DSH 端到端（临时 `DSH_HOME` + 复制 desktop profile 挂插件 + `dsh web` 起服务）：

- 浏览器打开带 token 的 URL，脚本写请求文件 → 左侧新增工作区并选中空白会话；
- 同一目录第二次 → 不新增、直接选中；
- 过期请求 / 非法目录 → 丢弃、不影响页面；
- 页面无报错、插件路由 200。

真机验收（用户执行）：

1. ws-workbench 点 `DeepSeek` → 出现安装确认 → 同意；
2. 按提示退出并重新打开 DeepSeek → 左侧出现该项目工作区且选中；
3. 再点一次（DSH 已在运行）→ 直接切到该工作区，不重复新增。

## 8. 交付物

- 新增：`dsh-plugin/dsh-open-workspace/**`、`src-tauri/src/commands/deepseek.rs`、
  `scripts/dsh-plugin-install.sh`、本设计文档、实施计划文档
- 修改：`src-tauri/src/commands/openwith.rs`、`src-tauri/src/apps/models.rs`、
  `src-tauri/src/commands/mod.rs`、`src-tauri/src/lib.rs`、`src-tauri/build.rs`、
  `src/api/system.ts`、`src/components/OpenWithButton.vue`、`README.md`

## 9. 主要风险

1. DSH 内部服务接口（`workspaces` / `uiWorkspace` / `webServer.register`）不是公开稳定
   契约，DSH 大版本升级可能失效——降级为「点了只打开 DSH，不建工作区」，需在验收清单
   里加一条「DSH 升级后回归」。
2. 用户 profile 已被 DSH 插件管理器接管时，手动写清单可能与插件管理器认知不一致（下次
   用 UI 安装/卸载会以 pnpm 为准）——安装时只追加不删除，尽量幂等；README 注明。
3. DSH 首次使用前就点（profile 未初始化）无法安装——已用明确提示兜住。
