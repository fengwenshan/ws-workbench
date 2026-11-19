# 局域网分享聊天化改造 实施计划

> 设计依据：`docs/superpowers/specs/2026-11-16-lan-share-chat-redesign.md`
> 执行方式：Team Lead + 2 名实现 teammate + 1 名验证 teammate，文件所有权划分见 §2。

**目标：** 局域网分享页改为「左侧机器列表 + 右侧聊天」；设备身份改为 MAC；修复在线点灰点。

**架构：** 后端在现有 `lanshare` 模块上加 `identity` / `chat_store`，HTTP 新增 `/chat`，文件发送/接收写入会话消息；前端拆成壳 + 左栏 + 聊天 + 设置四个组件，事件驱动刷新。

**技术栈：** Rust（Tauri 2 / rusqlite / std net）、Vue 3 + TypeScript + ant-design-vue、SQLite（vault 加密信封）。

---

## 1. 文件所有权（避免并行写冲突）

| 所有者 | 文件 | 说明 |
| --- | --- | --- |
| Lead | `src-tauri/src/lanshare/{identity.rs, protocol.rs, discovery.rs, server.rs, client.rs, mod.rs}`、`src-tauri/src/commands/lanshare.rs`、`src-tauri/src/lib.rs` | 身份、在线修复、协议、命令、集成 |
| backend-chat-store | `src-tauri/src/lanshare/chat_store.rs`、`src-tauri/src/db.rs` | 聊天存储 + schema v4 |
| frontend-chat-ui | `src/api/lanshare.ts`、`src/components/lanshare/*` | 全部前端 |
| verifier | 只读（问题回报，不改文件） | 独立验证 |

规则：同一文件同一时间只有一个所有者；teammate 不执行 `git commit` / `git add`（由 Lead 统一提交，避免 index 竞争）。

## 2. 后端接口契约（先钉死，双方按此实现）

### 2.1 `chat_store.rs`（backend-chat-store 实现，Lead 已提供骨架）

```rust
pub(crate) struct ChatStore { /* path + keys */ }
impl ChatStore {
    pub fn open(app: &AppHandle) -> Result<Self, String>;
    pub fn new(path: PathBuf, keys: VaultKeys) -> Self;
    pub fn list_conversations(&self) -> Result<Vec<ChatConversation>, String>;
    pub fn get_conversation(&self, peer_id: &str) -> Result<Option<ChatConversation>, String>;
    pub fn upsert_conversation(&self, peer_id: &str, alias: &str, activity_ms: i64) -> Result<(), String>;
    pub fn append_message(&self, peer_id: &str, alias: &str, message: &ChatMessage) -> Result<(), String>;
    pub fn list_messages(&self, peer_id: &str, limit: usize) -> Result<Vec<ChatMessage>, String>;
    pub fn update_message_status(&self, peer_id: &str, message_id: &str, status: &str, error: Option<&str>, file_path: Option<&str>) -> Result<(), String>;
    pub fn mark_read(&self, peer_id: &str) -> Result<(), String>;
    pub fn delete_conversation(&self, peer_id: &str) -> Result<(), String>;
}
```

`ChatMessage` / `ChatConversation` 字段严格按设计文档 §4.4（serde snake_case，前端同名）。

### 2.2 Lead 侧接线点

- `Inner.chat: Option<ChatStore>`；`from_app` 里 `ChatStore::open`，失败写 `warning`。
- `LanShareState::conversations / messages / send_chat / mark_read / delete_conversation` 公开方法，命令层只转交。
- server：`handle_chat`、`handle_prepare` / `handle_upload` 里的 `upsert_conversation` / `append_message`。
- client：`run_send_chat`；`collect_files` 改成 `pub(crate)`。
- `emit_chat(inner, app, peer_id, kind)`。

## 3. 任务

### Task 1（backend-chat-store）：聊天存储 + schema v4

Files：`src-tauri/src/lanshare/chat_store.rs`（实现骨架里的 `todo!()`）、`src-tauri/src/db.rs`（v4）。

步骤：

