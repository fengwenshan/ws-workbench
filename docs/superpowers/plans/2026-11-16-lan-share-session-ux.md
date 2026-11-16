# 局域网分享 · 会话体验增强（实施计划）

> **For agentic workers:** 按任务顺序执行；每个任务结束时跑对应验证命令并提交。后端用 TDD 先写失败测试。

**Goal:** 完成左栏右键会话管理、提示音/免打扰、粘贴与拖拽发送、托盘消息列表与未读角标。

**Architecture:** 后端在 `lanshare` 增加会话附件查询/清理、剪贴板与拖拽落盘、`ChatEvent.direction`、Dock 角标、快捷面板模式与定位事件；前端在现有 `LanSharePanel` / `LanChatPane` / `QuickPanel` 上接事件与交互；删除仅本机，收到的文件优先移入废纸篓。

**Tech Stack:** Rust（std 网络、rusqlite、arboard、image、trash、tauri 2）、Vue 3 + TS、ant-design-vue 4、Vite。

**Spec:** `docs/superpowers/specs/2026-11-16-lan-share-session-ux-design.md`

---

## Task 1: chat_store 会话附件查询

**Files:**
- Modify: `src-tauri/src/lanshare/chat_store.rs`
- Test: 同文件 `#[cfg(test)] mod tests`

- [ ] 加 `pub fn total_unread(&self) -> Result<u32, String>`：`SELECT COALESCE(SUM(unread), 0) FROM lan_share_conversations`。
- [ ] 加 `pub fn attachment_count(&self, peer_id: &str) -> Result<u32, String>`：解密该会话 `direction='in'` 且 `kind='file'` 的消息，统计 `file_path` 非空条数。
- [ ] 加 `pub fn received_file_paths(&self, peer_id: &str) -> Result<Vec<String>, String>`：同上返回路径（供删会话前收集）。
- [ ] 加 `pub fn clear_received_file_paths(&self, peer_id: &str) -> Result<Vec<String>, String>`：事务内把命中的 `file_path` 置空、`status='deleted'`、重写 `payload_enc`，返回路径。
- [ ] 新增测试：
  - `attachment_count_only_counts_incoming_with_path`
  - `received_file_paths_filters_direction_and_kind`
  - `clear_received_file_paths_marks_deleted_and_keeps_other_peer`
  - `single_file_message_payload_is_encrypted_at_rest`（文件名/路径明文不出现在 payload_enc）
- [ ] 运行：`cargo test -p ws-workbench --lib lanshare::chat_store`，全绿后提交。

## Task 2: 删除会话 / 删除附件（后端）

**Files:**
- Modify: `src-tauri/src/lanshare/mod.rs`
- Modify: `src-tauri/src/lanshare/chat_store.rs`（如缺事务辅助）
- Modify: `src-tauri/src/commands/lanshare.rs`
- Modify: `src-tauri/src/lib.rs`（注册命令）
- Test: `src-tauri/src/lanshare/mod.rs` 单测

- [ ] `mod.rs` 增加 `fn delete_local_files(paths: &[String]) -> usize`：只删普通文件（跳过目录/符号链接），`trash::delete` 失败回退 `std::fs::remove_file`，返回成功数。
- [ ] 改 `LanShareState::delete_conversation(app, peer_id, delete_files: bool)`：需要时先取路径 → 删记录 → 删文件 → `emit_chat("deleted")`。
- [ ] 加 `LanShareState::delete_attachments(app, peer_id) -> Result<u32, String>`：`clear_received_file_paths` → 删文件 → `emit_chat("message")` → 返回数量。
- [ ] 加 `LanShareState::attachment_count(peer_id)` 包装。
- [ ] 命令：`lan_share_delete_conversation(app, peer_id, delete_files)`、`lan_share_delete_attachments`、`lan_share_conversation_attachment_count`；在 `lib.rs` 注册。
- [ ] 测试：`delete_local_files_skips_directories_and_missing`、`delete_conversation_without_files_keeps_files`、`delete_conversation_with_files_removes_received_file`（tempdir 造文件）。
- [ ] 运行 `cargo test -p ws-workbench --lib lanshare` 后提交。

## Task 3: 剪贴板与拖拽落盘

