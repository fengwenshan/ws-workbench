# 局域网分享 · 会话体验增强（设计）

日期：2026-11-16
状态：已确认（用户确认「托盘行为 A，开工吧」）
范围：左栏列表交互、会话删除、消息提示音/免打扰、粘贴与拖拽发送、托盘消息列表与未读角标。

## 1. 背景与目标

局域网分享已经有双栏聊天、文件收发、本地备注和通知式文件卡。这一轮解决下面这些体验问题：

1. 左栏 hover 删除按钮遮挡时间，且长悬浮出现原生黑色提示；删除入口要移到右键菜单。
2. 删除聊天记录需要二次确认；会话里有收到的文件时可以勾选同时删除本机文件。
3. 新消息需要内置提示音，并提供免打扰开关（设置页 + 左栏右键菜单）。
4. 输入框要能粘贴截图、粘贴从 Finder 复制的文件、拖拽文件直接发送。
5. 托盘图标要有未读角标；有未读时左键点托盘弹出「消息列表」，点一条定位到会话（钉钉/微信式）。

## 2. 已确认的产品决策

- 删除附件只删「本机收到的文件」；我主动发送的原文件、粘贴/拖拽生成的暂存副本不删。
- 富文本本轮不做工具栏；输入框保持多行纯文本，只增强粘贴/拖拽。图片作为文件消息发送。
- 提示音只在收到对方消息/文件时响；默认开启；免打扰是全局开关。
- 托盘左键：有未读 → 弹消息列表；无未读 → 打开主界面（方案 A）。
- 右键托盘保持现有 VPN/休眠快捷面板。
- 所有删除都只影响本机，A 清空聊天不影响 B。

## 3. 左栏列表与右键菜单

### 3.1 列表项

- 删除 hover 删除按钮（`.delete-btn`）及其样式。
- 移除列表项、在线点上的 `title`（原生长悬浮提示）。
- 保留：显示名 `remark || alias`、最后消息预览、时间、待接收徽标、未读数红点。
- 无聊天记录的设备离线后从列表消失（现有行为，不改）。

### 3.2 右键菜单

在左栏列表项上右键弹出自定义菜单（`position: fixed`，跟随鼠标）：

| 菜单项 | 出现条件 | 行为 |
| --- | --- | --- |
| 删除聊天记录 | 该设备有会话 | 打开二次确认弹窗 |
| 删除附件 | 该设备有会话（无收到的文件时点击提示） | 二次确认后删除本机收到的文件 |
| 免打扰模式 | 始终 | 勾选态 = 免打扰开启，点击切换并持久化 |

- 点击菜单外部 / Esc / 滚动列表时关闭。
- 菜单打开时先异步查一次附件数量：`lan_share_conversation_attachment_count`；查询中「删除附件」置灰，数量 0 时置灰。

### 3.3 删除聊天记录弹窗

- 标题「删除聊天记录」；正文说明「只删除本机记录，对方不受影响」。
- 若附件数量 > 0，显示复选框：「同时删除本机收到的 N 个文件（移入废纸篓）」，**默认不勾选**。
- 确认按钮调用 `lan_share_delete_conversation(peerId, deleteFiles)`；成功后刷新会话、消息与选中态。
- 被删除的会话如果是当前选中会话：回退到空态（或该设备仍在线的会话空态），不选中别的会话。

### 3.4 删除附件

- 二次确认弹窗：「删除本机收到的 N 个文件（移入废纸篓），聊天记录保留」。
- 调用 `lan_share_delete_attachments(peerId)`，返回实际删除数量；刷新消息和会话。
- 被删除的文件消息：`status = deleted`、`file_path = null`，文件卡显示「已删除」，不显示打开/打开文件夹/重试按钮。

### 3.5 文件删除实现

- 只处理 `direction = in`、`kind = file`、`file_path` 非空的记录。
- 删除前校验：路径存在、`symlink_metadata` 不是目录/符号链接、`metadata.is_file()`。
- 优先 `trash::delete` 移入废纸篓；失败时回退 `std::fs::remove_file`；单个文件失败只记日志，不阻断整体。
- 删除聊天记录时先取路径、再删数据库、最后删文件（删文件失败不影响记录已删）。

## 4. 提示音与免打扰

- 新增前端单例 `src/components/lanshare/useLanShareNotify.ts`：
  - `dnd`：`ref<boolean>`，持久化到 `localStorage['ws-workbench:lanshare:dnd']`（`'1' / '0'`）。
  - `toggleDnd()` / `setDnd(value)`。
  - `playMessageSound()`：WebAudio 生成 880Hz → 660Hz 两段短音（约 0.18s），无音频资源文件；首次用户手势后 `resume()` 音频上下文。
