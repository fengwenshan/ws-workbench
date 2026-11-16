# 内测（Beta）更新通道设计

日期：2026-11-16

## 1. 背景与问题

现在只有一条更新通道：

- 打 tag `vX.Y.Z` 触发 GitHub Actions，三个平台构建后上传 GitHub 发行版附件，并把
  `updates/latest.json` 合并提交回 `main`；
- `release.yml` 末尾的 gitee-sync job 把安装包镜像到 Gitee 同名 tag，并把清单里的
  下载地址改写成 Gitee；
- 客户端用 `tauri-plugin-updater` 读 `tauri.conf.json` 里配置的清单端点（Gitee 优先、
  GitHub 兜底），只比「清单版本 > 当前版本」。

需求是拆成内测和正式两条通道：内测包版本号形如 `1.9.55-beta.14`（基础版本 + beta
序号），用来先发新功能；设置里给一个开关，开启后就能收到 beta 更新。

难点有四个：

1. 插件没有通道概念，一个清单只有一个 `version`；
2. 插件的 `check()` 是「按端点顺序取第一个能解析成功的清单」，再和当前版本比大小，
   **不是**多清单取最高，所以不能靠「把 beta 清单和稳定清单一起来」简单处理；
3. 稳定清单在 `main/updates/latest.json`，已安装的老客户端只认这个地址，不能动；
4. beta 产物不能出现在 GitHub（不建 tag、不建 release、不写清单），只能本地构建、
   发到 Gitee。

## 2. 目标

- 正式（stable）与内测（beta）两条通道；内测版本号 `<base>-beta.<N>`，`N` 递增。
- 设置页一个开关「接收内测版更新」，默认关，开启需二次确认；开启后启动检查、每日
  10:00 / 16:00 检查、手动检查都走内测通道。
- 内测用户同时能收到正式版更新；关闭开关不降级，停留在当前版本，等下一个正式版
  自然收敛。
- beta 产物只落 Gitee；GitHub 只发正式版；已安装的老客户端行为完全不变。

## 3. 非目标

- 不做灰度百分比、AB 分流、多通道（dev / nightly）。
- 不做降级安装（`allowDowngrades` 保持关闭）。
- 不做 Gitee 以外的镜像（OSS、自有 CDN 等）。
- 不做服务端资格白名单：开关对所有人可见（已在讨论中确认）。

## 4. 术语

| 术语 | 含义 |
| --- | --- |
| 通道 / channel | `stable`（正式）或 `beta`（内测） |
| 基础版本 base | beta 包对应的下一个正式版号，如 `1.0.18` |
| beta 序号 N | 同一 base 下递增的序号，如 `1.0.18-beta.14` 里的 14 |
| 稳定清单 | `updates/latest.json`，GitHub `main` 与 Gitee 镜像各一份 |
| beta 清单 | `updates/beta/latest.json`，只在 Gitee |
| 发布列车 | beta 分支跑「下一个正式版」，稳定后同号转正（Chrome / Firefox / VS Code Insiders 的做法） |

## 5. 设计

### 5.1 分支与发布流程

- `main` = 正式版分支；`beta` = 长期开发 / 集成分支。新功能先合 `beta`，转正时把
  `beta` 合进 `main`。
- **正式版**：push `main` 触发 CI。preflight 读 `src-tauri/tauri.conf.json` 的
  `version`，若远端已存在 tag `v<version>` 就跳过（文档提交、清单提交不会误发版）；
  否则三平台构建、自动创建 tag + GitHub 发行版、合并写 `updates/latest.json`、
  gitee-sync 镜像到 Gitee。行为与现在一致，只是触发方式从「tag push」改成「main push
  + 版本号变化」。
- **内测版**：在 `beta` 分支本地执行 `pnpm release:beta`：构建当前平台 → 上传到
  Gitee 同 tag 发行版 → 合并写 Gitee 上的 `updates/beta/latest.json`。全程不碰
  GitHub，不产生 GitHub tag / release / 清单，也不会触发 Actions。
- **版本号规则**：
  - 开一条 beta 线时，在 `beta` 分支把 `tauri.conf.json` 的 `version` 改成下一个
    正式号（如 `1.0.18`），提交；
  - 内测包版本 = `<base>-beta.<N>`；`N` 自动取 Gitee 上同 base 已有最大 N + 1，
    `--version 1.0.18-beta.14` 可强制指定；
  - 本地脚本校验 `base` 必须大于最新正式版，否则拒绝发 beta（防止版本号倒挂）。
  - 版本号只是排序：`1.0.18-beta.14 < 1.0.18`，所以 beta 用户到正式版会自然收敛。
    本项目沿用现有 patch 递增习惯（`1.0.17 → 1.0.18-beta.1`），不改成 `1.1.0-beta.1`。
