# 剪贴板历史 + Hosts 快捷编辑设计

> **更新（2026-11-09）**：本文档的剪贴板部分是 v1 设计（仅文本、纯内存、最多 20 条）。
> 现已升级为「图文 + SQLite 加密存储」v2，见
> [`2026-11-09-clipboard-images-sqlite-design.md`](./2026-11-09-clipboard-images-sqlite-design.md)；
> 热键为 `Command+Shift+V`，且只负责打开/聚焦、不再按一次收起。下文剪贴板部分仅作历史记录。


- 日期：2026-11-07
- 分支：feat/uninstaller
- 状态：设计已确认，实现中
- 关联：不改动现有项目 / 打包 / 笔记 / VPN 数据面逻辑；需求 2「VPN 流量统计 + 分流明细」已按用户要求取消，本设计不涉及

## 1. 背景与目标

工作台现在有项目、代理、系统、设置四个工具。这次补两个日常高频的小工具：

1. **剪贴板历史**：复制过的代码 / 链接 / 文字能找回来。Tauri 官方剪贴板插件采集，全局快捷键 `⌘⇧V`（Command+Shift+V）呼出面板，选中即写回剪贴板，存内存、最多 20 条，进程退出即清空。
2. **Hosts 快捷编辑**：切开发 / 测试 / 生产环境时改 hosts，配合 VPN 分流使用。结构化维护「环境配置」，一键应用到系统 hosts，自动备份、可还原；只改应用自己的托管块，不碰文件里其它内容。

### 目标（In Scope）

**剪贴板历史（macOS）**

- 后台轮询系统剪贴板文本，去重 + 最多 20 条 + 纯内存
- 全局快捷键 `⌘⇧V`（Command+Shift+V）呼出 / 收起面板，面板贴近鼠标位置，多显示器与屏幕边缘夹取
- 面板内搜索过滤、↑↓ 选择、Enter / 单击写回剪贴板并收起、Esc / 失焦收起
- 按内容打标签：链接 / 代码 / 文本
- 单条文本上限 200,000 字符，超出跳过
- Windows 不支持（系统自带 Win+V）：不采集、不注册热键、不建面板
- 选中后只写回剪贴板，不自动模拟 ⌘V（用户明确选择 B 方案，不申请辅助功能授权）

**Hosts 快捷编辑（macOS + Windows）**

- 左侧图标栏新增第 5 个工具「Hosts」，`⌘/Ctrl+5`
- 环境配置（profile）：名称 + 条目列表（IP / 域名（空格分隔多个）/ 备注 / 是否启用）
- 应用 = 只替换 hosts 文件里的托管块，块外内容逐字节保留；再次应用是幂等的
- 每次写入前备份当前 hosts（最多保留 30 份），支持从备份还原
- 写入前校验 IP / 域名格式、总文件大小；写临时文件 + 同目录原子替换，失败时原文件不变
- 提权：macOS 走 `osascript ... with administrator privileges` 系统授权框；Windows 走一次 UAC（`Start-Process -Verb RunAs`）
- 应用后刷新 DNS 缓存（macOS `dscacheutil` + HUP mDNSResponder；Windows `ipconfig /flushdns`）
- 支持「从当前 hosts 导入」块外的普通条目，支持「停用托管」（只移除托管块）
- 原始 hosts 内容只读预览，不提供整文件 RAW 写入（避免误删系统行）

### 非目标（Out of Scope）

- 剪贴板：图片 / 富文本 / 文件列表、固定（pin）、搜索历史落盘、跨设备同步、Windows / Linux
- 剪贴板：自动粘贴（需要辅助功能权限）、密码管理器式的敏感内容过滤
- Hosts：整文件 RAW 编辑、语法高亮、hosts 条目自动测速、远程 hosts 订阅
- Hosts：Linux 支持、开机自动应用环境
- 需求 2 的 VPN 流量统计与分流明细（用户已取消）

## 2. 信息架构

### 2.1 剪贴板

- 独立窗口 `clipboard-panel`（label），前端入口 `clipboard.html`，380 × 480 无边框透明卡片，启动时创建并隐藏
- 全局快捷键：`⌘⇧V`（Rust 侧 `tauri-plugin-global-shortcut` 注册，accelerator `Command+Shift+V`）
- 不占左侧图标栏，不占托盘菜单；纯全局热键 + 浮层
- 与托盘快捷面板类似的定位 / 失焦收起逻辑，但锚点是鼠标位置，不是托盘图标

