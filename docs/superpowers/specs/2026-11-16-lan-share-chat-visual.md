# 局域网分享聊天区视觉升级（钉钉式）设计

- 日期：2026-11-16
- 分支：beta
- 前置：`2026-11-16-lan-share-chat-redesign.md`（两栏聊天、MAC 身份、聊天库）
- 状态：已确认，按本文实现

## 1. 背景与目标

上一轮已把「局域网分享」改成左侧设备列表 + 右侧聊天。用户提供钉钉聊天截图作为视觉参考，并明确：

1. 没有服务器，**没有「查收」概念**：文件直接发过去落盘，不需要「未查收/下载」这类云端语义。
2. **不需要对方同意，默认直接接收**；如果用户想要确认，在左栏底部「设置」里加一个勾选，默认不勾。
3. 聊天区按钉钉观感改造：**头像 + 昵称 + 时间 + 气泡/文件卡**。
4. 头像用**系统图标**：对方电脑图标，自己人形图标。
5. 名称用电脑名称；**允许给对方的电脑起备注，备注只在本机显示**。

目标：视觉与交互对齐参考图，同时保持局域网直连、无服务器、无查收的语义。

## 2. 消息区视觉规范

### 2.1 消息行

- 左右分栏：对方在左（头像左、内容右），自己在右（内容左、头像右）。
- 头像 32px 圆形，内联 SVG：
  - 对方：电脑 / 显示器图标；
  - 自己：人形图标。
  - 底色统一淡色块，图标用主文字色；不引入图片资源。
- 昵称行在气泡/文件卡上方：
  - 对方：`备注 || 设备名`；自己：`我`；
  - 昵称右侧跟小号时间 `HH:mm`（本地时区）。
- 文本气泡：
  - 对方：白底、1px 边框、12px 圆角（靠头像一侧上角略小）；
  - 自己：淡蓝底（主色 8% 左右）、同尺寸圆角；
  - 文本保留换行、`word-break`。
- 日期分隔沿用现状（今天 / 昨天 / 日期），居中灰底小胶囊。
- 文本发送失败：气泡下方红字原因 + 「重试」保留。

### 2.2 文件消息卡

- 卡片宽约 320px（小窗自适应到容器宽），白底、12px 圆角、细边框。
- 头部：大号文件图标（蓝色文档 SVG）+ 文件名（粗体、单行省略）+ 大小；多文件传输追加「第 n/N 个」。
- 底部分隔线 + 一行：左侧状态文字，右侧胶囊按钮。
- 状态与动作矩阵：

| 场景 | 左侧状态 | 右侧动作 |
| --- | --- | --- |
| 发送中（out） | `发送中 42%` + 细进度条 | — |
| 发送失败 | `发送失败`（错误原因截断在下一行/标题） | `重试` |
| 发送成功（out） | `已发送` | `打开` `打开文件夹` |
| 接收已保存（in） | `已保存` | `打开` `打开文件夹` |
| 接收方开启确认、尚未接受 | 由待接收卡展示（见 §3.2） | — |

- **没有**「未查收 / 1人查收 / 下载」：文件接受（或自动接收）后即落盘。
- 「打开」调用现有 `open_file`；「打开文件夹」新增命令 `lan_share_open_file_dir(path)`（打开文件所在目录，目录不存在时退到最近存在的上级）。

### 2.3 布局细节

- 消息流容器、左栏 280px、窄窗收窄规则、设置页在右栏，均沿用上一轮实现。
- 设置页仍保留：设备名、状态开关、端口、地址、设备 ID（MAC）、接收目录、错误/警告；新增 §3.3 的确认勾选。

## 3. 默认直接接收（可开关确认）

### 3.1 设置字段

`LanShareSettings` 新增：

```rust
/// 接收文件前是否需要本机确认；默认 false（直接接收）
#[serde(default)]
pub require_accept: bool,
```

- 默认 `false`，旧 `lan_share.json` 缺字段时按 `false` 解析。
- 设置页新增勾选/开关：**「接收文件前需要确认」**，默认不勾选；切换后立即持久化并刷新快照。

### 3.2 接收端行为

`handle_prepare` 校验通过后：

- `require_accept == false`（默认）：
  1. 不创建 pending 通道、不走 `recv_timeout`；
  2. 直接创建接收目录 / session / 每个文件的一次性 token；
  3. 立即返回 `PrepareUploadResponse`，发送方开始上传；
  4. 不产生「待接收」请求、不显示左栏「待接收 N」；
  5. 文件上传成功后照常写入 `received` 文件消息（未读 +1）。
- `require_accept == true`：保留现有流程（pending offer → 左栏待接收提醒 / 聊天内接受拒绝卡 → 接受后建 session 返回 token；拒绝 403、超时 408）。

两条路径共用同一段「建目录 + session + token」的代码，避免复制。安全说明：默认免确认意味着同一局域网内运行本应用的设备可以直接写入接收目录；需要逐次确认时勾选设置即可。

### 3.3 发送端行为

不变：`prepare-upload` 返回 200 即开始上传；失败/拒绝/超时都会把对应文件消息回写 `failed` 并给出原因。

## 4. 本机备注（只本机显示）

### 4.1 存储与字段

`LanShareSettings` 新增：

```rust
/// 本机给其它设备起的备注：fingerprint -> 备注（不广播给对端）
#[serde(default)]
pub peer_remarks: std::collections::HashMap<String, String>,
```

