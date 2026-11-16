# 局域网分享聊天区视觉升级（钉钉式）实施计划

- 日期：2026-11-16
- 设计：`docs/superpowers/specs/2026-11-16-lan-share-chat-visual.md`
- 分支：beta
- 目标：聊天区头像/昵称/时间/文件卡对齐钉钉观感；文件默认直接接收（可开关确认）；本机备注只本机显示。

## 1. 总体分工

| 任务 | 负责人 | 写作用域 |
| --- | --- | --- |
| 后端：设置字段、快照字段、备注/开关方法、`handle_prepare` 免确认分支、打开文件夹命令、测试 | Lead | `src-tauri/src/lanshare/mod.rs`、`src-tauri/src/lanshare/server.rs`、`src-tauri/src/commands/lanshare.rs`、`src-tauri/src/lib.rs` |
| 前端：API 类型/命令、消息行头像昵称时间、文件卡重做、聊天头部备注编辑、设置页确认勾选 | frontend-chat-ui | `src/api/lanshare.ts`、`src/components/lanshare/` |
| 独立验证：命令全跑 + 契约/代码审查 + 交互自查 | verifier | 只读 |

Lead 负责最终集成、修复、提交。

## 2. 契约（先定，双方照此实现）

### 2.1 数据字段（serde snake_case）

`LanShareSettings` 新增：

```rust
#[serde(default)]
pub require_accept: bool,            // 默认 false：直接接收
#[serde(default)]
pub peer_remarks: std::collections::HashMap<String, String>,
```

`LanShareSnapshot` 新增：

```rust
pub require_accept: bool,
pub peer_remarks: std::collections::HashMap<String, String>,
```

前端类型同步：`LanShareSnapshot.require_accept: boolean`、`peer_remarks: Record<string, string>`。

### 2.2 命令（Tauri invoke 参数 camelCase）

| invoke | 参数 | 返回 |
| --- | --- | --- |
| `lan_share_set_peer_remark` | `{ peerId, remark }` | `LanShareSnapshot` |
| `lan_share_set_require_accept` | `{ requireAccept }` | `LanShareSnapshot` |
| `lan_share_open_file_dir` | `{ path }` | `void` |

### 2.3 显示名规则

`备注(peer_remarks[id]) || 最新 announce 别名 || 会话别名 || '未知设备'`。

- 左栏列表项、聊天头部统一使用；
- 备注 key = 设备指纹（MAC / 回退 UUID）；
- 备注 trim + 最多 32 字符，空串即删除。

### 2.4 文件状态文案

- out：`sending` → 发送中 + 进度；`sent` → 已发送；`failed` → 发送失败 + 重试。
- in：`received` → 已保存；有 `file_path` 时给「打开」「打开文件夹」。
- 不再出现「未查收 / 已接收 / 下载」。

## 3. 后端任务（Lead）

### 3.1 `mod.rs`

1. `LanShareSettings` 加 `require_accept` / `peer_remarks` 字段（含 serde default）。
2. `LanShareSnapshot` 加对应两个字段，`snapshot()` 组装。
3. `LanShareState::set_peer_remark(&self, peer_id, remark) -> Result<LanShareSnapshot, String>`：
   - `peer_id` 空串直接拒绝；备注 trim、按字符截断 32；空串从 map 删除；
   - 先克隆设置、在克隆上改备注，**锁外** `settings::save`；
   - 保存成功后再取锁只改内存对应字段；失败返回错误且内存保持旧值（与设计 §7 一致，不做乐观更新）。
4. `LanShareState::set_require_accept(&self, value) -> Result<LanShareSnapshot, String>`：同上（锁外保存，失败不改内存）。
5. 备注修改后不新增事件：命令返回最新快照给前端 apply；后端同时 emit 一次 snapshot。

### 3.2 `server.rs` `handle_prepare`

- 在「登记 pending + 等确认」前读取 `require_accept`：
  - `false`：跳过 pending 与 `recv_timeout`，直接走「建目录 + 建 session + token + 返回 200」。把接受后逻辑抽成 `fn attach_session(inner, files, peer_id, peer_alias) -> Result<(String, Vec<FileToken>), (u16, String)>`；内部在插入 session 的同一把锁里先 `prune_sessions()`（免确认路径不经过 pending 那段），事件由调用方 emit。
  - `true`：现有待接收流程不变。
- 自动接收路径不产生 `pending`、不推「待接收」相关快照（会话 upsert 与 chat 事件保留）。
- 注意锁：不得在 `lock_inner` 内做 DB IO（沿用上一轮修复的写法）。
- 失败清理：建目录/session 失败返回 5xx，不留下半截状态。

### 3.3 `commands/lanshare.rs` + `lib.rs`