1. 在 `db.rs` 增加 `SCHEMA_SQL_V4`、`SCHEMA_VERSION = 4`、`init_schema` 的 `if version < 4` 分支；更新 db 测试里对版本号的断言（v1/v2 → 4、新表存在、v5 拒绝）。
2. 实现 `ChatStore`：`open` 用 `crate::db::db_path` + `crate::crypto::vault_keys`，先 `db::open` 跑迁移；`append_message` 事务内 upsert 会话 + 插入消息（payload 用 `crypto::encrypt_with_keys`）；`list_conversations` 解密预览；`list_messages` 按时间正序取最近 N 条；`update_message_status` 解密-改-再加密；`mark_read` / `delete_conversation` 事务。
3. 测试（`#[cfg(test)] mod tests`，`ChatStore::new(temp_path, test_keys)`）：
   - 追加 in 消息 → 会话出现、unread=1、预览正确；追加 out 消息 → unread 不增加；
   - `list_messages` 顺序、`update_message_status` 生效、`mark_read` 清零、`delete_conversation` 级联删除；
   - 预览 / 正文在数据库里不是明文（原始 payload 含 `v1.` 前缀即可）。
4. 验证：`cd src-tauri && cargo test lanshare::chat_store`、`cargo test db::`。

### Task 2（Lead）：身份 + 在线修复 + 协议/服务端/客户端/命令

1. 新增 `identity.rs`（见设计 §3）+ 单测；`from_app` 里用 MAC 覆盖 `settings.fingerprint` 并回写。
2. `mod.rs`：`upsert_peer` 返回 `PeerUpdate`；`Inner.chat`；chat 相关方法；`LanShareConversation` 合并 online/别名；事件常量 `EVENT_CHAT` + `emit_chat`。
3. `discovery.rs`：`dirty |= update.changed || update.seen`；`changed` 才探测/回包。
4. `protocol.rs`：`ChatRequest` / `ChatResponse`；`server.rs`：`CHAT_PATH` + `handle_chat` + 文件消息接线；`client.rs`：`run_send_chat` + `collect_files` 可见性 + 发送文件消息状态更新。
5. `commands/lanshare.rs` 新增 5 命令；`lib.rs` 注册。
6. 测试：在线续期回归、`/chat` 校验、两实例端到端聊天、文件消息生成；跑 `cargo test lanshare::`。

### Task 3（frontend-chat-ui）：两栏聊天 UI

Files：`src/api/lanshare.ts`、`src/components/lanshare/{LanSharePanel,LanShareSidebar,LanChatPane,LanShareSettings}.vue`。

1. `api/lanshare.ts`：新增 `ChatMessage` / `ChatConversation` / `ChatEvent` 类型、5 个命令、`EVENT_CHAT` + `onLanShareChat`。
2. `LanSharePanel.vue`：维护 snapshot、conversations、activePeerId、pane（chat/settings）、messages、progress map；事件订阅与生命周期（沿用现有 v-show 常驻模式）；合并列表（在线 peer ∪ 会话，online 优先 + 最近活动降序）；动作（选中/已读/发文本/发文件/接受拒绝/删除会话/设置项操作）全部调 API。
3. `LanShareSidebar.vue`：标题、列表、在线点、摘要/时间/未读、hover 删除、底部设置入口 + 运行状态点；props/emits 明确。
4. `LanChatPane.vue`：头部（名称/状态/IP/发送文件/删除）、消息流（气泡、文件卡片带进度、待接收卡片、时间分隔）、输入区（Enter/Shift+Enter、离线禁用、📎）、空态。
5. `LanShareSettings.vue`：原图一本机信息卡内容（别名编辑、状态+开关、端口、地址、设备 ID、接收目录选择/打开/恢复默认、错误/警告）。
6. 验证：`npx vue-tsc --noEmit`、`npx vite build`；对照设计 §2 自查排序与空态。

### Task 4（Lead + verifier）：集成与验证

1. Lead 合并后跑：`cd src-tauri && cargo test`（全量）、`npx vue-tsc --noEmit`、`npx vite build`。
2. verifier 独立执行：编译/测试、对照设计文档检查契约（字段名、事件、排序、在线点、删除、设置页）、抽查代码正确性（并发/锁、SQL 事务、错误路径），输出问题清单。
3. Lead 修复问题 → 复跑全部验证 → 汇报。

## 4. 验收清单

- [ ] 两台设备在线时列表点持续为绿；断网/退出 10 秒内变灰、15 秒后从在线列表消失（有历史的保留为离线会话）。
- [ ] 列表排序：在线在前，组内按最近聊天时间倒序。
- [ ] 聊过天/传过文件的设备持久出现在列表（重启后仍在），删除后消失，对方再发消息重新出现。
- [ ] 文本消息双端可达并持久化；离线发送显示失败原因。
- [ ] 文件发送有进度、完成后双端都有文件消息；接收请求在聊天里可接受/拒绝。
- [ ] 设置页含设备名编辑、开关、端口、地址、MAC 设备 ID、接收目录。
- [ ] `cargo test`、`vue-tsc`、`vite build` 全绿。
