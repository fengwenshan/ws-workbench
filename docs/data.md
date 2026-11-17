# 配置与数据

> 来源：原 [README](../README.md) 的「配置与数据」章节；加密规则见 [AGENTS.md](../AGENTS.md) 第 5 节。

业务数据（分组、项目、笔记、附件元信息、VPN 配置、规则集）存在应用私有目录的 `ws-workbench.db`（SQLite）中：表里只留 `id`、`group_id`、`sort_order` 等少量明文列，每行内容序列化成 JSON 后用 SM4 信封加密，存进 `payload` 列。一级项目的 `group_id` 记为空串（列本身 NOT NULL，不需要改表结构）；分组与一级项目的混排顺序存在 `config.json` 的 `tree_order`（一级 id 序列）里。`AppConfig` 与前端命令保持不变，加载时从多表组装，保存时与内存快照做 diff、事务内只写变化的行。

应用设置（全局默认打包规则、上次选中的 VPN 配置、默认上传目录、worker 会话目录）仍存在 `config.json`（`tauri-plugin-store`，键名 `app_config`），**整份加密**：SM4-CBC + 随机 IV + HMAC-SM3，信封形如 `v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`。主密钥（32 字节）首次启动时随机生成，macOS 存系统钥匙串（`ws-workbench-vault` / `ws-workbench`），Windows 用 DPAPI 加密后写进 `<app_data_dir>/.vault_key`。

前端写 localStorage 也一律走 `safeStorage`：值经 `vault_encrypt` 变成密文再落盘。

数据库与设置文件统一放在「标准数据仓库」：优先 `app_data_dir`（macOS `~/Library/Application Support/com.ws.workbench`，Windows `%APPDATA%\com.ws.workbench`），不可写时按 `app_config_dir` → 主目录点目录 → `cwd/.config`（开发兜底）逐级回退，全部失败则退回进程内存，此时重启会丢数据。

旧版本的数据目录（产品名目录 `ws-workbench`、`~/.ws-workbench`、`%LOCALAPPDATA%\ws-workbench`）由 `storage` 模块在启动时自动识别并迁移：只搬 `config.json`、`ws-workbench.db`、`attachments/`、`clipboard-attachments/`、`vpn/` 这些业务数据，目标已有同名数据时不覆盖，搬空后删除旧目录；平台差异只在旧目录候选（`storage/macos.rs` / `storage/windows.rs`），搬运逻辑共享。

附件元信息随所属分组/项目一起加密保存；密文 blob 就在上传目录里，文件名为 `<uuid>.wst`。备份时复制 `ws-workbench.db`、`config.json` 与上传目录即可。

Hosts 在线编辑直接改系统 hosts（`/etc/hosts` / `C:\Windows\System32\drivers\etc\hosts`），不再单独存环境配置；每次保存前把原内容备份到 `<data_dir>/hosts-backups/`（保留 30 份）。剪贴板历史的文本 / HTML 与其它配置一起加密存在 `ws-workbench.db` 里，图片附件在 `<data_dir>/clipboard-attachments/`（WST1 密文），备份时一起复制即可。

VPN 会话目录同时被主进程和提权 worker 使用，因此不做路径拼接了事，而是带写入探测再逐级回退（受控文件夹访问、企业策略、受限令牌都可能让 `app_data_dir` 静默拒绝写入）。目录内文件：

| 文件 | 用途 |
| --- | --- |
| `request.json` | 主进程下发的连接请求 |
| `status.json` | worker 上报的实时状态（阶段、隧道参数、生效模式与路由） |
| `worker.log` / `script.log` | worker 与 vpnc-script 日志 |
| `stop.flag` | 请求 worker 停止 |
| `mode` / `rules.json` | 运行期代理模式与规则集（主进程写、worker 轮询后重新布线） |
| `main.pid` | 主进程 PID，worker 每个循环检查一次，界面进程被强杀时秒级收尾 |
| `main.alive` | 父进程心跳，`main.pid` 拿不到时的兜底（超过 10 秒没刷新就判主进程已死） |
| `owner.json` | 这条连接是哪个实例开的（PID + 实例名）；只有它能断开 / 改规则 |
| `script/` | 生成的 vpnc-script |

超时与轮询节奏：连接超时 120 秒，守护周期 15 秒，状态轮询 600 毫秒。worker 退出码 `0` 正常、`2` 无请求、`3` 失败。

**同机开两个实例（开发版 + 安装包、安装包 + 便携包）**：两者的 `app_data_dir` 是同一个（identifier 都是 `com.ws.workbench`），会话目录**共享**——这样两个窗口看到的是同一条连接的真实状态，不会出现「一边显示未连接、实际隧道是另一边连着的」然后又被诱导连第二次。能不能操作按 `owner.json` 判定：

- 连接是哪个实例发起的，就只有它能断开、切换配置、改代理模式和规则；另一个实例的状态照常显示，但连接/断开/切换按钮全部置灰，顶部标明「VPN 正由「ws-workbench-dev」连接，这个窗口只能看状态」。绕过界面直接调命令也会被后端拒绝（`vpn_connect` / `vpn_disconnect` / `vpn_switch_profile` / `vpn_push_rules` / `vpn_set_proxy_mode` 都查归属）。
- 退出收尾（托盘退出、Cmd+Q、关机/注销、更新前断开）同样按归属走：`owner.json` 指向另一个还活着的进程时，本次退出不写 `stop.flag`、也不删它的控制文件——以前关掉开发版会把安装包正在用的隧道一起拆了。
- 归属进程已经消失（崩溃 / 被强杀）时任何实例都能接管收尾，不会因为残留的 `owner.json` 卡住。
- Windows 的计划任务名按构建分开（`WsWorkbenchVpnWorker` / `WsWorkbenchVpnWorker-dev`）：任务里写死了 exe 与会话目录，共用名字会让两个实例每次连接都互相覆盖、各弹一次 UAC。

主进程被强杀（任务管理器结束任务、崩溃）时，worker 会在 200 毫秒内从 `main.pid` 发现父进程消失，按正常流程结束 openconnect、还原路由与 DNS 后退出；`main.alive` 心跳过期是兜底路径（约 12 秒）。如果连 worker 一起被杀，Windows 上 openconnect 挂在带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的作业对象里，worker 句柄一关就会被系统结束，隧道不会留在后台（残留的路由/NRPT 规则会在下次连接时由 `cleanup_stale` 清掉）。