**Files:**
- Create: `src-tauri/src/lanshare/outbox.rs`
- Modify: `src-tauri/src/lanshare/mod.rs`（`mod outbox;`、`send_dropped_files`、`import_clipboard_files`）
- Modify: `src-tauri/src/commands/lanshare.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `outbox.rs` 单测

- [ ] `outbox.rs`：
  - `pub const DROPPED_MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;`
  - `pub fn staging_dir(app: &AppHandle) -> Result<PathBuf, String>`（`crate::db::storage_dir(app)?/lan_share_outbox`，自动创建）。
  - `pub fn save_dropped_files(app, files: &[DroppedFileInput]) -> Result<Vec<String>, String>`：校验数量/大小、base64 解码、`sanitize_file_name`、uuid 前缀防重名。
  - `pub fn save_clipboard_image(app, img: &arboard::ImageData) -> Result<String, String>`：`image` crate 编码 PNG，文件名 `剪贴板图片_YYYYMMDD-HHmmss.png`。
  - `pub fn clipboard_paths(app) -> Result<Vec<String>, String>`：`arboard::Clipboard` 先 `get().file_list()`（过滤普通文件、目录报错），再图片；空则 Err。
- [ ] `mod.rs` 加 `send_dropped_files(app, peer_id, files) -> Result<String, String>`：落盘后调用 `self.send_files(app, peer_id, paths)`。
- [ ] 命令 `lan_share_import_clipboard_files() -> Vec<String>`、`lan_share_send_dropped_files(app, peer_id, files)`；注册。
- [ ] 测试：`save_dropped_files_rejects_oversize`、`save_dropped_files_sanitizes_name`、`save_clipboard_image_writes_png`（用构造的 RGBA）、`base64_roundtrip`。
- [ ] `cargo test -p ws-workbench --lib lanshare::outbox` 后提交。

## Task 4: 事件 direction、Dock 角标、快捷面板模式与托盘左键

**Files:**
- Modify: `src-tauri/src/lanshare/mod.rs`（`ChatEvent`、`emit_chat`、`emit_chat_with_direction`）
- Modify: `src-tauri/src/lanshare/client.rs`、`server.rs`（message 事件带 direction）
- Modify: `src-tauri/src/lib.rs`（托盘点击、快捷面板模式、启动角标）
- Modify: `src-tauri/src/commands/quick.rs`（`quick_panel_take_mode`、`quick_panel_open_chat`）
- Test: `lib.rs` 快捷面板单测文件（`mod quick_panel_tests`）

- [ ] `ChatEvent { peer_id, kind, #[serde(default)] direction: Option<String> }`；`emit_chat` 保持无方向；加 `emit_chat_with_direction(inner, app, peer_id, kind, direction)`。
- [ ] 收到的消息（server handle_chat / handle_upload）用 `"in"`；本地发送消息（send_chat / send_files 初始插入）用 `"out"`；状态更新/删除/已读保持 None。
- [ ] `emit_chat` 内更新 Dock 角标：总量来自 chat_store（先 clone store 出锁再查询，禁止持锁做 IO）；仅 macOS 调 `main window.set_badge_count`；忽略错误。
- [ ] 启动时（lanshare state 初始化后）计算一次角标；`mark_read` / 删除命令后也刷新（可复用 emit 或单独函数）。
- [ ] 快捷面板模式：`static QUICK_PANEL_MODE: AtomicBool`；`quick_panel_take_mode() -> &'static str` 返回并重置；显示面板后发 `quick-panel://mode`。
- [ ] 托盘左键：`total_unread > 0` → 消息模式显示/收起面板；否则原逻辑 `show_main_window`。右键：重置 quick 模式并 toggle。
- [ ] `quick_panel_open_chat(app, peer_id)`：hide 面板 → `show_main_window` → emit `lan-share://open-chat`。
- [ ] 测试：模式原子量 take 后重置、open-chat 事件载荷序列化（可直接测构造 + serde）。
- [ ] `cargo test -p ws-workbench --lib` 后提交。

## Task 5: 前端 API 类型与封装

**Files:**
- Modify: `src/api/lanshare.ts`

- [ ] `ChatMessage.status` 增加 `'deleted'`；`ChatEvent.direction?: 'in' | 'out' | null`。
- [ ] 新增封装：`deleteLanShareConversation(peerId, deleteFiles)`、`deleteLanShareAttachments(peerId)`、`getLanShareAttachmentCount(peerId)`、`importLanShareClipboardFiles()`、`sendLanShareDroppedFiles(peerId, files)`。
- [ ] `npx vue-tsc --noEmit` 通过后提交。

## Task 6: 左栏右键菜单与删除弹窗

**Files:**
- Modify: `src/components/lanshare/LanShareSidebar.vue`
- Modify: `src/components/lanshare/LanSharePanel.vue`
- Modify: `src/components/lanshare/useLanShareActions.ts`
- Modify: `src/components/lanshare/LanFileMessage.vue`
- Create: `src/components/lanshare/LanPeerContextMenu.vue`（或把菜单内联在 Sidebar，但不超过职责）

- [ ] Sidebar：删除 `.delete-btn` 及样式；移除列表项/在线点 `title`；新增 `@contextmenu.prevent` → emit `context-menu [item, x, y]`。
- [ ] 菜单组件：三项（删除聊天记录 / 删除附件 / 免打扰模式），带禁用态与勾选态；外部点击/Esc/滚动关闭。
- [ ] Panel：右键打开菜单时查附件数（`getLanShareAttachmentCount`）；「删除附件」无文件禁用。
- [ ] 删除会话弹窗：确认文案 + 附件数 >0 时复选框（默认不勾选）；调 `deleteLanShareConversation`；刷新并处理选中态。
- [ ] 删除附件弹窗：确认后调 `deleteLanShareAttachments`，toast 实际删除数量，刷新消息。
- [ ] `LanFileMessage.vue`：`status==='deleted'` 显示「已删除」，隐藏打开/文件夹/重试按钮。
- [ ] 免打扰项调用 notify composable（Task 7 提供），先接 props/emits。
- [ ] `npx vue-tsc --noEmit` 通过后提交。

## Task 7: 提示音与免打扰

**Files:**
- Create: `src/components/lanshare/useLanShareNotify.ts`
- Modify: `src/components/lanshare/useLanShareChat.ts`（chat 事件触发）
- Modify: `src/components/lanshare/LanShareSettings.vue`（开关）
- Modify: `src/components/lanshare/LanSharePanel.vue`（右键免打扰接线）

- [ ] composable：`dnd` 从 localStorage 初始化；`setDnd/toggleDnd` 写回；`playMessageSound`（WebAudio，双音，异常吞掉）；提供 `unlockAudio()` 在首次 pointerdown 时 resume。
- [ ] `useLanShareChat` 的 `handleChatEvent`：`kind==='message' && direction==='in'` → `playMessageSound()`（DND 时内部跳过）。
- [ ] 设置页加「消息提示音」卡片开关（开=非免打扰）。
- [ ] 右键「免打扰模式」勾选态与开关双向同步。
- [ ] `npx vue-tsc --noEmit` 后提交。

## Task 8: 输入框粘贴与拖拽

**Files:**
- Modify: `src/components/lanshare/LanChatPane.vue`
- Modify: `src/components/lanshare/useLanShareActions.ts`（新增 `handleSendPaths`）
- Modify: `src/components/lanshare/LanSharePanel.vue`（接线）

- [ ] `LanChatPane` 新增 emits：`send-paths`（paths: string[]）、`send-dropped-files`（files: {name,dataBase64}[]）。
- [ ] textarea `@paste`：见 spec §5.3；调用 `importLanShareClipboardFiles`，成功 emit `send-paths`，失败 `message.warning`。
- [ ] 聊天区 `dragenter/over/leave/drop` + 高亮遮罩；目录跳过、>100MB 提示、File→base64（FileReader）→emit `send-dropped-files`。
- [ ] Panel：`send-paths` → 现有 sendFiles 逻辑（路径数组）；`send-dropped-files` → `sendLanShareDroppedFiles`。离线/未开启时提示。
- [ ] `npx vue-tsc --noEmit`、`npx vite build` 后提交。

## Task 9: 快捷面板消息列表 + 定位

**Files:**
- Modify: `src/quick/QuickPanel.vue`
- Modify: `src/App.vue`（监听 `lan-share://open-chat` → `view='lanshare'`）
- Modify: `src/components/lanshare/useLanShareChat.ts`（监听 open-chat → selectPeer）
- Modify: `src/api/quick.ts`（如存在）或新增事件封装

- [ ] QuickPanel：第三页签「消息」+ 未读总数红点；列表（显示名/备注、预览、时间、未读数）；空态；点击调 `quick_panel_open_chat`。
- [ ] 挂载时 `quick_panel_take_mode()` 决定初始 tab；监听 `quick-panel://mode` 切页签；窗口 focus / `lan-share://chat` 时刷新列表；消息页签调 `quick_panel_resize` 适配高度。
- [ ] App.vue 监听 `lan-share://open-chat`：切 `view='lanshare'`。
- [ ] useLanShareChat 监听 open-chat：等待快照后 `selectPeer(peer_id)`；找不到时只切页签并提示。
- [ ] `npx vue-tsc --noEmit`、`npx vite build` 后提交。

## Task 10: 全量验证与文档

- [ ] `cargo test`（全部）、`npx vue-tsc --noEmit`、`npx vite build` 全绿。
- [ ] 自查 spec §10 手动清单，逐项标注可由代码/测试覆盖或需人工验证。
- [ ] 更新 spec/plan 中与实际实现不一致之处。
- [ ] 提交（`feat(lanshare): ...`），交给 verifier 复核。