- 3 个新命令（见 §2.2）；`lan_share_open_file_dir` 取 `Path::parent()`，复用 `crate::commands::system::open_dir`。
- `lib.rs` 在 lanshare 命令组后注册。

### 3.4 后端测试

- settings serde：空 JSON / 旧字段 JSON → `require_accept=false`、`peer_remarks` 空。
- 备注：设置 / 覆盖 / 清空、trim 截断、快照带出（normalize/apply 纯函数单测；peer_id 空串拒绝由方法内校验，因依赖 AppHandle 无单测）。
- `prepare` 自动接收：临时 state（`require_accept=false`）→ 200 且返回 token、`pending` 为空、上传后落盘并清掉过期 session；`require_accept=true` 走 pending（现有测试显式开启确认）。server 单测的 state 不接 chat store，`received` 消息由 e2e 确认路径覆盖（upload 两条路径共用）。
- e2e 文件测试：显式开启确认，保留原「等待 pending + 接受」路径；免确认路径由 server 单测覆盖。
- `open_file_dir`：只单测 `parent_dir(path)` 纯函数（绝对 / 相对 / 空路径），真实「打开文件夹」不纳入单测（会拉起 Finder）。

## 4. 前端任务（frontend-chat-ui）

### 4.1 `src/api/lanshare.ts`

- 快照类型加 `require_accept` / `peer_remarks`；
- 新增 `setLanSharePeerRemark(peerId, remark)`、`setLanShareRequireAccept(requireAccept)`、`openLanShareFileDir(path)`。

### 4.2 组件

- 新增 `LanAvatar.vue`：props `self: boolean`；自己人形图标，对方显示器图标；纯展示。
- `LanChatMessageList.vue`：
  - 每条消息渲染「头像 + 内容」两栏；内容上方昵称行（对方 `备注 || 设备名`、自己「我」）+ HH:mm；
  - 气泡样式按设计 §2.1；
  - 文件卡交给 `LanFileMessage`，传入显示名/方向不变；
  - 日期分隔、失败重试、自动滚底保留。
- `LanFileMessage.vue`：按设计 §2.2 重做；「打开」用 `openFile`，「打开文件夹」用新命令；失败重试保留；进度条保留。
- `LanChatPane.vue`：头部左侧加头像，名字按显示名规则；名字旁铅笔按钮 → 内联输入（Enter 保存 / Esc 取消 / 清空=删除备注），保存调用 `setLanSharePeerRemark` 并 `applySnapshot`；保存失败 `message.error`。
- `LanShareSettings.vue`：新增「接收文件前需要确认」勾选（默认不勾选），切换调用 `setLanShareRequireAccept` 并 `applySnapshot`；失败回滚 UI（重新拉快照）。
- `useLanShareChat.ts`：`mergedPeers` 显示名应用 `snapshot.peer_remarks`；导出 `savePeerRemark` / `setRequireAccept` 或交给 actions。
- `types.ts`：`MergedPeer` 增加 `remark: string`（可选）供展示；`useLanShareActions.ts` 增加保存备注动作。

### 4.3 前端自查

- 头像左右正确、自己/对方图标区分；昵称/时间不换行错位；
- 文件卡四态：发送中（进度）、已发送、已保存（打开/打开文件夹）、失败（原因+重试）；
- 备注编辑后左栏与头部即时变化；清空恢复设备名；
- 设置勾选默认关闭；勾选后接收文件出现待接收卡/左栏提示，取消后直接接收；
- 小窗口（<760px）不溢出。

## 5. 独立验证（verifier，只读）

1. `git status --short` / `git diff --stat` 核对改动面。
2. `cd src-tauri && cargo check --lib --tests && cargo test`（全绿，记录数量）。
3. `npx vue-tsc --noEmit && npx vite build`。
4. 契约核对：字段名、命令名/参数、显示名优先级、文件状态文案、备注不广播（检查协议结构体无 remark 字段）、默认 `require_accept=false`。
5. 代码审查：`handle_prepare` 两条路径是否共用建 session 代码、锁内无 DB IO、自动接收失败清理、前端事件刷新、备注持久化失败处理。
6. 输出：命令结果 + 问题清单（P0/P1/P2，文件:行号 + 建议）+ 可否交付结论。

## 6. 验收清单

- [ ] `cargo test` 全绿（新增：settings 缺省、备注、自动接收、确认路径）。
- [ ] `vue-tsc` / `vite build` 通过。
- [ ] 默认不勾选「接收文件前需要确认」，双击即传、接收端直接落盘。
- [ ] 勾选后恢复待接收确认流程。
- [ ] 聊天区头像/昵称/时间/文件卡按设计呈现；无「查收/下载」字样。
- [ ] 本机备注可编辑、只本机生效、左栏与头部同步。
- [ ] 独立 verifier 判定可交付。
- [ ] Lead 提交代码并在最终汇报中说明。