- **转正**：合 `beta` → `main` → push `main` → CI 发 `1.0.18` 正式版。beta 用户下次
  检查时收到正式版。
- **hotfix 纪律**（无法完全自动化，写进 README / 脚本提示）：
  - 紧急修复可以直接在 `main` 上改、提版本号、push 发版；
  - 发完必须把 `main` 合回 `beta`，并把 beta 线的 base 提到最新正式版之上；
  - 否则 beta 线版本号低于正式版，beta 用户既收不到 hotfix，也再也升不上去。
- **Windows 补齐**：mac 先发时只上传 mac 平台的包；之后在 Windows 机器上 checkout
  同一个提交，用**同一个 `--version`** 再跑一次，只上传 Windows 包并合并清单条目
  （见 5.2 的部分平台规则）。

### 5.2 清单与部分平台发布

- 稳定清单：`main/updates/latest.json`（GitHub）+ Gitee 镜像，地址与内容结构都不变。
- beta 清单：Gitee 上的 `updates/beta/latest.json`（Gitee 默认分支），通过 Gitee
  contents API 读写。仓库里**不提交**这个文件——beta 发布不产生任何 git 提交，自然
  不会误触发正式构建。
- beta 清单结构与插件 schema 一致，下载地址指向 Gitee 发行版附件：

```json
{
  "version": "1.0.18-beta.3",
  "notes": "ws-workbench v1.0.18-beta.3 (beta)",
  "pub_date": "2026-11-16T04:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "<minisign 签名>",
      "url": "https://gitee.com/feng_wenshan/ws-workbench/releases/download/v1.0.18-beta.3/ws-workbench_aarch64.app.tar.gz"
    }
  }
}
```

- 部分平台发布规则（mac 先发、Windows 后补）：
  - 清单顶层 `version` = 本次构建的版本；
  - 远端清单 `version` 与本次**相等**：合并本平台条目，其它平台条目保留；
  - 远端清单 `version` 与本次**不等**：`platforms` 重置为只含本平台条目，旧版本的
    平台条目丢弃。
  - 原因：插件只认顶层 `version`、再按平台取 url/signature。若保留旧版本条目，会出现
    「提示有 beta.14 → 下载到 beta.13 的包 → 装完版本没变 → 再次提示」的死循环。
  - 「Windows 补齐」指用同一个版本号再发一次，走「相等合并」分支，不会丢 mac 条目。

### 5.3 客户端通道检查

- 新增 Rust 命令 `update_check(channel, timeout) -> Option<UpdateMetadata>`：
  - `UpdateMetadata { rid, current_version, version, date, body, raw_json, channel }`；
  - 取到的 `Update` 放进 webview 的 resource table（与插件现状一致），返回 `rid`；
  - 下载 / 安装继续走现有 `update_download_resumed` / `update_install_resumed`，
    断点续传与 minisign 校验都不变。
- `stable`：用 `updater_builder()` 默认端点（`tauri.conf.json` 的 endpoints），行为与
  现在插件 `check()` 完全一致。
- `beta`：
  1. 从配置端点里筛出 Gitee 的（`host == gitee.com`），把路径里的
     `/updates/latest.json` 换成 `/updates/beta/latest.json`，得到 beta 端点；
  2. 用 beta 端点检查一次；再用稳定端点检查一次；
  3. 两边都拿到 `Update` 时用 semver 比大小取高者；相等时取正式版；
  4. beta 检查失败（404、无本平台条目、超时）视为「没有 beta 更新」，不打断稳定
     检查；两边都失败才返回错误。
- 版本比较用 `semver` crate（插件本来就依赖，`Cargo.toml` 显式加 `semver = "1"`），
  因为 `Update.version` 是 `String`，插件的比较逻辑在 crate 内部。
- 前端 `useUpdater` 增加 `channel` 状态：
  - `checkUpdate` 从 `check()` 改为 `invoke('update_check', { channel, timeout })`，
    返回值用插件导出的 `Update` 类包装（`new Update(metadata)`，只为拿到 `close()`
    和现有类型），或直接持 `{ rid, ... }` 并调 `plugin:resources|close` 释放；
  - 切换通道时清空 pending、清「已忽略版本」、立即重查。
- 老客户端完全不感知 beta，继续读稳定清单。

### 5.4 设置与 UI

