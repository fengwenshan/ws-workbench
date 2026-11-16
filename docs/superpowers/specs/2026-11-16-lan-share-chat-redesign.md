# 局域网分享：机器列表 + 聊天式改造设计

- 日期：2026-11-16
- 基线：`beta` 分支（`docs/superpowers/specs/2026-11-13-lan-share-design.md` 的后续改造）
- 目标：把「附近设备卡片 + 待接收 + 发送进度」的工具页，改成「左侧机器列表 + 右侧聊天/文件」的会话式页面；机器身份改为物理地址（MAC）；修复两台在线设备仍显示灰点的缺陷。

## 1. 背景与目标

现状问题：

- 页面是工具面板：附近设备一屏卡片，聊过谁、传过什么没有沉淀；关掉面板就全忘了。
- 设备身份是随机 UUID 指纹，重装 / 清配置后会变。
- 两台机器都在线，附近设备列表里的在线点却一直是灰的（见 §6 根因分析）。

目标：

1. 左侧机器列表：**当前在线的设备（含从没聊过的）+ 聊过天/传过文件的会话**；在线在前、离线历史会话在后，组内按最近聊天时间倒序。
2. 右侧聊天：文本消息 + 文件消息（带进度/完成/失败）+ 收到的文件请求（接受/拒绝）。
3. 机器身份 = 主物理网卡 MAC（`hardware_id`），跨重启/换 IP 不变；取不到 MAC 时回退原持久化 UUID。
4. 会话与消息持久化到本地 SQLite（schema v4），消息正文沿用 vault 加密；删除会话 = 删本机记录。
5. 图一的本机信息卡（设备名 / 开关 / 端口 / 地址 / 设备 ID / 接收目录）挪到左栏底部「设置」页；图二的「附近设备」变成列表中的在线设备，「发送进度」变成聊天里的文件消息。
6. 修复在线点灰点：发现线程续上 `last_seen` 时也要推快照（限速 ≤1 次/秒）。

非目标：

- 不做离线消息排队 / 补发（对方不在线时发送明确失败，可重试）。
- 不做消息撤回、表情、图片内联、已读回执给对方。
- 不改文件传输协议（沿用 prepare-upload / upload / token 流程）。
- 不做多设备账号体系、不做设备改名同步（改名随 announce 更新）。

## 2. 交互与布局