### 2.2 Hosts

- `AppView` 增加 `'hosts'`；图标栏在「系统」之后加「Hosts」按钮（服务器 / 文件图标，与现有 24px 线性图标一致）
- 快捷键 `⌘/Ctrl+5`；托盘菜单新增「Hosts」入口（与「系统 / 代理 / 设置 / 日志」并列）
- 页面结构：

```text
┌──────────────────────────────────────────────────────────────┐
│ Hosts 快捷编辑                       [刷新] [查看原始文件]     │
│ 状态条：文件路径 · 当前环境 · 写入需管理员授权 · 最近备份      │
├────────────────┬─────────────────────────────────────────────┤
│ 环境列表        │ 选中环境的条目表                            │
│ [+ 新建环境]    │ IP | 域名(空格分隔) | 备注 | 启用 | 删除     │
│ 开发环境 ✓     │ [+ 添加条目]                                │
│ 测试环境        │ [从当前 hosts 导入]                         │
│ 生产环境        │ [保存] [应用到 hosts] [停用托管]            │
├────────────────┴─────────────────────────────────────────────┤
│ 备份抽屉：时间 + 大小 + [还原]                                │
└──────────────────────────────────────────────────────────────┘
```

## 3. 模块划分

### 3.1 剪贴板（Lead 实现）

```text
src-tauri/src/clipboard.rs               历史栈、采集线程、面板显示/定位
src-tauri/src/commands/clipboard.rs      命令：列表 / 清空 / 写回 / 隐藏面板
src/clipboard/main.ts                    独立页面入口
src/clipboard/ClipboardPanel.vue         面板 UI
src/api/clipboard.ts                     命令封装 + 类型
```

- `lib.rs`：插件初始化（clipboard-manager / global-shortcut）、创建隐藏窗口、注册 `Command+Shift+V`、托管 `ClipboardState`、窗口失焦隐藏、命令注册
- `vite.config.ts`：增加 `clipboard` 页面入口；`capabilities/default.json` 的 windows 加上 `clipboard-panel`

### 3.2 Hosts（Teammate 实现，Lead 做共享文件集成）

```text
src-tauri/src/hosts/mod.rs               数据模型、托管块解析/合并/校验（纯逻辑 + 单测）
src-tauri/src/hosts/store.rs             环境配置持久化 <data_dir>/hosts.json
src-tauri/src/hosts/file.rs              读取、备份、提权写入、刷新 DNS（按平台 cfg）
src-tauri/src/commands/hosts.rs          Tauri 命令（只做参数/错误翻译）
src/types/hosts.ts                       前端类型
src/api/hosts.ts                         命令封装
src/composables/useHosts.ts              页面状态与动作
src/components/hosts/HostsPanel.vue      页面容器
src/components/hosts/HostsProfileList.vue
src/components/hosts/HostsEntryTable.vue
```

- 共享文件（`lib.rs` 命令注册、`commands/mod.rs`、`App.vue`、`useWorkbenchLayout.ts`、托盘菜单）由 Lead 改，Teammate 不碰

## 4. 数据与接口

### 4.1 剪贴板

```rust
pub struct ClipboardItem {
    pub id: String,        // uuid v4
    pub text: String,
    pub kind: String,      // url / code / text
    pub created_at: i64,   // unix 毫秒
}
```

- 采集：`std::thread` 每 600ms 读一次文本；与上一次内容相同则跳过；空串 / 纯空白 / 超 200k 字符跳过
- 去重：相同文本已存在 → 移到最前并刷新时间；超过 20 条丢弃最旧
- 启动时把当前剪贴板文本作为第一条（非空时）
- 变化后向前端发事件 `clipboard-changed`（payload 为完整列表）
- 命令：`clipboard_items` / `clipboard_clear` / `clipboard_copy(id)` / `clipboard_panel_hide`
- 面板显示函数 `toggle_panel(app)`：已显示则隐藏；否则定位到鼠标附近、显示、聚焦、发事件 `clipboard-panel-shown`
- 定位纯函数 `panel_origin_at(cursor, panel, screen)`：以鼠标为水平中心、下移 12px，再夹进显示器范围；macOS 用 AppKit 逻辑点（`NSEvent::mouseLocation` + `CGDisplayBounds`），有单测
- 触发全局热键时若应用不在前台，显式 `NSApplication::activateIgnoringOtherApps(true)` 后再 focus 搜索框