- 随 `lan_share.json` 持久化；key 是设备指纹（MAC，取不到时为回退 UUID）。
- 备注不写入 announce / `InfoResponse` / `/chat`，只影响本机显示。
- 写入前 `trim`，按字符截断到 32；空串表示删除该备注。

### 4.2 快照与显示

`LanShareSnapshot` 新增：

```rust
pub require_accept: bool,
pub peer_remarks: HashMap<String, String>,
```

- 前端左栏列表项显示名 = `peer_remarks[id] || 最新 announce 别名 || 会话别名 || "未知设备"`。
- 聊天头部同样按此优先级显示；对方离线时也能显示备注。
- 设备改名后备注保留（key 不变）。

### 4.3 编辑入口

- 聊天头部名字右侧铅笔按钮 → 内联输入框（回车保存、Esc 取消、清空保存即删除）。
- 命令：`lan_share_set_peer_remark(peer_id: String, remark: String) -> LanShareSnapshot`
  - 保存成功后返回最新快照，前端 `applySnapshot` 即可刷新左栏与头部；不额外广播。
- 命令：`lan_share_set_require_accept(require_accept: bool) -> LanShareSnapshot`
  - 写入设置、回存；运行期 `Inner.settings` 同步，后续 `prepare-upload` 立即生效。

## 5. 命令与协议变更清单

新增命令（`src-tauri/src/commands/lanshare.rs`，注册到 `lib.rs`）：

| 命令 | 参数 | 返回 |
| --- | --- | --- |
| `lan_share_set_peer_remark` | `peerId`, `remark` | `LanShareSnapshot` |
| `lan_share_set_require_accept` | `requireAccept` | `LanShareSnapshot` |
| `lan_share_open_file_dir` | `path` | `()` |

- `LanShareState::set_peer_remark(app, peer_id, remark) -> Result<LanShareSnapshot, String>`
- `LanShareState::set_require_accept(app, value) -> Result<LanShareSnapshot, String>`
- 打开文件夹：取 `Path::parent()` 后复用 `crate::commands::system::open_dir`（目录不存在会自动退到上级）。

事件：不新增事件；备注/开关保存后由命令返回值刷新，文件接收流程沿用 `lan-share://chat` / `lan-share://snapshot`。

## 6. 文件结构

后端（Lead 负责）：

- `src-tauri/src/lanshare/mod.rs`：设置字段、快照字段、两个状态方法、`Snapshot` 组装。
- `src-tauri/src/lanshare/server.rs`：`handle_prepare` 自动接收分支 + 抽取共用建 session 函数。
- `src-tauri/src/commands/lanshare.rs`：3 个新命令。
- `src-tauri/src/lib.rs`：注册命令。
- `src-tauri/src/lanshare/settings.rs`：无逻辑变化（serde 自动兼容缺省字段）。

前端（frontend-chat-ui 负责）：

- `src/api/lanshare.ts`：快照字段、3 个命令封装。
- `src/components/lanshare/LanChatMessageList.vue`：消息行头像/昵称/时间布局。
- `src/components/lanshare/LanFileMessage.vue`：文件卡重做（状态 + 打开/打开文件夹/重试 + 进度）。
- `src/components/lanshare/LanChatPane.vue`：头部头像 + 名称 + 备注编辑。
- `src/components/lanshare/LanShareSettings.vue`：确认勾选。
- `src/components/lanshare/useLanShareChat.ts` / `useLanShareActions.ts` / `types.ts`：备注覆盖显示名、保存备注、开关动作。
- 新增 `src/components/lanshare/LanAvatar.vue`（纯展示，系统图标）。

## 7. 错误与边界

- 备注命令：peer_id 空 / 备注超长截断；保存设置失败返回错误、内存不更新。
- 打开文件夹：`file_path` 为空或目录不存在 → 返回明确错误，前端 `message.error`。
- 自动接收：建目录失败 / 超过 20GB / 分享已关闭 → 返回 4xx/5xx，发送方标记失败，不产生半截会话。
- 接收方忙（多文件并发）沿用现有 session 上限与清理逻辑。
- 聊天库不可用：与上一轮一致，降级为 snapshot warning，不影响文件接收。

## 8. 测试

后端：

- `require_accept` 缺省为 false（旧 settings JSON 反序列化）。
- `require_accept == false` 时 `handle_prepare` 直接 200 + token、无 pending；上传成功后落盘 + `received` 消息。
- `require_accept == true` 时保留现有 pending 流程测试（403 / 408 / 接受）。
- 备注设置/清除：快照 `peer_remarks` 正确、trim/截断、非法 peer_id。
- 打开文件夹命令：构造临时文件，返回 `Ok`；不存在的路径返回 `Err`（不真正拉起 GUI 时可用 `nearest_existing_dir` 覆盖）。
- 现有 e2e 文件测试改为开启确认或走自动接收路径，保持全绿。

前端：

- `npx vue-tsc --noEmit`、`npx vite build` 通过。
- 自查：头像/昵称/时间、文件卡四种状态、备注编辑即时生效、设置勾选默认关闭、自动接收时不再出现待接收卡。

## 9. 兼容性

- 旧 `lan_share.json`：缺 `require_accept` / `peer_remarks` 时按 `false` / 空表解析，无需迁移。
- 数据库 schema 不变（v4），旧聊天记录照常显示；「已接收」文案改为「已保存」仅前端展示变化。
- 协议不变（`prepare-upload` 请求/响应字段不变），只是接收端可能不再等待确认；新旧版本互连时行为由接收端的设置决定。
