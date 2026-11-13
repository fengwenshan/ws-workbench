# 局域网分享（LocalSend 风格）设计

- 日期：2026-11-13
- 分支：`feat/LocalSend`（基于 `feat/master`）
- 参考：[LocalSend](https://github.com/localsend/localsend) 的产品形态（仅参考交互与协议思路，不做官方协议互通）

## 1. 背景与目标

给 ws-workbench 增加一个「局域网分享」工具页：同一局域网内、都在运行本应用的设备能互相发现，选中附近的设备就能把本机文件发过去；接收方在面板里确认后接收，保存到用户指定的下载目录。

目标：

- 局域网内自动发现运行本应用的设备（开启功能时）
- 双方都能发起 / 接收文件，多文件一次发送，显示发送进度
- 接收需要确认（不静默落盘）
- 功能可随时开启 / 关闭；关闭后不再被发现、不再监听
- 接收目录可指定，默认 `~/Downloads/ws-workbench 局域网分享`
- 只做「分享东西」这一个闭环，不做设备配对、加密传输、离线消息、剪贴板同步等

非目标：

- 不与官方 LocalSend App 互通（自定义协议，双方都必须是本应用）
- 不做传输加密 / 密码 / 一次性 PIN（局域网内信任模型，文件名与内容为明文）
- 不做断点续传、多网卡手动选择、IPv6

## 2. 交互设计

入口：左侧图标栏新增「局域网分享」独立工具页（与代理 / 系统 / 设置并列，快捷键 `5`）。

页面结构：

1. **顶栏**：标题 + 总开关（开启 / 关闭）+ 状态（运行中，端口 53317 / 未开启 / 启动失败原因）
2. **本机设置**：设备名（默认主机名，可改）、接收目录（显示实际路径，可「选择目录」/「打开目录」）
3. **附近设备**：发现到的设备列表（设备名、`IP:端口`、最近在线），每项一个「发送文件」按钮；空态说明「同一局域网内打开本应用的设备会出现在这里」
4. **待接收**：别人发来的分享请求卡片（发送方、文件列表、总大小、倒计时），「接受 / 拒绝」；60 秒未处理自动拒绝
5. **发送进度**：每个传输一行，文件名 + 进度条 + 完成 / 失败原因
6. **最近接收**：最近 20 条接收记录，可「打开文件」「打开所在目录」

细节：

- 功能关闭时：停止 UDP 组播与 HTTP 监听，设备列表清空，页面保留设置与最近接收
- 重名文件：接收时自动加 ` (1)`、` (2)` 后缀，不覆盖已有文件
- 应用冷启动时如果功能是开启状态，自动恢复发现与监听；启动失败在页面显示原因
- 主窗口在托盘常驻时接收事件仍会更新状态（面板打开时通过事件同步，关闭时状态保留在内存/持久化最近接收）

## 3. 协议设计

### 3.1 发现

- UDP 组播：`224.0.0.167:53317`（IPv4，TTL=1，开启 loop 便于同机调试）
- 发现端口开 `SO_REUSEADDR` + `SO_REUSEPORT`（socket2，本来就在 tauri 依赖树里），同机开发版 + 安装版可同时监听发现端口（HTTP 端口各自从 53317 起顺延）
- 每 3 秒广播一次 announce；收到别人的 announce 时，若是新设备或距上次广播超过 1 秒，立即回一次（加速发现）
- 15 秒没有收到 announce 视为离线，从列表移除
- 报文（UTF-8 JSON）：

```json
{
  "app": "ws-workbench-lanshare",
  "type": "announce",
  "version": 1,
  "alias": "冯文山的 MacBook",
  "fingerprint": "稳定 UUID，首次开启时生成并持久化",
  "port": 53317,
  "device_type": "desktop"
}
```

- 收到后以「报文源 IP + 报文里的 port」作为对端地址；`fingerprint` 相同的报文视为自己，忽略

### 3.2 传输（HTTP over TCP，明文）

- 服务端监听 `0.0.0.0:<port>`，默认 `53317`，被占用时依次尝试 `53318..53326`，实际端口写进 announce
- 所有响应 `Content-Type: application/json; charset=utf-8`；鉴权只靠一次性随机 token

| 方法/路径 | 说明 |
| --- | --- |
| `GET /api/lanshare/v1/info` | 返回 `{ alias, fingerprint, port, version }`，发送前探活 |
| `POST /api/lanshare/v1/prepare-upload` | 发送方提交文件清单，接收方生成待确认请求；用户接受后返回会话与 token，拒绝/超时返回 403/408 |
| `POST /api/lanshare/v1/upload?session_id=&file_id=&token=` | 请求体为文件原始字节流，服务端流式写盘，成功后返回 `{ ok, path }` |
| `POST /api/lanshare/v1/cancel?session_id=` | 发送方取消本次传输，服务端清理会话 |

`prepare-upload` 请求体：

```json
{
  "sender": { "alias": "对方设备名", "fingerprint": "对方指纹" },
  "files": [
    { "id": "f1", "file_name": "report.pdf", "size": 12345 }
  ]
}
```

接受后的响应：

```json
{
  "session_id": "uuid",
  "files": [{ "id": "f1", "token": "随机串" }]
}
```

约束与错误码：

- 单次最多 100 个文件；单个文件最大 20 GB（防误传），超限直接 400
- 会话 5 分钟未完成自动失活；`upload` 校验 session/file/token，不匹配返回 409
- 文件名只取 basename，过滤 `..`、路径分隔符、控制字符；空名用 `file`
- 落盘先写 `<name>.part`，完成后原子改名；中途断开发送方会收到错误，`.part` 文件由接收端在会话失活时清理

### 3.3 接收确认的线程模型

`prepare-upload` 的 HTTP 线程把请求登记进状态（生成 offer、发出快照事件），然后在 `mpsc::Receiver` 上等待用户决定，最长 60 秒：

- `lan_share_respond_offer(accept = true)`：创建会话、生成 token，把响应发回 HTTP 线程
- `accept = false` 或超时：HTTP 返回 403 / 408，面板里的待接收项消失

## 4. 架构与文件结构

后端（Rust；主体用 `std`，发现端口的复用用 `socket2`（tauri 依赖树里已有），其余复用现有 crate）：

```
src-tauri/src/lanshare/
  mod.rs        # 对外类型、LanShareState、快照组装、启动/停止入口、事件常量
  protocol.rs   # announce / HTTP 报文结构、文件名清洗、文件重名处理、查询串解析
  settings.rs   # 设置持久化（app store: lan_share.json；指纹首次生成）
  discovery.rs  # UDP 组播收发、peer 表维护、peer 快照事件
  server.rs     # TCP HTTP 服务、prepare-upload 等待/upload 流式落盘、会话管理
  client.rs     # 发送端：探活、prepare-upload、逐文件上传、进度事件
src-tauri/src/commands/lanshare.rs  # 6 个 tauri 命令
```

前端：

```
src/api/lanshare.ts                     # 类型 + invoke 封装 + 事件订阅
src/components/lanshare/LanSharePanel.vue
src/App.vue                             # 图标栏入口 + 工具页挂载
src/composables/useWorkbenchLayout.ts   # AppView 增加 'lanshare'，快捷键 5
```

修改：`src-tauri/src/lib.rs`（`mod lanshare;`、命令注册、setup 中按设置恢复）、`src-tauri/Info.plist`（macOS 本地网络用途说明）、`README.md`（功能说明）。

### 4.1 Tauri 命令契约

| 命令 | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `lan_share_snapshot` | — | `LanShareSnapshot` | 页面初始化 / 手动刷新 |
| `lan_share_set_enabled` | `enabled: bool` | `LanShareSnapshot` | 启停；start 失败把原因放 `last_error` |
| `lan_share_set_alias` | `alias: string` | `LanShareSnapshot` | 空串回退主机名；持久化并重新 announce |
| `lan_share_set_download_dir` | `dir: string \| null` | `LanShareSnapshot` | 校验存在/可创建；空回退默认目录 |
| `lan_share_respond_offer` | `offer_id: string, accept: bool` | `void` | 接受 / 拒绝待接收请求 |
| `lan_share_send_files` | `peer_fingerprint: string, paths: string[]` | `string`（transfer_id） | 立即返回，后台线程发送，进度走事件 |
| `lan_share_clear_received` | — | `void` | 清空最近接收列表（只清记录，不删文件） |

事件：

- `lan-share://snapshot`：`LanShareSnapshot`，peer 变化、offer 增删、收到文件、启停、设置变更时推送
- `lan-share://send-progress`：`SendProgress`，每个分块 / 文件完成 / 失败时推送

### 4.2 数据结构（serde 默认 snake_case，前端类型同名对齐）

```rust
struct LanShareSettings { enabled: bool, alias: String, download_dir: Option<String>, fingerprint: String }

struct LanPeer { fingerprint: String, alias: String, ip: String, port: u16, device_type: String, last_seen_ms: i64 }

struct OfferFile { id: String, file_name: String, size: u64 }

struct PendingOffer {
    id: String, sender_alias: String, sender_fingerprint: String, sender_ip: String,
    total_size: u64, files: Vec<OfferFile>, created_at_ms: i64, expires_in_ms: i64,
}

struct ReceivedFile { file_name: String, path: String, size: u64, from_alias: String, received_at_ms: i64 }

struct SendProgress {
    transfer_id: String, peer_alias: String, file_name: String,
    file_index: usize, file_total: usize,
    sent_bytes: u64, total_bytes: u64, done: bool, error: Option<String>,
}

struct LanShareSnapshot {
    enabled: bool, running: bool, alias: String, fingerprint: String,
    port: Option<u16>, download_dir: String, download_dir_custom: bool,
    local_addresses: Vec<String>,
    peers: Vec<LanPeer>, pending_offers: Vec<PendingOffer>,
    recent_received: Vec<ReceivedFile>, last_error: Option<String>,
}
```

### 4.3 兼容性 / 并发约定

- 事件发送统一走 `Option<AppHandle>`；测试里传 `None`，不依赖 Tauri 运行时
- 运行期状态用 `Mutex<Inner>`，锁内不阻塞（HTTP 等待在锁外做）
- 启停用 `Arc<AtomicBool>` + socket 读超时（500ms）实现快速退出；重复启停幂等
- 同一局域网多实例（开发版 + 安装版）允许共存：不同 fingerprint，设备名加 `-dev` 由 `app_display_name()` 给出默认值

## 5. 设置与持久化

- store 文件：`lan_share.json`（tauri-plugin-store 默认目录），键：`settings`
- 明文 JSON（设备名、目录、指纹不属于机密）；不放进加密的 `config.json`
- 首次读取没有设置时生成 `fingerprint`（uuid v4），`enabled=false`，`alias` 空（展示时回退主机名）
- 冷启动：`setup` 里读取设置，`enabled=true` 则后台线程启动；启动失败只记录 `last_error`，不阻断应用启动

## 6. 错误处理与边界

| 场景 | 行为 |
| --- | --- |
| 端口全被占用 | 功能标记启动失败，`last_error` 显示，开关回到关闭 |
| 下载目录不可写 | 设置时报错不保存；启动时回退默认目录并记录错误 |
| 接收目录删掉 | 接受文件时重新创建；失败则该次传输失败并提示 |
| 发送时对方离线 / 拒绝 | 进度事件 `done=true, error=...`，面板展示失败原因 |
| 用户关闭功能时有进行中的传输 | 直接中断，发送方收到失败；待接收清理 |
| 磁盘满 / 写失败 | 该文件失败，其余文件继续，发送方看到错误 |
| Windows 防火墙 | 首次监听触发系统放行提示（应用级）；不做静默规则 |

## 7. 平台适配

- macOS：新增 `src-tauri/Info.plist`，写入 `NSLocalNetworkUsageDescription`（Tauri 构建时自动合并）
- Windows：NSIS 安装包首次运行会弹防火墙允许提示；文档中说明
- Linux：不在当前发布范围，代码按 `cfg` 不特殊处理，能编译即可

## 8. 安全考量

- 仅在局域网：组播 TTL=1，HTTP 绑定 `0.0.0.0` 但只接受同一子网可达的请求（无公网穿透）
- 上传必须持有 prepare-upload 下发的随机 token，且文件大小与登记值一致
- 文件名清洗 + 只写目标目录，杜绝路径穿越
- 不主动对外开放除 53317 系列端口外的任何服务；功能关闭即释放端口

## 9. 测试与验收

后端：

- 单测：announce / prepare / info 报文 JSON 解析、文件名清洗与去重、query 解析
- 单测：`prepare-upload` 拒绝 / 超时 / 接受三条路径（用测试内启动的服务端 + 手动响应）
- 端到端（单进程双运行时）：运行时 A 启动 HTTP 服务，运行时 B 通过 client 发送两个文件，断言文件落盘、内容一致、`recent_received` 有记录、进度事件完整
- 端口占用回退、目录不存在自动创建各有单测

前端：

- `pnpm build`（`vue-tsc --noEmit` + vite build）通过
- 面板挂在 v-show 常驻页面，`active` 切回时刷新快照；事件监听在卸载时清理

人工验收（本机单实例 + 模拟对端脚本）：

- 开启功能后，模拟对端（Node 脚本发 announce + 调 prepare-upload / upload）能在面板出现设备与待接收卡片；接受后文件落到指定目录
- 面板点「发送文件」能向模拟对端服务发出 prepare-upload 与 upload 请求并显示进度
