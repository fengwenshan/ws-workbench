# 构建、发布与自动更新

> 来源：原 [README](../README.md) 的「构建与发布」「自动更新」章节；发布硬规则见 [AGENTS.md](../AGENTS.md) 第 14 节。

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

**安装包内容按平台裁剪**：`src-tauri/` 下两份平台配置都是 JSON Merge Patch（RFC 7396，`null` 即删除），Tauri 2 构建时自动和 `tauri.conf.json` 合并：`tauri.macos.conf.json` 把 `bundle.resources` 里的 `resources/apps` 去掉，再把 `resources/openconnect-macos` 映射到 `Contents/Resources/apps/openconnect`，所以 mac 包带的是自己的静态 openconnect（不含 Windows 运行时约 9 MB 的 exe + DLL）；`tauri.windows.conf.json` 把 `resources/zh-Hans.lproj` 去掉，所以 Windows 包不带 macOS 的本地化资源（`resources/apps` 继续由 `tauri.conf.json` 打进 Windows 包）。VPN 探测逻辑也按平台拆文件：macOS / Windows 都只探内置副本、不向外找（见上文「连接器来源」）。改一边不会把另一边的行为带偏。

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
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 更新签名私钥的密码（私钥没设密码就设成空值；写进 `env.local`，不要提交） |
| `GITHUB_TOKEN` | GitHub 令牌（CI 里由 Actions 注入），创建发行版、上传附件、推送更新清单必需 |
| `GITEE_TOKEN` | Gitee 私人令牌（`projects` 权限），`pnpm release:beta` 读写 Gitee 清单和发行版必需；默认空（beta 必填，稳定版由 CI 提供） |
| `TAURI_BUNDLES=app` | 只打 app、跳过 Tauri 的 dmg 打包脚本，受限环境配 `--dmg` 使用 |

环境变量也可以写在仓库根目录的 `env` 和 `env.local` 里：脚本启动时会自动读取，
优先级是「命令行 / CI 显式设置 > `env.local` > `env`」。`env` 可以提交（只放空占位或
公开默认值，例如 `GITEE_TOKEN=`），`env.local` 已被 `.gitignore` 的 `*.local` 忽略，
用来放本机令牌：

```bash
# env.local（不要提交）
GITEE_TOKEN=xxxxxxxx
# 签名私钥没有密码时留空即可（不会再弹 Password:）；有密码就填真实密码
TAURI_SIGNING_PRIVATE_KEY_PASSWORD=
```

这样 `pnpm release:beta` 就不用每次在命令前指定令牌。

### 多平台发布：同一把签名密钥

更新签名密钥和操作系统无关（minisign 密钥对，只跟 `tauri.conf.json` 里的 `updater.pubkey`
配对），所有平台共用同一把，不要给不同平台生成不同密钥对：

- 另一台机器（Windows / Intel Mac）上把 `~/.tauri/ws-workbench.key` 和同目录的 `.pub`
  复制到对应位置（Windows 是 `%USERPROFILE%\.tauri\ws-workbench.key`），或者在 `env.local`
  里写 `TAURI_SIGNING_PRIVATE_KEY=<密钥内容>`；