```
┌──────────────────────── 局域网分享 ────────────────────────┐
│ ┌──────────────┬─────────────────────────────────────────┐ │
│ │ 局域网分享    │  <设备名>  ● 在线                        │ │
│ │              │  <IP>:<端口>        [发送文件] [删除会话] │ │
│ │ ● 在线设备 A  │ ┌─────────────────────────────────────┐ │ │
│ │   在线 · 摘要 │ │ 消息气泡（对方左 / 自己右）           │ │ │
│ │ ● 在线设备 B  │ │ 文件消息（进度 / 完成 / 失败 / 重试） │ │ │
│ │ ○ 历史设备 C  │ │ 待接收文件请求（接受 / 拒绝 / 倒计时）│ │ │
│ │ ○ 历史设备 D  │ └─────────────────────────────────────┘ │ │
│ │              │ ┌─────────────────────────────────────┐ │ │
│ │              │ │ 输入框（Enter 发送 / Shift+Enter 换行）│ │ │
│ │              │ │                            [📎] [发送] │ │ │
│ │ ──────────── │ └─────────────────────────────────────┘ │ │
│ │ ⚙ 设置 ●运行中│                                         │ │
│ └──────────────┴─────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

左栏（固定约 280px，窄窗口可折叠）：

- 列表项数据 = 会话表 ∪ 当前在线 peer。
  - 有会话的：展示会话别名、最后一条消息摘要、最后活动时间、未读数。
  - 仅在线无会话的：展示 peer 别名、`在线 · 发消息或文件`、在线点；`last_activity_ms` 用 `last_seen_ms` 参与排序。
- 排序：`online` 为 true 的排前面；组内按 `last_activity_ms` 降序；离线组同理。
- 在线点：绿 = 后端 `online`（最近 10 秒内收到 announce / 心跳）；灰 = 离线历史会话。
- 离线且没有会话的 peer 不显示（`prune_peers` 15 秒后也会移除）。
- 列表项 hover 出现「删除会话」入口（只对有历史会话的显示）。
- 底部「⚙ 设置」：点击右栏切换为设置页，按钮旁显示运行状态点（运行中绿 / 未开启灰 / 启动失败红）。

右栏：

- 空态：没有选中设备时提示「从左侧选择一台设备开始聊天」。
- 头部：设备名、在线状态、`IP:端口`、发送文件、删除会话（无会话时隐藏）。
- 消息流：按时间正序；文本气泡；文件消息卡片（文件名、大小、状态、发送中进度条、失败原因 + 重试）；收到的文件请求卡片（文件列表、倒计时、接受/拒绝）；日期/时间分隔。
- 输入区：Enter 发送，Shift+Enter 换行；发送中禁用；对方离线时禁用并显示「设备已离线」；`📎` 选择文件发送。
- 设置页（右栏内）：本机设备名（铅笔就地编辑）+ 运行状态 + 开关；监听端口；本机地址；设备 ID（MAC，长按/悬浮看完整）；接收目录（选择 / 打开 / 恢复默认）；错误/警告提示。

文件消息归属：

- 发送文件：每个文件生成一条 `direction=out, kind=file` 消息，`status=sending → sent | failed`，`transfer_id` / `file_index` / `file_total` 与进度事件对应，前端按 transfer_id + file_index 把进度合进对应气泡。
- 接收文件：`prepare-upload` 到达时先 upsert 会话（让设备出现在列表），待接收请求仍走 snapshot（聊天里展示请求卡片）；用户接受并落盘成功后，每个文件生成 `direction=in, kind=file, status=received` 消息。
- 拒绝 / 超时：不生成消息，但会话保留（交互已发生）。

## 3. 身份：物理地址（MAC）

新增 `lanshare/identity.rs`：

- `primary_mac() -> Option<String>`：从 `crate::netinfo::NetworkInfo::current()` 取网卡，过滤虚拟网卡与全 0 MAC，规范成大写冒号格式（`A4:83:E7:12:34:56`）。
  - 过滤规则（名字小写匹配前缀/包含）：`utun`、`tun`、`tap`、`awdl`、`llw`、`bridge`、`vmnet`、`veth`、`docker`、`br-`、`tailscale`、`zerotier`、`wsl`、`vethernet`、`hyper-v`、`virtualbox`、`loopback`、`lo0`；MAC 为空或全 0 也过滤。
  - 优先级：带 IPv4 的物理网卡 > 任意物理网卡；同优先级按接口名排序（保证稳定）。macOS 的 `en0`、Windows 的「以太网 / WLAN」、Linux 的 `eth*/wlan*` 自然排前。
  - 纯函数 `pick_primary_mac(interfaces: &[NetworkInterface]) -> Option<String>` 便于测试。
- 身份取值：启动时（`LanShareState::from_app`）如果 `primary_mac()` 有值且与 `settings.fingerprint` 不同，则把 `settings.fingerprint` 覆盖为 MAC 并回写 `lan_share.json`；取不到 MAC 则沿用持久化值（旧 UUID 或上次的 MAC）。
- 协议字段沿用 `fingerprint`（值就是 MAC 字符串），不新增 wire 字段：旧版本设备仍能互相发现（只是它们的身份还是旧 UUID，聊天功能对旧版本不可用，发送会失败提示）。
- 同一台机器开发版 + 安装版 MAC 相同 → 会被当成同一台设备、互相忽略（用户已确认接受）。测试里 `LanShareState::new` 仍用随机指纹，不受影响。
- UI 文案把「设备指纹」改为「设备 ID（MAC）」；回退 UUID 时显示短指纹。

## 4. 聊天协议与后端

### 4.1 HTTP 接口（在现有 4 个接口上新增）

| 方法/路径 | 说明 |
| --- | --- |
| `POST /api/lanshare/v1/chat` | 接收一条文本消息；校验后落库、更新会话、推事件，返回 `{"ok":true}` |

请求体（`protocol::ChatRequest`）：

```json
{
  "sender": { "alias": "冯文山的 MacBook", "fingerprint": "A4:83:E7:12:34:56" },
  "message_id": "uuid",
  "text": "你好",
  "sent_at_ms": 1763270000000
}
```

约束：正文 `Content-Length ≤ 16KB`；`text.trim()` 非空且字符数 ≤ 4000；`sender.fingerprint` 非空；`message_id` 为空时服务端补 UUID；`sent_at_ms` 缺失用服务端时间。

### 4.2 事件

- `lan-share://snapshot`：不变（peer / pending offers / 运行状态 / 设置），但 `last_seen_ms` 会随心跳持续刷新（§6）。
- `lan-share://chat`：`ChatEvent { peer_id: String, kind: String }`，`kind ∈ "message" | "read" | "deleted"`；前端收到后重新拉会话列表，若正在看该会话再拉消息。文件进度仍走 `lan-share://send-progress`。

