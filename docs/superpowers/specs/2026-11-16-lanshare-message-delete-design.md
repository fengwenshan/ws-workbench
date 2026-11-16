# LANLink · 消息删除与列表可见性（补充需求 14）

日期：2026-11-16
前置：`2026-11-16-lanlink-ui-refresh.md`（13 项 UI 刷新）

## 1. 需求

1. 只备注了对端、没有任何聊天消息、且对端离线 → **不出现在左栏列表**。
2. 聊过天（有聊天记录）、即使对端离线 → **仍然显示在左栏**。
3. 「清除聊天记录」等同于没聊过天：清空后若对端离线就应从列表消失。
4. 聊天消息支持右键删除（单条）。

当前第 1/2/3 条的合并逻辑已经满足（`mergedPeers` = 会话 ∪ 在线 peer；删除会话会删掉会话行）。本轮重点是把第 4 条做出，并补测试/回归，确保删到「最后一条」时会话行也删掉，从而第 1/3 条在消息维度同样成立。

## 2. 可见性规则（固化）

- 列表项来源只有两个：
  - `lan_share_conversations` 里的会话（有至少一条历史消息时后端才会留下会话行）；
  - `lan_share_snapshot.peers` 里当前在线的设备。
- 本机备注（`peer_remarks`）只是显示名，**不产生列表项**；离线 + 无会话的设备无论是否有备注都不显示。
- `list_conversations` 只返回 `EXISTS` 至少一条消息的会话；接收文件前 `upsert_conversation` 写的占位会话不算「聊过天」。
- 删除聊天记录 / 删除最后一条消息后会话行被删 → 离线对端立即从列表消失；在线对端会以上述「在线 peer」身份继续显示（摘要「在线 · 发消息或文件」）。

## 3. 删除单条消息

### 3.1 交互

- 在消息气泡 / 文件卡上右键 → 菜单只有「删除消息」（危险色）。
- 点菜单 → `Modal.confirm`：
  - 文本消息：`删除这条消息？只影响本机聊天记录，对方设备不受影响。`
  - 文件消息且是本机收到的文件：`删除这条消息？只移除聊天记录，本机已收到的文件不会删除；对方设备不受影响。`
- 确认后调命令，成功后刷新消息与会话，1 条 toast「已删除」。
- 删除失败 toast 错误原因；消息保留。
- 正在发送中的消息也可删：后台发送线程回写状态时找不到消息会静默忽略（现有逻辑）。

### 3.2 存储语义（`ChatStore::delete_message`）

事务内：

1. 查目标消息（`peer_id + id`），不存在 → `Ok(false)`（幂等，不报错）。
2. 删除该消息行。
3. 若是 `direction=in`，会话未读 `unread = MAX(unread - 1, 0)`（没有按条已读标记，只能近似扣减）。
4. 查剩余最新一条：
   - 有：更新会话 `last_activity_ms = 最新.created_at_ms`，`last_preview_enc = seal(preview_of(kind,text,file_name))`；
   - 没有：**删除该会话行**（等价于清除聊天记录，使离线对端不再出现）。
5. 本机已收到的文件**不删除**（物理文件保留，只删消息记录）。

### 3.3 契约

- 新命令：`lan_share_delete_message(peer_id: String, message_id: String) -> Result<(), String>`
  - trim 校验两个参数；空 → 明确错误。
  - 内部调用 `ChatStore::delete_message`，随后发事件。
- 事件沿用 `lan-share://chat { peer_id, kind: "updated" }`（不响提示音、不标已读；前端刷新消息和会话列表）。
- 前端封装：`deleteLanShareMessage(peerId, messageId): Promise<void>`。

## 4. 前端结构

- 新增 `useLanShareMessageMenu.ts`：菜单状态（message / x / y）、`openMessageMenu`、`closeMessageMenu`、`requestDeleteMessage`（Modal.confirm + 调 API + 刷新）。
- 新增 `LanMessageContextMenu.vue`：fixed 定位 + Teleport，样式复用左栏右键菜单的观感（危险色「删除消息」）。
- `LanChatMessageList.vue`：`.msg-row` 上 `@contextmenu.prevent`，emit `(message, x, y)`；不做删除逻辑。
- `LanChatPane.vue`：转发 `message-context-menu` 事件 + 渲染菜单。
- `LanSharePanel.vue`：接 `useLanShareMessageMenu`，把菜单 props/事件接到 ChatPane。
- 删除后若 `activePeerId` 已不在 `mergedPeers` 中（离线对端最后一条被删），清空选中回到空态。

## 5. 验收

- 后端单测：删中间消息不改其它消息；删最后一条删会话行；删入站未读消息未读减 1 且不为负；删不存在的消息幂等；删除后重开会话列表为空。
- `cargo test` 全绿；`npx vue-tsc --noEmit`、`npx vite build` 通过。
- 手动：备注一台离线设备 → 列表不显示；与它聊一句后离线 → 显示；删最后一条 / 清除聊天记录 → 离线项消失；在线设备仍在列表；消息右键 → 删除 → 消息消失、文件不被删。