- 「设置 → 检查更新」卡片新增一行「更新通道」：
  - 开关「接收内测版更新（Beta）」，默认关；
  - 开启时弹确认，文案要点：内测版更新更频繁、可能不稳定；版本号形如
    `1.0.18-beta.14`；关闭后**不会降级**；下一个正式版发布后自动回归。
- 当前版本号包含 `-`（如 `1.0.18-beta.14`）时，设置页与标题栏显示「内测版」徽标。
- 关闭通道：立即按稳定通道重查；若当前版本高于稳定版，提示「当前已安装内测版，
  不会再收到内测更新；下一个正式版发布后自动回归」。
- 启动静默检查、每日 10:00 / 16:00 检查、手动「检查更新」都使用当前通道。
- 开发构建保持现状：可以检查，不能安装。

### 5.5 持久化与通道推断

- `AppSettings` / `AppConfig` 新增 `update_channel: String`（serde default 空串），
  存 `config.json`；`from_config` / `into_config` 同步映射；前端
  `AppConfig.update_channel?: 'stable' | 'beta'`。
- 空串表示「跟随安装版本」：当前版本是预发布（含 `-`）时按 `beta` 处理，否则按
  `stable`。这样手动装了 beta 安装包、还没动过开关的用户不会「卡住收不到更新」。
  用户拨动开关后写入显式 `stable` / `beta`。
- 前端类型为 `update_channel?: 'stable' | 'beta' | ''`（`''` / 缺省 = 跟随安装版本）。
- `App.vue` 加载配置后调用 `setChannel`；开关变化时先 `saveConfig`，再切通道并重查。

### 5.6 发版脚本与 CI

**`scripts/release.mjs` 新增 `--beta`**

1. 校验：当前分支是 `beta`、工作区干净、有签名私钥、有 `GITEE_TOKEN`；
2. 版本：`--version` 优先；否则 base 取 `tauri.conf.json` 的 version，N 取 Gitee 上
   `v<base>-beta.*` 的最大 N + 1；校验 base 大于**稳定清单里的 version**（最新正式版）；
3. 构建：复用现有 `build()`，通过 `--config {"version": ...}` 注入版本，不发布 GitHub；
4. 发布：调用 `gitee-sync.mjs --beta --version <版本> --dir release`；
5. 打印下一步提示：本次提交 SHA、Windows 补齐命令（同一 `--version` + 该 SHA），以及
   Mac Intel 版是否需要 `--target x86_64-apple-darwin` 再跑一次。

**`scripts/gitee-sync.mjs` 新增 `--beta`**

- 跳过 GitHub 相关步骤（`--beta` 隐含 `--no-github`），清单基准改为从 Gitee 的
  beta 清单读取（读不到就按空清单处理）；
- `ensureRelease` 用 beta tag `v<version>`，上传更新载荷（`.exe` / `.app.tar.gz`）
  和可选手工安装包（dmg / msi）；
- 按 5.2 规则合并 `platforms`，`notes` 带 `(beta)` 标记；
- 写回 Gitee `updates/beta/latest.json`；
- `prerelease` 字段：Gitee API 支持就置 `true`，否则忽略（不影响更新）；
- 只写 Gitee，绝不触碰 GitHub 清单与仓库文件。

**`.github/workflows/release.yml`**

- 触发改为 `on.push.branches: [main]`（另留 `workflow_dispatch` 便于手动重建）；
  去掉 tag 触发，避免换触发方式后重复构建；
- 新增 preflight job：从 `src-tauri/tauri.conf.json` 读版本 → 查 GitHub 是否已有
  `v<version>` 的 tag/release → 输出 `should_release` / `version`；
- 三个构建 job 依赖 preflight，并传 `--version <version>`；
- 清单提交继续带 `[skip ci]`，防止 preflight 被清单提交再次触发；
- gitee-sync job 用 preflight 的 version，不再依赖 `github.ref_name`；
- 创建 GitHub 发行版时显式指定 `target_commitish` 为本次提交 SHA。

**兼容**：切换触发方式前已存在的 tag 会被 preflight 判定为「已发布」直接跳过，不会
重复发版。

### 5.7 救火

- beta 包有问题：优先快速发下一个 beta 修复（不降级）。
- beta 清单写错：通过 Gitee contents API 改回上一份内容，客户端下次检查即生效。
- 严重问题：引导用户手动安装正式版安装包（GitHub / Gitee 发行版）。

### 5.8 从现流程迁移

1. `main` 上的 `tauri.conf.json` 版本目前还是 `1.0.0`（真实版本一直靠 tag 注入），
   切到「push main 按版本号发版」之前，先把它对齐到线上最新版本（当前是 `1.0.24`），
   否则 preflight 会误判（`v1.0.0` 已存在 → 永远不构建）。
