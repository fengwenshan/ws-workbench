# 剪贴板：开放 Windows + 可配置快捷键

日期：2026-11-09 · 状态：已确认

## 目标

1. 剪贴板历史（采集、浮层面板、管理页）在 Windows 上与 macOS 同样可用。
2. 全局快捷键可配置：主窗口「系统 → 剪贴板」工具栏加齿轮图标，点开弹窗用按键录制设置。
3. 界面上不加说明性文字，保持简洁。

## 方案

复用现有采集与面板框架，只补平台差异，不重写：

- **采集**：`arboard` 在 Windows 上支持文本、`HTML Format`（读取时已剥掉 CF_HTML 头）、
  PNG / DIBV5 图片，现有 `read_and_store` 逻辑直接复用。变化检测 macOS 用
  `NSPasteboard.changeCount`，Windows 用 `GetClipboardSequenceNumber`。
- **面板**：Windows 与 macOS 使用同一个 380×480 透明置顶面板窗口；失焦收起沿用现有
  窗口事件。呼出时记录前台窗口，复制/取消后还原焦点，用户可直接 Ctrl+V 粘贴；
  呼出时把主窗口压到所有窗口后面，避免顶掉用户正在用的应用（对应 macOS 的
  `orderBack`，Windows 用 `SetWindowPos(HWND_BOTTOM)`）。
- **快捷键**：默认 `CommandOrControl+Shift+V`（mac 显示 ⌘⇧V，Windows 显示
  Ctrl+Shift+V）。保存到 `config.json` 的 `clipboard_shortcut` 键，启动时注册；
  修改时先注册新键（失败即保留原键并报错），再注销旧键并落盘。
- **界面**：删除「剪贴板历史仅在 macOS 启用」整块占位；管理页工具栏加齿轮按钮，
  弹窗内是录制区域 + 保存/取消，不写说明文字；仅在保存失败时给一条错误提示。
  面板右上角快捷键徽标按平台格式化显示，每次呼出重新读取设置。

## 平台范围

macOS 与 Windows 启用全部功能；Linux 维持现状（不采集、不建面板）。

## 验证

- macOS：`cargo check` / `cargo test --lib`、`pnpm build`、mock 面板与弹窗交互截图。
- Windows：本机无法编译该目标（交叉检查卡在 ring 的 C 工具链），Win32 调用单独
  在临时 crate 里做 `x86_64-pc-windows-msvc` 编译验证；真机行为需在 Windows 上确认。

## 实现备注（2026-11-09 完成时补充）

- 齿轮放在「系统 → 剪贴板」工具栏最右，只显示图标（hover 提示「快捷键设置」），点击弹
  `ShortcutSettingsDialog`；弹窗只有一个录制区 + 取消/保存，没有解释性文案，注册失败时才在原位显示后端返回的错误
- 录制把 mac 的 ⌘ 归一成 `CommandOrControl`，Windows 的 Ctrl 归一成 `Control`，要求至少一个非 Shift 修饰键
- 保存时后端先注册新组合，成功再落 `config.json` 的 `clipboard_shortcut`，最后注销旧组合；保存失败会回滚注册
- 新旧字符串解析成同一热键（`Command` ↔ `CommandOrControl` 这类别名）时不再重复注册，只更新保存值与显示
- Windows 收起面板的顺序是「先 `SetForegroundWindow` 还焦点、再 `hide`」：隐藏后再抢前台会被系统拒绝
- tauri 2.12 依赖 windows 0.62，本 crate 仍是 0.61，`main.hwnd()` 的句柄取 `.0` 裸指针再包一层，ABI 一致
- 面板徽标 / 空状态用 `formatShortcut()` 显示：mac `⌘⇧V`，Windows `Ctrl+Shift+V`；每次呼出重新读一次 `clipboard_limits`
