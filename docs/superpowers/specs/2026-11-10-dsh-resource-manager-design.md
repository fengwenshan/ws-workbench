# DeepSeek 插件独立发布 + 资源管理设计

日期：2026-11-10（替代 `2026-11-10-deepseek-workspace-open-design.md` 中「插件随包内置 + 点击时自动安装」的方案）

## 1. 背景与问题

上一版把 `dsh-open-workspace` 插件源码内置在 ws-workbench 里，点「DeepSeek」时用 `include_str!`
把文件写进 DSH profile，版本号只在 `package.json.version` 里比。带来三个问题：

1. 插件不是独立项目，改插件要动主仓、发版也要跟着 ws-workbench 发；
2. 改插件忘了 bump 版本时，本机装着的旧副本会被当成最新（后来补了内容指纹，但仍是补丁）；
3. 插件无法单独迭代、单独给别人用。

新方案：插件变成独立仓库，按 tag 发布 zip；ws-workbench 在「设置 → 资源」里检测/下载/安装/更新，
点「DeepSeek」时只做检测和引导，不再内置、不再自动装。

## 2. 目标

- 插件源码独立到 `/Users/fengwenshan/wenshan/demo/dsh-open-workspace`，推送到
  `https://github.com/fengwenshan/dsh-open-workspace.git`；tag `vX.Y.Z` 与插件
  `package.json.version` 一一对应。
- 每个 tag 由 GitHub Actions 自动打 zip 并挂到 GitHub Release（资产名固定）。
- ws-workbench「设置」新增「资源」栏目：列出资源、显示本地版本 / 最新版本 / 状态，
  支持下载安装、更新、卸载、刷新；为后面管理 Chrome 插件预留通用模型与占位行。
- 点「打开方式 → DeepSeek」时：
  1. 没装 DeepSeek Harness → 提示先安装（不启动）；
  2. 装了但没装插件 → 提示去「设置 → 资源」下载（不自动安装）；
  3. 装了插件 → 用本地版本 vs 远端最新 tag 判断；不一致提示更新（可去设置，也可先直接打开）；
  4. 版本一致 → 走原打开工作区流程（写请求文件 + 唤起 DSH）。

## 3. 非目标

- 本期不做 Chrome 插件的下载/安装，只留资源模型与占位 UI。
- 不做插件自动后台升级；更新由用户点「设置 → 资源」触发。
- 不做离线内置兜底：没装过插件的用户必须能访问 `api.github.com` 才能装上。

## 4. 术语

| 术语 | 含义 |
| --- | --- |
| 资源 | ws-workbench 可下载/安装的附加组件，本期有 DeepSeek 工作区插件、Chrome 插件（占位） |
| 插件仓库 | `fengwenshan/dsh-open-workspace`，独立 git 仓库 |
| 最新版本 | 插件仓库 GitHub Release `latest` 的 tag 去掉 `v` 前缀 |
| 本地版本 | `$DSH_HOME/profiles/desktop/node_modules/dsh-open-workspace/package.json` 的 `version` |
| 资源 zip | Release 资产 `dsh-open-workspace-<version>.zip` |

## 5. 插件独立仓库

### 5.1 目录与历史

- 目录：`/Users/fengwenshan/wenshan/demo/dsh-open-workspace`。
- 内容从主仓 `dsh-plugin/dsh-open-workspace` 原样搬过去，历史用
  `git subtree split --prefix=dsh-plugin/dsh-open-workspace -b plugin-split` 生成，
  新仓库以该分支为 `main` 起点，保证旧提交仍可追溯。
- `git remote add origin https://github.com/fengwenshan/dsh-open-workspace.git`；
  本机连不上 `github.com:443`，push 由用户在能访问的网络执行（或提供可用镜像/代理）。

### 5.2 目录结构

```text
dsh-open-workspace/
  package.json            # name=dsh-open-workspace, version=X.Y.Z, dsh.client.inject ...
  cordis.patch.yml
  lib/index.mjs           # host 插件（注册 poll/status 路由）
  lib/client.js           # 浏览器插件（轮询请求文件、开工作区）
  README.md
  LICENSE                 # 可选
  .github/workflows/release.yml
```

### 5.3 版本与 zip 契约