2. `beta` 分支的 `tauri.conf.json` 版本改成下一个正式号（`1.0.25`），作为第一条 beta
   线的 base。
3. 先发一个包含本功能的正式版（走现有 tag 流程或新流程均可），这样老用户升级后才
   有「内测通道」开关；之后 beta 才有人能收。

## 6. 边界与异常

| 场景 | 行为 |
| --- | --- |
| beta 清单不存在 / 404 / 超时 | beta 用户回落稳定通道，不报错 |
| Gitee 不可达 | 稳定检查走 GitHub 兜底（现状）；beta 通道无更新 |
| beta 清单缺本平台条目 | 该平台视为「无更新」，不报错 |
| 版本号解析失败 | 取稳定清单结果，不取 beta |
| 关闭开关但当前版本高于稳定版 | 停住、不降级，等下一个正式版 |
| 同 base 的 beta 号重复 / 回退 | 发版脚本拒绝 |
| 正式版号 ≤ beta 线 base | beta 用户升到正式版后，beta 线必须提 base 号 |
| 老客户端 | 完全不受影响，继续读稳定清单 |

## 7. 测试与验收

**单元测试（Rust，`src-tauri/src/updates.rs`）**

- Gitee 稳定端点 → beta 端点派生（含 master / main 两个分支 URL）；
- semver 取高：`1.0.18-beta.14 < 1.0.18`、`1.0.18-beta.2 < 1.0.18-beta.14`；
- beta 检查失败 / 无本平台条目时回落稳定；
- 两边相等时取稳定。

**构建与类型检查**

- `cd src-tauri && cargo test`、`cargo check`；
- `pnpm build`（`vue-tsc --noEmit && vite build`）。

**手工端到端（本地 mock）**

- 本地 HTTP 服务模拟稳定清单 / beta 清单，验证开关切换 → 检查 → 下载 → 安装路径；
- 下载用现有签名载荷，验证断点续传与签名校验不受影响。

**真包演练**

1. `beta` 分支把 base 提到 `1.0.19`，`pnpm release:beta` 出 `1.0.19-beta.1`；
2. 手动装 beta 包 → 设置里开启内测通道 → 能收到 `1.0.19-beta.2`；
3. 合 `beta` → `main`、push `main` 发 `1.0.19` → beta 用户收到正式版；
4. 继续发 `1.0.20-beta.1` → 已开启通道的用户收到；
5. 关闭开关 → 不再收 beta；`1.0.20` 正式版发布后自动回归。

**CI 守卫**

- push `main` 但版本号没变 → 不构建；
- beta 发布不产生任何 GitHub tag / release / 清单。

## 8. 风险与取舍

- beta 只存 Gitee：Gitee 不可用或限流时 beta 通道不可用（内测可以接受，国内下载更快）。
- 正式版触发从 tag 改成 push main：每次 main push 都会跑 preflight（秒级），版本没变
  不会构建；发布动作变成「合并即发布」，需要习惯。
- 流程纪律（开线提版本号、hotfix 合回 beta）无法完全自动化，脚本只能做硬校验和提示。
- 部分平台发布有窗口期（mac 发了、Windows 还没发），5.2 的规则保证不会下错包，但
  Windows 用户在那段时间收不到 beta。
- beta 版本号可能被用户手动装到更旧的稳定版机器上：属于用户自己下载安装的行为，
  不在本设计处理范围。

## 9. 后续可选

- beta 也镜像到 Gitee 之外的源（OSS / CDN）。
- 通道灰度、白名单（需要服务端能力）。
- 设置页展示 beta 更新日志 / 对应提交。
- 老 beta 安装包与缓存的自动清理。

## 10. 改动面

- Rust：`src-tauri/src/updates.rs`（新命令、端点派生、取高、测试）、`src-tauri/src/lib.rs`
  （注册命令）、`src-tauri/Cargo.toml`（`semver`）、`src-tauri/src/models.rs`、
  `src-tauri/src/config.rs`（`update_channel`）
- 前端：`src/types/index.ts`、`src/composables/useUpdater.ts`、新增 `src/api/updater.ts`、
  `src/components/settings/SettingsPanel.vue`、`src/App.vue`、`src/composables/useConfig.ts`
- 脚本：`scripts/release.mjs`（`--beta`）、`scripts/gitee-sync.mjs`（`--beta`）
- CI：`.github/workflows/release.yml`
- 文档：本 spec、`README.md` 的发版章节（流程纪律）