### 4.2 Hosts

```rust
pub struct HostsEntry {
    pub id: String,
    pub ip: String,
    pub hostnames: Vec<String>,
    pub comment: String,
    pub enabled: bool,
}
pub struct HostsProfile {
    pub id: String,
    pub name: String,
    pub entries: Vec<HostsEntry>,
    pub note: String,
    pub created_at: String,
    pub updated_at: String,
}
pub struct HostsStore { pub profiles: Vec<HostsProfile>, pub active_profile_id: String }
pub struct HostsStatus {
    pub path: String,
    pub exists: bool,
    pub readable: bool,
    pub managed: bool,
    pub active_profile_id: String,
    pub active_profile_name: String,
    pub line_count: usize,
    pub backup_count: usize,
    pub last_backup_at: String,
    pub platform: String,
    pub needs_admin: bool,
    pub hint: String,
}
pub struct HostsApplyResult {
    pub applied: bool,
    pub path: String,
    pub backup_id: String,
    pub active_profile_id: String,
    pub line_count: usize,
    pub flushed_dns: bool,
    pub message: String,
}
pub struct HostsPreview { pub content: String, pub line_count: usize, pub block: String }
pub struct HostsBackup { pub id: String, pub created_at: String, pub size: u64 }
```

托管块（固定标记，便于幂等替换）：

```text
# >>> ws-workbench managed hosts >>>
# profile: <name> (<id>)
# updated: <ISO8601>
<ip> <hostname...>  # <comment>
...
# <<< ws-workbench managed hosts <<<
```

- 应用：读当前文件 → 去掉旧托管块 → 拼上新块（保留块外内容原样，统一 LF）→ 校验 → 备份 → 提权原子替换 → 刷新 DNS
- 备份：`<data_dir>/hosts-backups/<毫秒>_hosts`，最多 30 份，超了删最旧
- 停用托管：只删除托管块，文件其它内容不动
- 导入：解析块外「非注释 + 合法 IP + 至少一个域名」的行；不能解析的行跳过并在返回值里报告忽略条数
- 写入校验：IPv4 / IPv6 可解析；域名/别名只允许字母数字 `-` `_` `.`，长度 ≤ 253；单行 hostname 数 ≤ 32；生成内容 ≤ 1 MiB；块内出现结束标记的行直接拒绝

命令：

```text
hosts_status
hosts_profiles
hosts_profile_save(profile)
hosts_profile_delete(id)
hosts_apply(profile_id)
hosts_remove_managed
hosts_preview(profile_id)
hosts_import_current
hosts_backups
hosts_restore(backup_id)
hosts_open_file_dir
```

### 4.3 提权写入

- macOS：生成提权 shell 脚本（`/bin/sh <script>`），`osascript -e 'do shell script "..." with administrator privileges'`；脚本内先写 `/etc/hosts.ws-workbench.tmp`、`chmod 644`、`mv -f` 覆盖，再刷新缓存；stdout 输出 `WS_WRITE_OK` / `WS_WRITE_FAIL`（DNS 刷新用 `WS_FLUSH_OK` / `WS_FLUSH_FAIL`），用户取消时按 `-128` / `cancel` / `已取消` 识别并返回「已取消授权」
- Windows：生成 `.ps1` 脚本，`Start-Process -Verb RunAs -Wait` 拉起提权 PowerShell 执行复制 + `ipconfig /flushdns`；UAC 取消返回「已取消授权」
- 所有路径 / 参数在生成脚本时做引号转义（macOS POSIX 单引号转义，Windows PowerShell 单引号 `''` 转义），并有单测
- 应用与还原走同一套提权写入；备份由普通权限完成（读 hosts + 写自己的数据目录）

## 5. 安全与边界

- 剪贴板：纯内存、最多 20 条；不落盘、不进日志；面板失焦即收起
- Hosts：只动托管块；写前备份；写失败保留原文件；所有输入先校验再写；不提供删除系统行的入口
- Hosts 文件路径固定（macOS `/etc/hosts`，Windows `C:\Windows\System32\drivers\etc\hosts`），不接受用户传入路径
- 提权脚本只做「复制临时文件 → 覆盖 hosts → 刷新 DNS」，不执行用户输入的任意命令
- DNS 刷新失败不回滚 hosts（内容已生效），只在结果里如实报告 `flushed_dns=false`
- 剪贴板热键冲突：⌘⇧V 是 Chrome / 编辑器常用的「粘贴为纯文本」，全局注册期间会被本应用接管；注册失败（被其它软件占用）时在启动日志里提示；剪贴板面板本期不做托盘入口