- 私钥有密码时再在 `env.local` 里写 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=...`；
- 构建时脚本用这把密钥签名，各平台产物都由同一个公钥验签；换密钥 = 所有已装客户端
  自动更新失效，只能手动重装，所以宁可找回密码也不要轻易重新生成。

### 内测（beta）通道

功能先合到长期分支 `beta`，正式发布 = 把 `beta` 合进 `main`。

- **beta 版本号**：`tauri.conf.json` 的 `version` 是 base（如 `1.0.25`），每次发布自动加成
  `1.0.25-beta.N`（N 取 Gitee 上已有的最大值 + 1），构建时用 `tauri build --config` 注入。
- **发布 beta**：先把 `beta` 分支推到远程，再在本地跑 `pnpm release:beta`（GITEE_TOKEN 放 `env.local` 里，
  也可以临时用 `GITEE_TOKEN=... pnpm release:beta`）。脚本会构建当前平台、签名、把产物和
  `updates/beta/latest.json` 只发到 Gitee（代码仍推 GitHub `beta`；安装包和清单不发 GitHub、不触发 GitHub 构建）。
  Windows 的 exe/msi、Mac 的 Intel 版在各自机器上 checkout 同一个提交，用同一个
  `--version` 再跑一次，清单会按平台补齐。
- **beta 分支发布规则（先推远程，再本地打包）**：顺序固定——先把代码推到 `beta` 远程，再在本地/各平台构建，最后把产物上传到 Gitee 同名发行版。每次推送到 beta 的代码都必须对应一个可安装的 beta 发行版，不允许停在「只有代码、没有包」；构建失败时用同一个 `--version <base>-beta.<N>` 补发，不能重新分配 N。
  1. 推送前：工作区干净、当前在 `beta` 分支；`src-tauri/tauri.conf.json` 的 `version`（base）必须大于最新正式版。先跑 `cd src-tauri && cargo test`、`pnpm build`。
  2. **先发布 beta 远程环境**：`git fetch origin beta`，有远端新提交先 rebase，再 `git push origin beta`。Gitee 仓库只有 `master`，发行版和 tag 建在 `master` 上，tag 不会指向 beta 提交；产物对应的是构建时的 beta HEAD，清单写在 Gitee `master` 的 `updates/beta/latest.json`。
  3. **再本地构建并上传**：第一条命令会分配 beta 号，记下日志里的 `<base>-beta.<N>`；后续平台必须用同一个 `--version`。
     - macOS 必须出 **两个 dmg**：Apple Silicon（`aarch64`）与 Intel（`x86_64`）。在 Apple Silicon 上两条都跑；Intel 主机先 `rustup target add aarch64-apple-darwin`。`--dmg` 额外生成 dmg，更新载荷仍是 `.app.tar.gz`；受限环境无法挂载临时卷时加 `TAURI_BUNDLES=app`。
       ```bash
       pnpm release:beta -- --target aarch64-apple-darwin --dmg
       pnpm release:beta -- --target x86_64-apple-darwin --dmg --version <base>-beta.<N>
       ```
     - Windows 必须出 **两个安装包**：NSIS 的 `.exe`（更新载荷）与 WiX 的 `.msi`（手工安装）。MSI 依赖 WiX，必须在 Windows 主机上构建，不能交叉编译。WiX 不接受 `1.0.25-beta.4` 这种预发布版本号，发布脚本会给 MSI 单独注入纯数字 ProductVersion（如 `1.0.25.4`），应用版本仍是 beta：
       ```bash
       pnpm release:beta -- --version <base>-beta.<N>
       ```
     `GITEE_TOKEN` 放 `env.local`（beta 必填）。脚本会创建 `v<base>-beta.<N>` 发行版、上传 exe / msi / dmg，并把 `updates/beta/latest.json` 合并写回 Gitee（三个平台条目按平台合并，不覆盖另一个平台）。`--no-publish` 只用于不想上传的本机试装，正式发版不要带。
  4. 完成检查：Gitee 发行版列表里能看到 `v<base>-beta.<N>`，附件包含 macOS 两个 dmg（Intel + Apple Silicon）和 Windows exe + msi；`updates/beta/latest.json` 平台条目不缺；产物对应的提交就是这次推送的 beta HEAD。

- **发布正式版**：`beta` 合进 `main` 后 push。CI 的 preflight 读 `tauri.conf.json` 的版本号，
  该版本没发过才构建（并发/重复 push 不会重复发版）；发完自动打 `v<version>` tag。
  想强制重跑用 Actions 的 `workflow_dispatch` + `force`。
- **客户端**：设置 →「检查更新」→「开发者模式」开关。默认关闭即只收正式版；打开后优先
  收内测版，标题栏会挂 `beta` 标签。**关闭不降级**：当前内测版保留，下一个正式版发布后
  自动收敛（semver 里 `1.0.25-beta.1 < 1.0.25`，所以正式版一发布就会被当成更新）。
  生效通道只看这个开关，不按安装版本推断；手动装过 beta 包但没开开关的用户只会收到正式版。
- **救火**：beta 清单写坏、或想临时暂停内测，直接在 Gitee 上编辑 / 删除
  `updates/beta/latest.json` 即可：beta 用户下一次检查会自动回退稳定清单，不报错也不降级。
  正式版发坏时，修好代码但**不改** `tauri.conf.json` 版本，push `main` 会被 preflight 判为
  已发布而跳过，需用 Actions 的 `workflow_dispatch` + `force` 重建该版本；如果改了版本号，
  push `main` 会正常自动发版，不需要 force。
- **顺序要求**：每次正式版发完后，beta 分支要把 `tauri.conf.json` 的版本提到正式版之上，
  否则 beta 线会被正式版反超，内测用户提前「升级」到正式版后就再也收不到 beta。

发布版本的唯一来源是 `src-tauri/tauri.conf.json` 的 `version`：push `main` 后 CI 先读它，
没有同名发行版才构建，构建时通过 `tauri build --config` 注入，不改源文件。发布流程：
push `main` → preflight 判断 → 构建（含更新签名）→ 上传安装包到 GitHub 发行版并打
`v<version>` tag → 合并生成本平台的 `updates/latest.json` 条目并提交回 `main`（带
`[skip ci]`）→ 串行跑一次 Gitee 镜像（`gitee-sync` job），把三个更新载荷传到 Gitee 发行版
并把清单里的下载地址换成 Gitee。客户端先从 Gitee 读清单，读不到再用 GitHub 清单兜底。

```bash
# 改 src-tauri/tauri.conf.json 的 version，提交后：
git push origin main   # preflight 判断该版本没发过 → 构建 → 发布 → 自动打 v<version> tag
```

### CI

`.github/workflows/ci.yml` 是常驻护栏：push 到 `main`、pull request 与手动 `workflow_dispatch` 都会跑，`test-macos` 和 `test-windows` 两个 job 分别在 `macos-latest`、`windows-latest` 上原生编译，各自 `pnpm build` + 在 `src-tauri` 下 `cargo test --lib`，防止改坏对面平台的代码合进来。两个 job 都先构建前端：tauri 的 `generate_context!` 在编译期要求 `frontendDist`（`../dist`）存在。发版由 push `main` 触发 `release.yml`，preflight 按 `tauri.conf.json` 版本判断，不受普通提交影响。

`.github/workflows/release.yml` 由 push `main` 触发，先跑 `preflight` 判断该版本是否已发布，三个构建 job 再并行，产出：

| 平台 | 自动更新载荷（带签名） | 手工安装包 |
| --- | --- | --- |
| macOS Apple Silicon | `ws-workbench_aarch64.app.tar.gz` | `ws-workbench_<ver>_aarch64.dmg` |
| macOS Intel | `ws-workbench_x86_64.app.tar.gz` | `ws-workbench_<ver>_x64.dmg` |
| Windows x64 | `ws-workbench-setup.exe`（NSIS） | `ws-workbench_<ver>_x64_en-US.msi` |

macOS 两个架构都在 `macos-latest`（arm64）上跑，Intel 版用 `--target x86_64-apple-darwin` 交叉编译；Windows 在 `windows-latest` 上原生构建（MSI 需要 WiX）。版本号由 `preflight` 从 `tauri.conf.json` 读出（必须是 `X.Y.Z` 形式，否则预检直接失败），tag 由脚本按 `v<version>` 创建。

跑之前必须配一个仓库密钥：`TAURI_SIGNING_PRIVATE_KEY`（本机 `~/.tauri/ws-workbench.key` 的全文），没配会在预检步骤直接退出。私钥有密码时再配一个 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（workflow 里已把它注入构建 job；没配这个 secret 时变量是空串，脚本会当作「私钥无密码」处理）。

两个构建 job 会同时改 `updates/latest.json`，脚本用「快进到远端最新 → 以远端为基线合并本平台条目 → 推送被拒就撤销重试」来避免互相覆盖。CI 自己推回 `main` 的清单提交带 `[skip ci]`，且 preflight 判断该版本已发布就跳过构建，不会自触发。

三个构建 job 之后还有一个串行的 `gitee-sync` job（`needs: [preflight, macos, windows]`）：`scripts/gitee-sync.mjs` 把三份更新载荷**和手工安装包（两个 dmg + msi）**传到 Gitee 同名 tag 的发行版，再把 Gitee 仓库里和 GitHub `main` 上的两份 `updates/latest.json` 的下载地址都改成 Gitee。dmg / msi 不在更新清单里，只是给国内用户手动下载安装用，缺了只告警、不影响发版。它需要：

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