- tag 形如 `v0.1.0`，必须等于 `v` + `package.json.version`，否则 workflow 失败。
- Release 资产名：`dsh-open-workspace-<version>.zip`。
- zip 根目录直接包含 `package.json`、`cordis.patch.yml`、`lib/index.mjs`、`lib/client.js`、
  `README.md`（不再套一层目录）。
- Release 由 workflow 自动创建，`latest` 指向最新正式版（非 pre-release）。

### 5.4 首次引导顺序

1. 先在 ws-workbench 里 `git subtree split` 生成插件历史分支，克隆到
   `/Users/fengwenshan/wenshan/demo/dsh-open-workspace`，加 `.github/workflows/release.yml`，
   commit 后打 tag `v0.1.0`；
2. 用户把该仓库推到 GitHub 并 push tag，workflow 生成首个 Release + zip；
3. 确认 Release 资产可访问后，ws-workbench 再删 `dsh-plugin/` 并切到「设置 → 资源」方案，
   避免出现「插件没地方下载、点击又不再自动装」的中间状态。

### 5.5 Release workflow（tag 触发）

`.github/workflows/release.yml`：

1. `on: push: tags: ["v*"]`；
2. checkout 后读 `package.json.version`，校验 `v${version} == $GITHUB_REF_NAME`；
3. `zip -r "dsh-open-workspace-${version}.zip" package.json cordis.patch.yml lib README.md`；
4. `softprops/action-gh-release@v2` 上传 zip、生成 release notes、`prerelease: false`。

## 6. ws-workbench 主仓改动

- 删除 `dsh-plugin/` 目录（源码唯一源在插件仓库）；`build.rs` 里 5 个
  `cargo:rerun-if-changed` 与 `include_str!` 相关代码一并删除。
- `src-tauri/src/commands/deepseek.rs` 只保留：DSH 安装检测、profile 状态、插件本地状态
  （manifest + node_modules 版本）、启动 DSH、写打开请求、打开工作区；删除内置文件表
  `PLUGIN_FILES`、`install_to_home`、`bundled_version` / `needs_update` 字段、内容指纹相关
  函数与测试；`install_deepseek_plugin` 命令删除（安装只走资源命令）。
- 前端 `src/api/system.ts` 同步删除 `installDeepseekPlugin`，`DeepseekPluginStatus` 去掉
  `bundled_version`、`needs_update`。
- 测试 fixture：`src-tauri/tests/fixtures/dsh-plugin/` 下放最小插件文件，
  测试运行时用 `zip` crate 现场打 zip，不依赖已删除的插件源码。
- `scripts/dsh-plugin-install.sh` 改为「从本地 zip 安装」（`--zip <path>`），
  保留给本地开发 / 离线手动安装；文档标注生产路径是设置-资源。

## 7. 资源管理后端（`src-tauri/src/commands/resources.rs`）

### 7.1 数据模型

```rust
pub struct ResourceStatus {
  pub id: String,             // "dsh-open-workspace" | "chrome-extensions"
  pub name: String,           // 展示名
  pub kind: String,           // "dsh-plugin" | "chrome-extensions"
  pub description: String,
  pub available: bool,        // 本期是否支持安装（Chrome=false）
  pub installed: bool,
  pub installed_version: Option<String>,
  pub latest_version: Option<String>,
  pub update_available: bool,
  pub error: Option<String>,  // 远端检查/下载失败原因
  pub note: Option<String>,   // 例如「需要重启 DeepSeek 生效」
}
```

资源目录内置两条：`dsh-open-workspace`（repo=fengwenshan/dsh-open-workspace，
kind=dsh-plugin）与 `chrome-extensions`（kind=chrome-extensions，available=false，
note=「Chrome 插件管理规划中」）。

### 7.2 远端版本检查

- 只对 `available=true` 的资源做远端检查；Chrome 占位行跳过。
- 默认基址 `https://api.github.com`，可用环境变量 `WS_RESOURCE_API_BASE` 覆盖（测试/镜像）。
- `GET {base}/repos/fengwenshan/dsh-open-workspace/releases/latest` → `tag_name`（去 `v`）。
- 内存缓存 5 分钟；`force=true` 强制刷新。
- 实现方式：`std::process::Command` 调系统 `curl`（macOS / Windows 10+ 自带），
  `curl -sSfL --max-time 8 -H "Accept: application/vnd.github+json" <url>`，不新增 Cargo 依赖。
