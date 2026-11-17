# LANLink · 未读红点与状态订阅（补充需求 15）

日期：2026-11-17
前置：`2026-11-16-lanlink-ui-refresh.md`、`2026-11-16-lanshare-message-delete-design.md`

## 1. 需求

1. 左侧模块导航的 LANLink 图标有未读消息时显示红点；用户看过（mark_read 后未读归零）红点消失。
2. 「消息订阅模式」：模块导航 / 托盘快捷面板不应各自轮询、各自求和，而是订阅同一份后端状态，代码只维护一处。
3. 系统托盘图标在有未读时也显示红点；托盘快捷面板的未读角标走同一订阅。
4. 不改 VPN 的既有事件/poller 机制（它本来已是订阅式：`vpn` 状态轮询后 emit，前端 useVpn 订阅）。本轮只把「未读数」收敛成单一事件源。

## 2. 数据流

```
SQLite（唯一事实源：lan_share_conversations.unread）
  └─ ChatStore::total_unread()
       ├─ 命令 lan_share_unread_total → 各窗口首次加载
       ├─ 事件 lan-share://unread { total } → 主窗口 rail / QuickPanel 角标
       └─ Rust 托盘：收到 chat 变化时刷新托盘图标（红点/无红点）
```

- 事件在 `emit_chat` 统一出口触发（消息、已读、删除会话/消息、附件状态变化都会经过它），不再让每个组件自己算。
- 事件 payload：`{ "total": number }`。
- 初始值：新命令 `lan_share_unread_total() -> u32`；前端 composable 先拉一次，再订阅增量。

## 3. 后端

- `lanshare/mod.rs`：
  - `pub const EVENT_UNREAD: &str = "lan-share://unread";`
  - `#[derive(Serialize)] struct UnreadPayload { total: u32 }`
  - `emit_chat_with_direction_opt` 末尾：计算 `total_unread()` 并 emit 新事件；同时调用托盘刷新（`crate::refresh_tray_badge(app)`）。
  - `LanShareState::total_unread()` 已存在（保持）。
  - 命令 `lan_share_unread_total`。
- `lib.rs`：
  - 托盘图标按 unread 生成带红点变体（macOS 模板图叠加不透明圆点，Windows 红色圆点；36/32px 原图右上角叠加，不改变图尺寸）。
  - `pub(crate) fn refresh_tray_badge(app: &AppHandle)`：读 `total_unread`，在主线程 `tray_by_id("main").set_icon(...)`；启动时托盘 build 后调用一次。
  - 不新增托盘 tooltip（保持「不显示应用名」的既有决定）。
- VPN 不在本轮合并进事件；其状态继续沿用 `vpn` 状态轮询 + 前端订阅（QuickPanel / rail 已这样工作）。

## 4. 前端

- `src/api/lanshare.ts`：`EVENT_UNREAD`、`onLanShareUnread(handler)`、`getLanShareUnreadTotal()`。
- 新增 `src/composables/useLanShareUnread.ts`（或 lanshare 目录）：`unread` ref；onMounted 拉初始值 + 订阅，onUnmounted 取消；订阅失败静默（不阻塞主界面）。
- `App.vue`：rail LANLink 按钮右上角红点 `v-if="lanUnread > 0"`，样式红点 + 白描边，不改变按钮点击区域；VPN 图标保持现有绿点。
- `QuickPanel.vue`：未读角标改用同一 composable（删除基于 conversations 求和的 `unreadTotal`），会话列表仍按需刷新；聊天事件不再每次都触发列表重拉（保持现有防抖即可）。
- 红点消失条件：后端 `mark_read` / 删除消息/会话等导致 total 变为 0 → 事件 → 前端 ref 归零。

## 5. 验收

- 单测：`total_unread` 现有覆盖；新增「删除最后一条消息/删除会话后 total 归零」已有相关测试覆盖。
- `cargo test`、`cargo check`、`npx vue-tsc --noEmit`、`npx vite build` 全绿。
- 手动：两台设备互发消息 → 主窗口 rail LANLink 红点 + 托盘图标红点 + QuickPanel 角标；打开会话已读 → 三处同时消失；退出/重启有历史未读 → 初始红点正确。