### 4.3 Tauri 命令

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `lan_share_conversations` | — | `LanShareConversation[]` | 持久化会话 + `online`/最新别名（从运行期 peer 合并） |
| `lan_share_messages` | `peer_id: string` | `ChatMessage[]` | 最近 200 条，时间正序 |
| `lan_share_send_chat` | `peer_id: string, text: string` | `ChatMessage` | 本地先落库（sending），后台线程投递，事件更新 |
| `lan_share_mark_read` | `peer_id: string` | `void` | 清零未读 |
| `lan_share_delete_conversation` | `peer_id: string` | `void` | 删本机会话 + 消息，推 `deleted` 事件 |

`send_files` 参数不变（`peer_fingerprint` 即 peer 的 MAC 身份），内部额外写入文件消息；`snapshot` / `set_enabled` / `set_alias` / `set_download_dir` / `respond_offer` 不变。

### 4.4 数据结构

```rust
struct ChatMessage {
    id: String, peer_id: String,
    direction: String,   // "in" | "out"
    kind: String,        // "text" | "file"
    text: String,
    file_name: Option<String>, file_size: Option<u64>, file_path: Option<String>,
    status: String,      // "sending" | "sent" | "failed" | "received"
    error: Option<String>,
    transfer_id: Option<String>, file_index: Option<usize>, file_total: Option<usize>,
    created_at_ms: i64,
}
struct LanShareConversation {
    id: String, alias: String, online: bool,
    last_activity_ms: i64, last_preview: String, unread: i64, created_at_ms: i64,
}
struct ChatEvent { peer_id: String, kind: String }
```

### 4.5 发送流程

1. `lan_share_send_chat`：校验 → 运行期 peer 必须存在（否则「设备已离线」）→ 生成消息 id → `ChatStore::append_message(direction=out, status=sending)` → 推 chat 事件 → 返回消息。
2. 后台线程 `client::run_send_chat`：按候选地址 POST `/api/lanshare/v1/chat`（连接 / 读写超时 5 秒）→ 成功把状态更新为 `sent`，失败更新为 `failed` 并写 error → 再推 chat 事件。
3. 重试：`lan_share_send_chat` 不提供重试命令；失败气泡点击重试时前端用同一文本重新调用（新消息 id，简单可靠）。

文件发送流程：`send_files` 先 `client::collect_files` 校验，再为每个文件写 `kind=file, status=sending` 消息（带 transfer_id/file_index），然后复用原后台发送线程；进度事件只用于更新气泡进度；结束时把每条消息状态更新为 sent/failed。

