# ws-workbench

基于 Tauri 2 的桌面工作台，把交付日常收进一个窗口：分组 / 项目管理、Markdown 笔记与加密附件、
Layui 项目打包、VPN 规则分流、Hosts 在线编辑、运行环境自检，外加加密落盘的剪贴板历史
（文本 / 图片 / 图文，macOS / Windows）。关闭窗口不退出，应用常驻系统托盘。

- 编码与发布规则：[AGENTS.md](AGENTS.md)（硬规则，改代码前先读）
- 功能说明与实现细节：见文末「文档索引」

## 功能概览

- **工作区**：分组与一级项目混排的项目树（像文件管理器），支持拖拽排序；分组和项目各带一份 Markdown 笔记与一组加密附件
- **笔记**：Vditor 即时渲染（Typora 式交互）、大纲 / 附件栏；正文图片与附件从上传起就是密文
- **项目打包**：识别 Layui / Vue2 / Vue3 / React / SpringBoot 等项目类型与 Git / SVN 远端；仅 Layui 可打包，包含白名单可按项目配置
- **代理（VPN）**：内置 openconnect，规则 / 全局 / 直连三种模式，按域名、后缀或 IP 网段分流，改规则热生效
- **系统工具**：软件卸载与残留清理、弹窗拦截（仅 Windows）、屏幕休眠、Hosts 在线编辑
- **剪贴板历史**：`⌘⇧V` / `Ctrl+Shift+V` 呼出，文本 / 图片 / 图文，加密落盘
- **局域网分享**：同一局域网内自动发现、收发文件（仅本应用之间互通）
- **托盘**：左键显示主界面，右键快捷面板（VPN / 休眠）

完整说明见 [docs/features.md](docs/features.md)。

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

- Node 22 + pnpm（CI 固定 pnpm 10；`pnpm-lock.yaml` 为 9.0 格式）
- Rust stable 工具链
- VPN 功能：连接器随安装包内置（macOS 静态 openconnect、Windows openconnect + `wintun.dll`），无需另装；连接需要管理员授权

## 开发与启动

```bash
pnpm install          # 安装前端依赖
pnpm dev              # 只起前端，http://localhost:8790
pnpm exec tauri dev   # 起完整桌面应用
```

- `pnpm build` = `vue-tsc --noEmit && vite build`；跑 `cargo test` 前要先 build 出 `dist/`。
- 改完代码要重启验证：Windows 用 `powershell -ExecutionPolicy Bypass -File scripts\dev-restart.ps1`；
  macOS 先清掉残留的 vite / tauri 进程，再 `pnpm exec tauri dev`。
- 开发细节（实机冒烟、开发版与安装版共存、平台代码分离、消息订阅）：[docs/development.md](docs/development.md)

## 打包

```bash
pnpm build                              # 前端构建
pnpm exec tauri build                   # 打当前平台安装包
node scripts/release.mjs --no-publish   # 构建但不发布，产物在 release/
pnpm package:local                      # 复制最新安装包到下载目录并打印 SHA-256
```

- 版本唯一来源是 `src-tauri/tauri.conf.json` 的 `version`；改完 push `main`，CI 自动构建、发布并打 tag。
- Windows 的 exe / msi 必须在 Windows 主机构建（MSI 依赖 WiX，不能交叉编译）；macOS 需要 Apple Silicon 与 Intel 两份 dmg。
- 发布流程、beta 通道、CI、自动更新：[docs/release.md](docs/release.md)

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [AGENTS.md](AGENTS.md) | 编码与发布硬规则（必读） |
| [docs/features.md](docs/features.md) | 功能详解与实现取舍 |
| [docs/ui.md](docs/ui.md) | 界面规范：颜色令牌（亮/暗）、字号、间距、组件约定、评审清单 |
| [docs/development.md](docs/development.md) | 开发细节：实机冒烟、重启、双版本共存、平台代码分离、消息订阅 |
| [docs/release.md](docs/release.md) | 构建与发布、CI、beta 通道、自动更新 |
| [docs/data.md](docs/data.md) | 配置与数据（数据库、加密、目录布局） |
| [docs/platform.md](docs/platform.md) | 平台与限制 |
| [docs/architecture.md](docs/architecture.md) | 目录结构与分层约定 |
