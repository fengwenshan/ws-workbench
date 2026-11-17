# 目录结构与分层

> 来源：原 [README](../README.md) 的「目录结构」章节；分层规则摘要见 [AGENTS.md](../AGENTS.md) 第 0、11 节。

```
AGENTS.md          项目规则（编码 Agent 必读；功能细节见 README）
src-tauri/src/
  commands/        前端命令入口按业务域拆分
    config.rs      配置读写、存储信息、本地键值加密
    project.rs     打包、校验、项目类型与 VCS 识别
    system.rs      打开目录/终端/编辑器、平台与网络信息
    attachments.rs 附件加解密、上传目录解析、笔记内联图片
    cleaner.rs     系统工具命令（列表/卸载/残留）
    cleaner_sleep.rs / cleaner_popup.rs
                   屏幕休眠 / 弹窗拦截（仅 Windows）的命令入口
    quick.rs       托盘快捷面板：隐藏、按页签内容调整高度、底部「显示主界面 / 退出」
    clipboard.rs   剪贴板历史命令层：changeCount 采集 / 运行时状态 / 面板显隐
    hosts.rs       Hosts 在线编辑：读取 / 保存 / 备份列表 / 还原（整文件写入）
    lanshare.rs    局域网分享命令层：快照 / 启停 / 设备名与接收目录 / 接收确认 / 发送
  util/            通用能力层
    fs.rs          目录创建、文本读写、可写性探测
    time.rs        本地 RFC3339 / UTC 毫秒时间戳
  config.rs        配置读写：整份加密、写入目录多级回退、历史字段迁移
  crypto.rs        字符串密文信封（SM4-CBC + HMAC-SM3）与主密钥托管
  filecrypto.rs    附件流式加密容器 WST1
  packer/          打包：遍历、包含规则过滤、复制、流式 ZIP；svn_platform/ 按平台定位 svn
  updates.rs       更新包断点续传下载 + minisign 校验（安装交给 updater 插件）
  models.rs        AppConfig / 分组 / 项目 / 附件元信息 / 默认包含规则
  platform.rs      操作系统判定
  netinfo.rs       本机网卡 MAC / IP（Windows 走 PowerShell，macOS/Linux 走 getifaddrs）
  process.rs/winps.rs  进程管理与 PowerShell 调用
  apps/            外部应用定位（finder / registry）
  clipboard/       剪贴板历史：mod.rs 共享采集/分类/快捷键/面板门面，
                   macos.rs / windows.rs / unsupported.rs 平台实现；
                   rich.rs（HTML 图文抽取 / 还原 / 哈希 / 缩略图）、store.rs（SQLite 读写与附件路径）
  hosts/           Hosts 在线编辑：内容校验（mod.rs）、备份（store.rs）、
                   提权写入在 file/ 下按平台拆（scripts.rs 放纯脚本文本生成）
  lanshare/        局域网分享：mod.rs 运行期状态/快照/启停、protocol.rs 报文与文件名清洗、
                   settings.rs 设置持久化（lan_share.json）、discovery.rs UDP 组播发现、
                   server.rs 接收 HTTP 服务（prepare 等待确认 / upload 流式落盘）、
                   client.rs 发送端（探活 / prepare-upload / 分块上传 / 进度事件）
  cleaner/         系统工具：安装列表/卸载/关联文件/残留扫描 + 屏幕休眠、弹窗拦截的后端
    related.rs     卸载窗口关联文件清单组装（分组 / 默认勾选 / 使用情况文案）
    installer_macos/ / installer_windows/  卸载实现，按平台拆
    running/       运行检测、优雅退出与强制退出（macos.rs / windows.rs 各一份）
    sleep/         防休眠（macos.rs IOKit 断言 / windows.rs SetThreadExecutionState；triggers/ 同结构）
    trash_guard/   删除安全层（白名单 + 废纸篓 + 提权删除），平台实现已拆
    popup.rs       弹窗拦截（仅 Windows：规则匹配 + EnumWindows 轮询 + 启发式打分）
  vpn/
    models.rs      规则、规则集、VPN 配置、状态
    mod.rs         模块导出与数据目录名
    rule_engine.rs 规则类型 / 走向解析（纯逻辑，可离线单测）
    split.rs       规则模式：按域名分流解析 + 隧道路由注入（macOS 转发器 / Windows 主动解析）
    win_dns.rs     Windows 专用：NRPT 规则与接口 DNS
    fulltunnel.rs  全局模式：半默认路由 + 系统 DNS 接管（带验证与回滚）
    dns_client.rs  最小 DNS 客户端：构造 A 查询、解析应答
    worker.rs      提权进程，连接生命周期主体
    commands.rs    主进程侧命令与状态轮询
    connector.rs   拉起 openconnect
    route.rs       路由表读写与崩溃清理
    script.rs      生成 vpnc-script
    credential.rs  密码加密存储
    platform/      连接器探测，按平台拆文件（mod.rs 只做 cfg 分派）
      common.rs    共享：执行 `--version` 探测、版本解析、探测结果模型
      macos.rs     macOS：只认安装包内置副本、清理旧路径登记与副本
      windows.rs   Windows：只认安装包内置副本、拒绝导入
    elevate.rs     提权启动
    guide.rs       openconnect 安装引导（Windows 只提示重装内置副本）
    session.rs     会话目录 IPC
src/
  components/      ProjectList / GroupDetail / ProjectDetail / ProjectConfig /
                   NoteEditor / AttachmentPanel / PackProgress / ExcludeRules / LogDrawer（VPN 日志抽屉复用）
    vpn/           VpnPanel / VpnProfileDialog / VpnRuleDialog / VpnRoutePreview /
                   VpnStatusDot / VpnLogDrawer / VpnInstallGuide
    settings/      SettingsPanel
    cleaner/       CleanerPanel / InstalledAppsPanel / MemoryPanel / AppIcon /
                   PopupPanel / SleepPanel / StartupPanel / DefaultAppsPanel
    hosts/         HostsEditor（整文件在线编辑 + 备份还原，编辑区是 Monaco）
    clipboard/     ClipboardManager（「系统 → 剪贴板」管理页签）
    lanshare/      LanSharePanel（局域网分享工具页：开关 / 设备卡片 / 待接收 / 发送进度）
  composables/     useConfig / usePack / useVpn / useCleaner / useApps / useUpdater /
                   useWorkbenchLayout / useEntitySelection / useProjectForm /
                   useDeleteConfirm / safeStorage / useKeepAwake / useStartup / usePopup /
                   useDefaultApps / useHosts / useClipboardHistory
  api/             后端命令的类型化封装（config / project / system / attachments / apps / cleaner /
                   sleep / startup / popup / defaultapps / clipboard / hosts / lanshare）
  editor/          Monaco 集成（hosts 语言 / 主题 / worker，首次打开 Hosts 页签才加载）
  utils/           通用小工具（time / dialog / path / error / hostsTokens / note-text /
                   note-assets / note-clipboard）
  types/           前后端共享的数据结构（含 cleaner / sleep / startup / popup / defaultapps / hosts / clipboard）
  quick/           托盘快捷面板（独立入口：quick.html + main.ts + QuickPanel.vue）
  clipboard/       剪贴板历史面板（独立入口：clipboard.html + main.ts + ClipboardPanel.vue）
scripts/release.mjs  构建、上传、更新清单合并
updates/latest.json  更新清单（由发布脚本生成并提交）
```

分层约定：

- 通用能力只放 `src/utils`、`src/api` 与 `src-tauri/src/util`，业务代码不重复实现路径、时间、对话框、命令调用。
- 前端每个业务一个 composable 或组件（项目、VPN、设置、附件各自独立），跨业务组合只在 `App.vue` 里做。
- Rust 命令按业务域放在 `commands/<域>.rs`，新增命令先归域再导出，避免所有入口堆在一个文件里。
- 改某个业务时只动它自己的模块；通用层变更需同时跑 `pnpm build` 与 `cargo test --lib`。