### 4.6 存储（SQLite schema v4）

`src-tauri/src/db.rs`：`SCHEMA_VERSION = 4`，新增 `SCHEMA_SQL_V4`：

```sql
CREATE TABLE IF NOT EXISTS lan_share_conversations (
  id              TEXT PRIMARY KEY NOT NULL,
  alias           TEXT NOT NULL,
  last_activity_ms INTEGER NOT NULL,
  last_preview_enc TEXT NOT NULL,
  unread          INTEGER NOT NULL DEFAULT 0,
  created_at_ms   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lan_share_conversations_activity
  ON lan_share_conversations(last_activity_ms DESC);

CREATE TABLE IF NOT EXISTS lan_share_messages (
  id            TEXT PRIMARY KEY NOT NULL,
  peer_id       TEXT NOT NULL,
  direction     TEXT NOT NULL,
  kind          TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  payload_enc   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lan_share_messages_peer_created
  ON lan_share_messages(peer_id, created_at_ms);
```

- 消息 payload（`serde_json`）用 `crate::crypto::encrypt_with_keys` 加密：`{ text, file_name, file_size, file_path, status, error, transfer_id, file_index, file_total }`。
- 会话预览 `last_preview_enc` 同样加密；列表接口解密后再返回。解密失败（换机器 / 密钥丢失）时该条消息跳过或显示占位，不让整页报错。
- `ChatStore` 每次操作 `crate::db::open(path)` 打开连接（WAL + busy_timeout 已有），不在 `Inner` 锁内做 DB IO。

`ChatStore` 接口（`src-tauri/src/lanshare/chat_store.rs`）：