## 6. 测试计划

- 剪贴板（Rust 单测）：类型判定、去重移到最前、20 条上限、超长/空白跳过、清空、面板定位夹取（多显示器 / 边缘 / 小屏）
- Hosts（Rust 单测）：解析 / 合并 / 幂等、块外内容保留、CRLF 归一、非法 IP/域名拒绝、禁用条目不写入、导入跳过注释与托管块、备份数量上限、脚本引号转义
- 构建：`cargo test`、`cargo check`、`pnpm build`（vue-tsc）
- 实机（macOS）：`pnpm tauri dev` 起应用；复制文本 → ⌘⇧V 面板出现 → 选中写回；hosts 读取 / 预览 / 写入（会弹系统授权框，需人工点一次）→ 验证 /etc/hosts 托管块与备份
- Windows：本机只做 `cargo check` 级别的编译验证（若工具链可用），真机验证留到 Windows 机器

## 7. 风险

- 全局快捷键在 macOS 后台应用里弹出窗口需要显式激活应用，否则窗口可能不获得键盘焦点（需要 AppKit activate，实测验证）
- 剪贴板轮询 600ms 会漏掉极快连续复制（两次复制间隔 < 600ms 只保留后一次）；对历史工具可接受
- macOS 每次写入 hosts 都会弹系统授权框（无法像 VPN worker 那样复用提权进程）；这是系统安全模型，不做常驻 root 守护
- Windows 提权 / hosts 路径只做编译检查，未实机验证


## 修订：2026-11-09（Hosts 改为整文件在线编辑）

用户要求：Hosts 不再做条目解析 / 环境配置，直接把系统 hosts 的完整内容放进页面在线编辑，`⌘/Ctrl+S` 保存；入口从左侧图标栏移到「系统」页的「屏幕休眠」页签之后。

变更点：

- 页面：`src/components/cleaner/CleanerPanel.vue` 增加第 4 个页签「Hosts」，内部使用 `src/components/hosts/HostsEditor.vue`（等宽编辑器 + 行号 + 脏状态 + `⌘/Ctrl+S`）。
- 移除：环境（profile）/ 条目表 / 托管块合并 / 预览 / 导入 / 停用托管 / `hosts.json` 配置；不再有 `src-tauri/src/hosts` 的合并与解析逻辑。
- 命令收敛为 5 个：`hosts_status`（路径 + 完整内容 + 备份信息）、`hosts_save(content)`（整文件写入）、`hosts_backups`、`hosts_restore(backup_id)`、`hosts_open_file_dir`。
- 保留的安全边界：固定路径（`/etc/hosts`、`%SystemRoot%\System32\drivers\etc\hosts`）；保存前校验 ≤1 MiB、拒绝 NUL、拒绝非 UTF-8；写前自动备份到 `<data_dir>/hosts-backups/`（保留 30 份）；macOS `osascript` / Windows UAC 提权 + 临时文件原子替换 + 刷新 DNS；取消授权不改文件。
- 校验放宽：不再逐行校验 IP / hostname（在线编辑需要写任意 hosts 语法），写错条目由用户负责，可随时从备份还原。
- 入口移除：左侧图标栏 Hosts 按钮、`⌘/Ctrl+5`、托盘「Hosts」菜单、`AppView` 的 `hosts` 值。
- 页头状态条（路径 / 行数 / 大小 / 已同步）按用户要求移除；路径改挂在「打开目录」按钮 `title` 上；底部语法图例、底部快捷键提示也按反馈移除，只在有改动 / 关闭高亮时显示一行小字（「有未保存改动」/「文件较大…」）。
- 语法高亮：`src/utils/hostsHighlight.ts` 手写 hosts tokenizer（注释 / IPv4 / IPv6 / 主机名，非法标红），编辑器用 pre 叠层 + 透明 textarea 渲染；IME 组字时临时关掉叠层；文件 >32 KB 退回纯文本。编辑器内部统一 LF，保存时按原文件 CRLF/LF 还原。