- 触发：`lan-share://chat` 事件 `kind === 'message' && direction === 'in'` 时播放；DND 开启时跳过。
- 自己发送、状态更新、历史刷新、删除/已读事件不响。
- 设置页 `LanShareSettings.vue` 增加「消息提示音」开关（默认开）；和右键「免打扰模式」共享同一状态（勾选=免打扰=关闭声音）。
- 文案：开关标题「消息提示音」，描述「收到对方消息时播放提示音」。

## 5. 粘贴与拖拽发送

### 5.1 后端命令

- `lan_share_import_clipboard_files() -> Result<Vec<String>, String>`
  - 优先读系统剪贴板文件列表（Finder 复制文件）：过滤为存在的普通文件；全是目录则报错「暂不支持发送文件夹」。
  - 没有文件时读剪贴板图片：编码 PNG，保存到 `<storage_dir>/lan_share_outbox/剪贴板图片_YYYYMMDD-HHmmss.png`。
  - 都没有 → `Err("剪贴板里没有可发送的文件或图片")`。
- `lan_share_send_dropped_files(peer_id, files: Vec<DroppedFileInput>) -> Result<String, String>`
  - `DroppedFileInput { name: String, data_base64: String }`（`#[serde(rename_all = "camelCase")]`）。
  - 后端限制：单文件 ≤ 100MB、总数 ≤ 20 个；解码 base64；文件名清洗；保存到 `lan_share_outbox` 后调用 `send_files`。
  - 返回 transfer id（与 `lan_share_send_files` 一致）。

### 5.2 暂存目录

- 目录：`crate::db::storage_dir(app)/lan_share_outbox/`。
- 保留暂存副本（发送失败可重试、可打开）；不在删除附件/删除聊天记录时清理。

### 5.3 输入框行为（`LanChatPane.vue`）

- `paste`：
  - `clipboardData.types` 包含 `Files`，或没有 `text/plain`（纯图片/纯文件）→ `preventDefault()`，调用 `lan_share_import_clipboard_files`；返回路径后通过 `send-files` 动作发送；空剪贴板提示错误文案。
  - 有 `text/plain` 且没有 `Files` → 不拦截，走原生文本粘贴。
- 拖拽：
  - 聊天区根节点监听 `dragenter/dragover/dragleave/drop`；`types` 含 `Files` 或 `files.length > 0`。
  - 拖入时显示高亮遮罩「松开即可发送到当前设备」；离开/放下后消失。
  - `drop` 时跳过目录（`webkitGetAsEntry().isDirectory`），单文件 >100MB 跳过并提示「文件超过 100MB，请用「发送文件」选择」。
  - File → base64（去掉 data URL 前缀）→ `lan_share_send_dropped_files`；成功后刷新由聊天事件驱动。
- 设备离线 / 发送中：拖拽与粘贴发送置灰或直接提示「设备已离线」。
- 输入框、发送按钮的现有行为不变。

## 6. 数据库加密核查

- 现状：`lan_share_messages.payload_enc` 整条加密（含 `file_name`、`file_path`、`status`、`error`），`lan_share_conversations.last_preview_enc` 也加密。
- 传输中的文件、接收方落盘文件、发送方源文件均为明文，符合「正常数据不加密」。
- 本轮补回归测试：文件消息入库后，直接在 DB 里查询 `payload_enc` / 整行原始值，断言不包含文件名和路径明文。

## 7. 托盘未读角标与消息列表

### 7.1 未读

- 未读总数 = `SUM(unread) FROM lan_share_conversations`。
- macOS Dock 角标：主窗口 `set_badge_count(未读数 > 0 ? Some(n) : None)`；更新时机：`emit_chat` 之后、`mark_read` / 删除会话 / 删除附件之后、应用启动初始化一次。
- 左栏每条会话未读红点保持现状。

### 7.2 托盘左键（方案 A）

- 未读总数 > 0：
  - 快捷面板已可见且处于消息模式 → 收起；
  - 否则显示快捷面板并切到「消息」页签。
- 未读总数 = 0：保持原行为，收起面板并显示主界面。
- 右键：显示/收起快捷面板（VPN/休眠），并把面板模式重置为普通模式。

### 7.3 快捷面板「消息」页签