```rust
impl ChatStore {
    pub fn open(app: &AppHandle) -> Result<Self, String>; // db_path + vault_keys，并跑一次 open 确保 v4 迁移
    pub fn new(path: PathBuf, keys: VaultKeys) -> Self;   // 测试用
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

`append_message` 行为：事务内 upsert 会话（保留已有 `created_at_ms`；`alias` 非空才覆盖；`last_activity_ms = max(旧值, 消息时间)`；预览 = 文本摘要或 `[文件] 文件名`；`direction == "in"` 时 `unread += 1`）→ 插入消息 → 提交。

## 5. 文件传输与待接收的接线

- `server::handle_prepare`：校验通过、登记 pending offer 后，`ChatStore::upsert_conversation(sender_fingerprint, sender_alias, now)` 并推 chat 事件，使会话立刻出现在左侧。
- `server::handle_upload`：文件落盘成功后 `append_message(dir=in, kind=file, status=received, file_path=落盘路径)` 并推 chat 事件。
- 拒绝 / 超时：仅移除 pending，不写消息。
- `handle_chat`：写消息并推 chat 事件。
- 所有 chat store 调用都在 `lock_inner` 之外；store 从 `Inner.chat.clone()` 取出。

## 6. 在线点灰点修复

根因：`discovery::run` 只在 `Inner::upsert_peer` 返回 `changed == true` 时把 `dirty` 置真；而 `upsert_peer` 的 `changed` 只比较 alias/ip/port/device_type/addresses，**`last_seen_ms` 更新不算变化**。于是快照里的 `last_seen_ms` 停留在「首次发现」时刻，前端 `isPeerOnline`（10 秒窗口）在 10 秒后把两台在线设备判成离线 → 灰点；15 秒内又没到 `prune_peers` 的移除阈值，所以设备还在列表里。

修复：

- `upsert_peer` 返回 `PeerUpdate { changed: bool, seen: bool }`；`seen = true` 表示 `last_seen` 确有推进。
- 发现线程：`dirty |= update.changed || update.seen`；只有 `changed` 时才做地址探测 / 立即回 announce（保持原限速行为）。
- `emit_snapshot` 限速仍为 ≤1 次/秒，心跳 3 秒一次，正常情况列表在线点持续为绿。
- 回归测试：连续两次 `upsert_peer`（第二次只推进时间）断言 `changed == false && seen == true`；`snapshot_of` 在时钟推进后 `last_seen_ms` 跟着变。

## 7. 文件结构

后端（新增/修改）：

```
src-tauri/src/lanshare/identity.rs    # 新增：MAC 识别
src-tauri/src/lanshare/chat_store.rs  # 新增：聊天存储
src-tauri/src/lanshare/protocol.rs    # ChatRequest / ChatResponse
src-tauri/src/lanshare/server.rs      # /chat + 文件消息接线
src-tauri/src/lanshare/client.rs      # run_send_chat / collect_files 可见性 / 文件消息
src-tauri/src/lanshare/discovery.rs   # last_seen 续推快照
src-tauri/src/lanshare/mod.rs         # 身份初始化、Inner.chat、chat 相关方法、事件、命令支撑
src-tauri/src/commands/lanshare.rs    # 5 个新命令
src-tauri/src/db.rs                   # schema v4
src-tauri/src/lib.rs                  # 命令注册
```

前端（新增/修改）：

```
src/api/lanshare.ts                        # 类型 + 命令 + 事件
src/components/lanshare/LanSharePanel.vue  # 两栏壳 + 状态/事件编排
src/components/lanshare/LanShareSidebar.vue# 左栏列表 + 底部设置入口
src/components/lanshare/LanChatPane.vue    # 右栏聊天/消息/输入
src/components/lanshare/LanShareSettings.vue# 本机信息卡（原顶部卡片）
```

## 8. 错误处理

- 聊天存储不可用（钥匙串 / DB 打不开）：功能其余部分照常，`snapshot.warning` 提示「聊天记录不可用」；聊天命令返回错误。
- 消息过大 / 空消息：发送端直接提示；接收端 400。
- 对端离线 / 连接失败：消息状态 failed + 气泡显示原因；不自动重发。
- 解密失败：跳过坏消息并在控制台留痕，页面不崩。
- 删除会话：本地事务删除；之后对方再来消息重新建会话。

## 9. 测试计划

Rust（`cd src-tauri && cargo test`）：

- `identity`：虚拟网卡过滤、带 IPv4 优先、MAC 规范化、空网卡回退 None。
- `mod`：`upsert_peer` 第二次只推进时间 → `changed=false, seen=true`；`snapshot_of` 的 `last_seen_ms` 会更新；MAC 身份写入 settings（纯函数测试）。
- `chat_store`：临时 DB（测试密钥）追加/读取/未读/标记已读/更新状态/删除会话/预览加密可解。
- `db`：v1/v2/v3 → v4 迁移后 `user_version = 4` 且新表存在。
- `server`：`/chat` 正常写入 + 校验失败 400；`/prepare-upload` 到达时创建会话；`/upload` 完成后生成 received 文件消息。
- `client`/`mod`：两实例端到端聊天（A 起 HTTP，B 的 `run_send_chat` 投递成功）；文件发送生成 outgoing 文件消息并在完成后更新状态。
- 现有 lanshare 测试全部保持通过。

前端（`npx vue-tsc --noEmit` + `npx vite build`）：类型检查、构建通过；人工检查两栏布局、排序（在线优先 / 组内最近聊天）、在线点、发送/接收消息、文件进度、待接收、删除会话、设置页信息与开关。

## 10. 兼容性 / 迁移

- 旧 `lan_share.json` 无需迁移：`fingerprint` 启动时被 MAC 覆盖；旧 UUID 仍作为无 MAC 环境回退。
- 旧版对端：仍可被发现、互传文件；聊天接口不存在时发送失败并在气泡提示。
- 数据库 v4 迁移只新增表，不改旧表；升级失败（版本过新）沿用现有保护。