- 失败（超时、非 0 退出、JSON 解析失败、无 release）→ `error=Some(...)`，`latest_version=None`。

### 7.3 安装

`resource_install(id)`：

1. 校验 id 与 `available`；Chrome 直接返回「暂不支持」。
2. 校验 DSH profile 存在（`$DSH_HOME/profiles/desktop/package.json`），否则返回
   「先打开一次 DeepSeek Harness 完成初始化」。
3. 取最新版本（缓存或现查）；查 Release 资产：`GET {base}/repos/.../releases/latest`，
   在 `assets[]` 里找 `dsh-open-workspace-<version>.zip`，取 `url`（API asset URL）。
4. 下载：`curl -sSfL --max-time 120 -H "Accept: application/octet-stream" -o <tmp.zip> <asset_url>`
   （asset URL 会 302 到 `release-assets.githubusercontent.com`，`-L` 跟随）。
5. 解压到临时目录，校验：必须包含 `package.json`、`cordis.patch.yml`、`lib/index.mjs`、
   `lib/client.js`；`package.json.name == dsh-open-workspace`；`version` 非空。
6. 安装到 DSH：
   - 写 `$DSH_HOME/plugins/dsh-open-workspace/` 与
     `$DSH_HOME/profiles/desktop/node_modules/dsh-open-workspace/`；
   - 合并 `$DSH_HOME/profiles/desktop/package.json` 的 `dependencies` 与 `dsh.profile.bundles`
     （保留其它条目，重复则覆盖为 `link:../../plugins/dsh-open-workspace`）；
   - 首次修改前备份 `package.json.ws-workbench.bak`（已存在则不覆盖）。
7. 返回安装后的 `ResourceStatus`；若检测到 DSH 正在运行，`note` 置为「需要重启 DeepSeek 生效」。

### 7.4 卸载

`resource_uninstall(id)`：删除两个插件目录，从 profile `package.json` 移除 `dsh-open-workspace`
的 `dependencies` 与 `dsh.profile.bundles` 条目（备份文件保留，不自动恢复）。重启 DSH 生效。

### 7.5 命令

| 命令 | 说明 |
| --- | --- |
| `resource_list(force: bool)` | 资源列表：本地状态 + 远端最新版本（缓存/强制刷新） |
| `resource_install(id)` | 下载 zip → 校验 → 安装 → 返回状态 |
| `resource_uninstall(id)` | 卸载插件 → 返回状态 |

### 7.6 点击 DeepSeek 用的检测命令

`deepseek_plugin_status` 保留（快速、本地）：`app_installed`、`profile_ready`、
`plugin_installed`、`installed_version`、`dsh_running`。
新增 `deepseek_plugin_update()` → `{ latest_version, update_available, error }`（远程检查，缓存同上），
点「DeepSeek」时在本地检查通过后调用。

## 8. 设置 → 资源 UI

- `SettingsPanel.vue` 末尾新增 `<section class="card">`「资源」，组件
  `src/components/settings/ResourcePanel.vue`。
- 每行资源：名称、说明、状态徽标（未安装 / 已安装 vX / 可更新 vX → vY / 检查失败）、
  本地版本、最新版本、按钮：
  - 未安装：[下载安装]
  - 已安装且可更新：[更新]
  - 已安装：[卸载]
  - 右上角：[刷新]（force=true）
- 安装/更新/卸载时按钮 loading；结果用 `message` 提示；错误在行内展示并保留重试。
- 安装成功且 DSH 在运行：行内显示「已安装，需要重启 DeepSeek Harness 生效」。
- Chrome 行：`available=false`，按钮禁用，显示「Chrome 插件管理规划中」。
- 首次进入设置-资源自动检查一次远端版本（静默，不阻塞页面）。

## 9. 点击 DeepSeek 的判定流程

```text
点「DeepSeek」
  ├─ app_installed=false → 警告：请先安装 DeepSeek Harness（结束）
  ├─ profile_ready=false → 确认：先打开一次 DeepSeek 完成初始化（可只启动 DSH）
  ├─ plugin_installed=false →
  │     警告：还没装工作区插件，去「设置 → 资源」下载
  │     [打开设置]（切到设置页并定位到资源卡片） / [取消]
  ├─ 调 deepseek_plugin_update()
  │     ├─ update_available=true → 确认：插件有新版本 vX（当前 vY）
  │     │     [去设置更新] / [直接打开]（直接打开仍写请求并唤起）
  │     ├─ error（无法检查更新）→ 直接打开，log 里记一条警告
  │     └─ 版本一致 → 直接打开
  └─ 打开：写 `$DSH_HOME/open-workspace.json` → 唤起 / 前置 DSH
```