- `QuickPanel.vue` 增加第三个页签「消息」，标题右侧显示未读总数红点（0 时隐藏）。
- 列表项：设备显示名（备注优先）、最后一条消息预览、时间、未读数红点；无会话空态「暂无聊天记录」。
- 点击列表项：调用 `quick_panel_open_chat(peer_id)`；后端收起面板、显示主界面、发 `lan-share://open-chat` 事件。
- 数据：`lan_share_conversations` + `lan_share_snapshot.peer_remarks`；面板挂载、窗口聚焦、收到 `lan-share://chat` 时刷新；切到消息页签时按内容调整面板高度。

### 7.4 定位到会话

- 主窗口 `App.vue` 监听 `lan-share://open-chat`：`view = 'lanshare'`。
- `useLanShareChat` 监听同一事件：刷新快照/会话后 `selectPeer(peerId)`（标已读、拉消息）。
- 若设备不在列表（理论上有会话就不会），提示并只切页签。

### 7.5 快捷面板模式传递

- 后端静态 `QUICK_PANEL_MODE: AtomicBool`（true=消息模式）。
- `quick_panel_take_mode() -> String`（`"messages"` / `"quick"`）：返回当前模式并重置为 `quick`。
- 已创建的面板通过事件 `quick-panel://mode` 通知切换；首次创建时前端挂载后调用 `take_mode` 取初值。

## 8. 接口契约

### 8.1 命令

| 命令 | 参数 | 返回 |
| --- | --- | --- |
| `lan_share_delete_conversation` | `{ peerId: string, deleteFiles: boolean }` | `void` |
| `lan_share_delete_attachments` | `{ peerId: string }` | `number` |
| `lan_share_conversation_attachment_count` | `{ peerId: string }` | `number` |
| `lan_share_import_clipboard_files` | 无 | `string[]`（本地路径） |
| `lan_share_send_dropped_files` | `{ peerId: string, files: { name: string, dataBase64: string }[] }` | `string`（transfer id） |
| `quick_panel_take_mode` | 无 | `'messages' \| 'quick'` |
| `quick_panel_open_chat` | `{ peerId: string }` | `void` |

### 8.2 事件

| 事件 | 载荷 |
| --- | --- |
| `lan-share://chat` | `{ peer_id, kind: 'message'\|'read'\|'deleted', direction: 'in'\|'out'\|null }` |
| `lan-share://open-chat` | `{ peer_id: string }` |
| `quick-panel://mode` | `{ mode: 'messages' \| 'quick' }` |

### 8.3 类型

- `ChatMessage.status` 增加 `'deleted'`。
- `ChatEvent.direction?: 'in' | 'out' | null`。
- `DroppedFileInput`（后端）：`{ name: String, data_base64: String }`，serde camelCase。

## 9. 错误处理

- 剪贴板为空 / 只有文件夹：`message.warning` 提示，不发送。
- 拖拽超过 100MB / 目录：跳过并汇总提示。
- 删除文件失败：记录日志、仍然删除数据库记录；toast 显示实际删除数量。
- 离线 / 未开启：发送动作前检查，提示「设备已离线」/「请先开启局域网分享」。
- Dock 角标设置失败：忽略（部分平台不支持），不影响主流程。

## 10. 测试与验收

- 后端：`cargo test`（新增 chat_store / outbox / 删除相关单测）。
- 前端：`npx vue-tsc --noEmit`、`npx vite build`。
- 独立 verifier 只读复核：删除语义、加密断言、事件 direction、托盘模式、粘贴/拖拽边界。
- 手动验收清单：
  1. 左栏无 hover 删除按钮、长悬浮无黑色提示；右键出现三项菜单。
  2. 删除聊天记录弹窗有勾选（有附件时）；勾选后本机收到的文件消失，对方会话不变。
  3. 「删除附件」后聊天文字保留、文件卡显示「已删除」。
  4. 收到消息有提示音；设置页/右键可开关免打扰；开启后不再响。
  5. 截图后 Cmd+V 直接发出去（文件消息）；Finder 复制文件 Cmd+V 直接发送。
  6. 拖拽文件到聊天区高亮并发送；>100MB 有提示；拖文件夹有提示。
  7. 有未读时点托盘左键弹消息列表；点一条打开主界面并定位到该会话且标已读；无未读时左键打开主界面。
  8. Dock 图标未读角标数字随消息/已读变化。

## 11. 不做的事

- 富文本工具栏、图片内嵌气泡、Markdown 渲染。
- 文件夹发送、断点续传、大文件分片优化（另立项目）。
- 删除同步到对方设备。
- 聊天记录的端到端加密（仅本地数据库加密，已有）。
