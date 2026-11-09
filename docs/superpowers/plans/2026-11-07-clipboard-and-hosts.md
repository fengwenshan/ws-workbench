# 剪贴板历史 + Hosts 快捷编辑实现计划

- 日期：2026-11-07
- 规格：`docs/superpowers/specs/2026-11-07-clipboard-and-hosts-design.md`
- 分支：feat/uninstaller
- 分工：Lead 做剪贴板 + 全部共享文件集成 + 最终验证；Teammate `hosts-dev` 做 Hosts 后端与前端页面

## 阶段 0：共享骨架（Lead）

1. `src-tauri/Cargo.toml` 增加 `tauri-plugin-clipboard-manager = "2"`、`tauri-plugin-global-shortcut = "2"`
2. `src-tauri/src/lib.rs`：插件初始化、`clipboard-panel` 窗口、`Command+Shift+V` 注册、`ClipboardState` 托管、命令注册（剪贴板 + Hosts）、托盘「Hosts」入口、窗口失焦隐藏
3. `src-tauri/src/commands/mod.rs`：声明并重导出 `clipboard`、`hosts`
4. `src-tauri/src/clipboard.rs`、`src-tauri/src/commands/clipboard.rs`：先放可编译的占位实现
5. `src-tauri/src/hosts/mod.rs`、`src-tauri/src/commands/hosts.rs`：先放可编译的占位实现（Teammate 后续替换）
6. `vite.config.ts` 增加 `clipboard` 入口；`clipboard.html`；`capabilities/default.json` 加 `clipboard-panel`
7. `useWorkbenchLayout.ts`：`AppView` 加 `hosts`，`⌘/Ctrl+5`；`App.vue` 加图标栏按钮、页面容器、托盘事件映射；`HostsPanel.vue` 占位

验收：`cargo check` 通过（占位实现）、`pnpm build` 通过（占位组件）

## 阶段 1：剪贴板（Lead，与阶段 2 并行）

1. `clipboard.rs`：`ClipboardItem` / `ClipboardHistory` / `classify` / 采集线程 / 定位与显示
2. `commands/clipboard.rs`：`clipboard_items` / `clipboard_clear` / `clipboard_copy` / `clipboard_panel_hide`
3. `src/api/clipboard.ts`、`src/clipboard/ClipboardPanel.vue`、`src/clipboard/main.ts`、`clipboard.html`
4. Rust 单测覆盖去重 / 上限 / 类型 / 超长 / 定位夹取

验收：`cargo test clipboard` 通过；`pnpm build` 通过

## 阶段 2：Hosts（Teammate，与阶段 1 并行）

1. `hosts/mod.rs`：模型、托管块解析/合并/校验、导入、纯函数单测
2. `hosts/store.rs`：`<data_dir>/hosts.json` 环境配置读写、备份列表/创建/清理、单测
3. `hosts/file.rs`：读 hosts、生成内容、备份、macOS/Windows 提权写入、DNS 刷新、单测
4. `commands/hosts.rs`：设计文档 4.2 列的 11 个命令
5. 前端：types / api / composable / 3 个组件
6. 回报：集成所需共享文件改动（若有）、测试结果、Windows 未验证项

验收：`cargo test hosts` 通过；`pnpm build` 通过；按规格交互完整

## 阶段 3：集成与验证（Lead）

1. 合并 Teammate 的模块，补齐命令注册 / 托盘 / 页面入口 / 样式
2. 跑 `cargo test`、`cargo check`、`pnpm build`
3. 起 `pnpm tauri dev` 做冒烟：剪贴板面板出现 / 写回；Hosts 读取与预览（写入需人工授权，标注未自动验证）
4. 更新 README 功能说明
5. 检查最终 diff、清理占位代码与调试日志
6. 可选：按仓库习惯提交（默认不提交，等用户确认）

## 文件写作用域

| 范围 | Owner |
| --- | --- |
| `src-tauri/src/clipboard.rs`、`src-tauri/src/commands/clipboard.rs`、`src/clipboard/`、`src/api/clipboard.ts`、`clipboard.html` | Lead |
| `src-tauri/src/hosts/`、`src-tauri/src/commands/hosts.rs`、`src/types/hosts.ts`、`src/api/hosts.ts`、`src/composables/useHosts.ts`、`src/components/hosts/` | hosts-dev |
| `src-tauri/src/lib.rs`、`src-tauri/src/commands/mod.rs`、`src-tauri/Cargo.toml`、`vite.config.ts`、`src/App.vue`、`src/composables/useWorkbenchLayout.ts`、`src-tauri/capabilities/default.json`、`README.md`、`docs/` | Lead |

Teammate 不得修改共享文件；需要共享文件配合时在完成消息里列出。

## 执行记录（2026-11-07）

| 阶段 | 结果 |
| --- | --- |
| 0 共享骨架 | 完成；`cargo check` + `pnpm build` 通过 |
| 1 剪贴板 | 完成；`cargo test clipboard` 7 passed；dev 冒烟确认面板窗口创建、`⌘⇧V` 注册、启动收录 / 轮询收录、面板页面加载并调用 `clipboard_limits` / `clipboard_items` |
| 2 Hosts | 完成（`hosts-dev`）；`cargo test hosts` 36 passed；全量 `cargo test` 382 passed / 3 ignored；`pnpm build` 通过 |
| 3 集成验证 | 完成；全量 `cargo test` 382 passed / 3 ignored，`cargo check` 通过，`pnpm build` 通过；README 已补功能说明 |

未自动验证（需要真实桌面 / 授权交互）：

- 真实按 `⌘⇧V` 呼出 / 收起面板（macOS 辅助功能授权限制，无法用 `osascript` 模拟按键）
- 点击条目写回剪贴板 + 收起面板的完整交互（依赖上一条）
- Hosts「应用到 hosts」的系统授权框、取消授权、备份落盘、DNS 刷新（需人工点授权）
- Windows 的 UAC 写入 / `ipconfig /flushdns` / 取消路径（无 Windows SDK，交叉 `cargo check` 因缺 MSVC 头文件无法完成；只做了代码审查）

## 修订：2026-11-09（Hosts 在线编辑）

- 按用户要求把 Hosts 从「左侧独立工具 + 环境/条目表」改为「系统页第 4 个页签 + 整文件在线编辑」，`⌘/Ctrl+S` 保存。
- 后端命令收敛为 `hosts_status` / `hosts_save` / `hosts_backups` / `hosts_restore` / `hosts_open_file_dir`；删除环境配置 / 托管块 / 导入 / 预览相关代码与测试。
- 前端删除 `HostsPanel` / `HostsProfileList` / `HostsEntryTable`，新增 `HostsEditor.vue`；`CleanerPanel.vue` 在「屏幕休眠」后加「Hosts」页签。
- 验证：`cargo check`、`cargo test`、`pnpm build` 通过；同样受系统授权限制，真实保存 / UAC 取消 / DNS 刷新需手动验收。
- 追加：去掉编辑器顶部状态条、底部语法图例与快捷键提示（用户反馈「这个不要」）；hosts 语法高亮（注释 / IPv4 / IPv6 / 主机名，非法标红）、CRLF 保留；tokenizer 用 esbuild + node 断言过文本往返一致与 32 KB 性能。