- 「打开设置」通过 `window.dispatchEvent(new CustomEvent("ws-open-settings", {detail:"resources"}))`
  通知 `App.vue`：`selectTool("settings")` 并让资源卡片滚动/高亮。
- 设置里安装/更新完成不会自动打开工作区；用户再点一次「DeepSeek」即可（请求文件 10 分钟 TTL）。

## 10. 版本比较

- 解析版本：去掉 `v` 前缀，截掉第一个 `-` 之后的内容（如 `0.2.0-beta.1` → `0.2.0`），
  按 `.` 拆成数字段逐段比较，缺段补 0；远端 > 本地 → `update_available=true`，否则 false。
- 不做「版本回退」提示；本地比远端新时 `update_available=false`（避免误报）。

## 11. 错误处理与降级

| 场景 | 行为 |
| --- | --- |
| `api.github.com` 超时/失败 | 设置里显示错误 + 重试；点击 DeepSeek 仍可打开（log 警告） |
| Release 里没有 zip 资产 | 安装失败，提示「发布包缺少 zip 资产」 |
| zip 结构非法 | 安装失败，提示缺哪个文件，临时目录清理，不改 DSH 文件 |
| profile 不存在 | 安装失败，提示先打开一次 DeepSeek |
| 安装中 DSH 正在运行 | 安装照常完成，提示重启 DeepSeek 生效 |
| curl 不存在（极老系统） | 返回明确错误「系统缺少 curl」 |
| 最新 Release 没有 zip 资产 | 报错且不写 DSH 文件 |
| `package.json.name` 不是 `dsh-open-workspace` | 报错且不写 DSH 文件 |

## 12. 测试

- Rust 单测（`resources.rs`）：
  - 版本解析/比较（`v0.1.0` vs `0.2.0`、预发布、非法输入）；
  - Release JSON 解析与资产选择（fixture JSON）；
  - zip 解压安装：用 fixture 目录现场打 zip，安装到临时 DSH_HOME，断言文件、manifest、备份；
  - 卸载：断言文件删除、manifest 条目移除；
  - zip 缺文件 / name 不符 → 报错且不改动 DSH。
- DeepSeek 侧单测：保留 `app_installed`、打开请求、启动参数等既有测试；删除内置插件与
  指纹相关测试。
- 集成测试：用 `python3 -m http.server` 起本地假 GitHub API（`WS_RESOURCE_API_BASE` +
  `releases/latest` JSON + zip 资产），跑「检查 → 下载 → 安装 → 卸载」全链路。
- 前端：`pnpm build` 类型检查；手动点设置资源卡、点 DeepSeek 各分支。
- 端到端（在真 DSH 上）：装插件 → 重启 DSH → 点 DeepSeek → 左侧出现/选中工作区并打开空白会话。

## 13. 迁移与兼容

- 已装插件（0.1.0）的用户：ws-workbench 通过 profile manifest 识别为已安装，版本 = 0.1.0；
  远端发布 v0.1.0 后视为一致，不发更新提示。
- 主仓删除 `dsh-plugin/` 是破坏性改动：旧版 `scripts/dsh-plugin-install.sh` 同步改成 zip 安装。
- 插件仓库首次 tag `v0.1.0` 必须在 ws-workbench 改版发版前完成，否则点击会一直提示去设置下载。

## 14. 风险与后续

- 国内网络访问 `api.github.com` 可能不稳定；后续可加「镜像基址」配置或走 ws-workbench 已有的
  Gitee 同步链路（本期不做）。
- 未登录查询 GitHub API 有 60 次/小时限流；版本检查缓存 5 分钟，正常使用足够。
- 后续 Chrome 插件：复用 `ResourceStatus` 与 `ResourcePanel`，新增 `kind=chrome-extensions`
  的检查/安装实现（Chrome Web Store API 或本地 crx），资源目录扩展即可，不改 UI 骨架。
