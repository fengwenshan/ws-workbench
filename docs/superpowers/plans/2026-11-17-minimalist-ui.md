# Developer Minimalism 落地实施计划

- 设计稿：[2026-11-17-minimalist-ui-design.md](../specs/2026-11-17-minimalist-ui-design.md)
- 原则：每个阶段结束都要 `pnpm build` 通过；改完统一用浏览器 mock 预览亮/暗两套截图。

## P0 规范文档

- [x] 新增 `docs/ui.md`：规范的执行版（token 表、字体、间距、圆角、阴影、组件、检查清单）
- [x] `AGENTS.md` §10 引用 `docs/ui.md`，§15 增加「亮暗双主题自检 / 无新增硬编码色值」检查项
- [x] `README.md` 文档索引补 `docs/ui.md`

## P1 Token 层

- [x] `src/assets/styles/main.css`：重写 `:root` 亮色令牌 + `html[data-theme='dark']` 暗色覆盖
- [x] 全局：13px/1.6、系统字体栈、`--font-mono`、滚动条变量化
- [x] `.btn` 系列按规范重写（30px 高、6px 圆角、次按钮白底 1px 边框、危险按钮白底红字）
- [x] 全局过渡 `background-color/color/border-color .2s ease`

## P2 antd 主题与切换

- [x] 新增 `src/theme/antdTheme.ts`（buildAntdTheme，亮/暗 algorithm + token/components）
- [x] 新增 `src/composables/useTheme.ts`（mode/resolved/apply/持久化/antd theme computed）
- [x] `App.vue`：`<ConfigProvider :theme="antdTheme">`
- [x] 设置页新增「外观」卡片（Segmented 跟随系统/浅色/深色）
- [x] `quick/main.ts`、`clipboard/main.ts` 接入主题初始化

## P3 外壳

- [x] `App.vue` 样式：标题栏、图标栏、项目树、工作区、弹窗
- [x] 树选中态改浅灰 + 2px 品牌蓝竖线；层级竖线
- [x] 全局去重阴影/渐变

## P4 首页与项目

- [x] `ProjectList.vue`、`ProjectDetail.vue`、`GroupDetail.vue`、`ProjectConfig.vue`
- [x] `AttachmentPanel.vue`、`AttachmentPreview.vue`、`OpenWithButton.vue`
- [x] `NoteSidebar.vue`、`NoteWorkspace.vue`、`NoteEditor.vue`
- [x] `PackProgress.vue`、`LogDrawer.vue`、`RulesEditor.vue`、`QuickOpenPalette.vue`

## P5 VPN

- [x] `VpnPanel.vue`、`VpnProfileDialog.vue`、`VpnRuleDialog.vue`、`VpnInstallGuide.vue`、`VpnRoutePreview.vue`、`VpnStatusDot.vue`

## P6 清理

- [x] `CleanerPanel.vue`、`InstalledAppsPanel.vue`、`PopupPanel.vue`、`PopupRuleDialog.vue`、`PopupShield.vue`、`PopupWindowPicker.vue`、`SleepPanel.vue`、`AppIcon.vue`

## P7 LANLink

- [x] 壳与侧栏：去玻璃拟态/渐变/16px 圆角，改白卡 + 1px 边框 + 12px
- [x] 聊天、消息、文件卡、菜单、弹窗、设置页全部令牌化

## P8 设置页

- [x] `SettingsPanel.vue`、`ResourcePanel.vue`（去 3 处渐变）、`HostsEditor.vue`、`ClipboardManager.vue`、`ShortcutSettingsDialog.vue`

## P9 独立窗口

- [x] `QuickPanel.vue`、`QuickMessages.vue`（快捷面板）
- [x] `ClipboardPanel.vue`
- [x] `src/uninstall/styles.css` 令牌化（保持浅色）

## P10 验证

- [x] `grep` 检查 src 下无字面色值（白名单外）
- [x] `grep` 检查无 `gradient`、无 `backdrop-filter`
- [x] `pnpm build` 通过
- [x] 浏览器 mock 截图：首页、项目详情、附件、笔记、VPN、清理、LANLink 聊天/设置、设置页、快速面板、剪贴板；亮色 + 暗色
- [x] 截图自查：无漏改浅色块、对比度正常、间距/圆角统一

## P11 收尾

- [x] 更新 `docs/` 中与视觉相关的旧描述（如 LANLink 的 Glass Deck 说法）
- [x] 汇总变更清单给用户（不提交、不推送）

## 执行记录（2026-11-17）

已落地：

- 令牌层：`src/assets/styles/main.css` 重写为亮/暗两套（`html[data-theme='dark']`），补齐语义令牌（`--on-primary`、`--success-text`、`--warning-text`、`--danger-text`、`--code-bg`、`--mask`）；新增 `.mono`；按钮三态、滚动条、antd 组件收口。
- 主题：`src/theme/antdTheme.ts`（`buildAntdTheme(dark)`，ant-design-vue 4.2.6 无组件级 `ComponentToken`，组件细节改在 main.css 用变量覆盖）、`src/composables/useTheme.ts`（system/light/dark + matchMedia + `safeStorage` 持久化 `ws-workbench:ui:theme`）；`App.vue` 的 `ConfigProvider` 接 `:theme`；设置页新增「外观」卡片；quick / clipboard 入口各调一次 `initTheme()`。
- 全量迁移 38 个文件：字面色值全部换成令牌；4 处渐变、LANLink 玻璃拟态与浮起阴影、彩色投影全部去掉；圆角收敛到 4/6/12；11px 字号抬到 12px；等宽字体统一走 `--font-mono`；技术栈 / VCS 标签改中性色。
- LANLink：模块不再自己套任何外框——直接铺满工作区（无 margin/padding/外层边框），工作区自身（标题栏下边线、图标栏右边线）就是外框；左右两栏保留卡片（1px 边框 + 12px 圆角，中间 12px 间隙）。选中会话改「浅灰底 + 2px 品牌蓝竖线」；文件卡去掉旧的大阴影。
- 卸载窗口：`--un-*` 令牌补齐，仍保持浅色（唯一例外）。
- 文档：新增 `docs/ui.md`（规范正文）；`AGENTS.md` §2/§10/§15 同步；README 文档索引加一行。

验证：`pnpm build` 绿；浏览器 mock 截图看过首页、LANLink 聊天/设置、设置页、代理、清理、快速面板、剪贴板，亮色 + 暗色各一组（截图在 `.tmp/preview/ui-*.png`，未提交）。

待办（不属于本轮代码）：

- 真机 `pnpm exec tauri dev` 重启后手工过一遍托盘面板、剪贴板窗口的暗色首帧（独立窗口初始化时机已处理，但真实 WebView 未验）。
- 剩余 `font-size: 13px` 等奇数尺寸属规范字体档位，不按「偶数 px」规则折叠。

补充（浏览器预览）：`pnpm dev` 起的是纯 Vite，没有 Tauri 注入的 `__TAURI_INTERNALS__`，`App.vue` 里 `getCurrentWindow()` 会直接抛错导致白屏。新增 `src/dev/browserTauriShim.ts`，四个入口（index / quick / clipboard / uninstall）在 `import.meta.env.DEV` 且没有真实 internals 时装一份假后端（窗口元数据、事件注册、首页/设置/LANLink/剪贴板只读命令 + 少量假数据）。验证：浏览器打开四个页面控制台 0 报错；`pnpm build` 绿且 `dist` 里不含假数据。说明写进 `docs/development.md`「浏览器里看样式」。

补充（LANLink 贴边细节）：两栏卡片贴住工作区上/下/右/左边时不画外沿的框——`.lan-side` 只留 `border-right`，圆角只做右侧两只角（`0 12px 12px 0`）；`.lan-main`（空态与对话共用）只留 `border-left`，圆角只做左侧两只角（`12px 0 0 12px`）。

补充（项目 / VPN 模块统一）：按 LANLink 的「工作区即外框」处理。项目模块 `.tree-panel` 只留右边线 + 右侧两只圆角，`.home-body` 变成贴住工作区右/上/下边的卡片（`border-left` + 左侧两只圆角 + `bg-primary`），拖动条 `.resizer-tree` 加宽到 12px 当两栏间隙；VPN 的 `.vpn-body` 加 12px `gap`，`.profile-list` 圆角同左栏，`.detail` 变成右侧卡片（`border-left` + 左侧两只圆角 + `bg-primary`）。规范写进 `docs/ui.md` §4。

补充（两栏长度一致）：新增 `--pane-left-width: 260px` 令牌，LANLink `.lan-side`（原 300）、VPN `.profile-list`（原 232）改用该令牌，项目树默认宽度同为 260（`useWorkbenchLayout.ts` 里加了同步注释）。实测三个模块左栏 260px、右栏起点都在 x=328、两栏间隙 12px。

补充（右栏内容卡片）：项目 `.home-body`、VPN `.detail` 从「贴边、只留左边线 + 左侧圆角」改为独立卡片——`margin: 12px 12px 12px 0`、整圈 `1px` 边框、`12px` 四角圆角；左栏列表仍贴边（只留朝中间一侧边线与圆角）。亮/暗各截一张确认。LANLink 右栏 `.lan-main` 同步改成独立卡片（`margin: 12px 12px 12px 0` + 整圈边框 + 四角圆角），三个两栏模块的右栏内容区处理一致。

补充（贴边收口）：`.home-body` 去掉 `margin`，与 VPN `.detail`、LANLink `.lan-main` 一致——右栏内容区贴住工作区右/上/下边，保留整圈 1px 边框 + 12px 圆角；`.titlebar` 去掉 `border-bottom` 下划线，改由内容区边界分界。

补充（顶部边框线）：`.tree-panel`（项目）、`.profile-list`（VPN）、`.lan-side`（LANLink）各补 `border-top: 1px`，与右栏卡片的顶边在同一水平线；标题栏仍不画下划线。

补充（rail 顶边）：`.rail` 补 `border-top: 1px`，顶部横线从图标栏开始，经左栏列表、右栏卡片连成一条（两栏间隙处断开）。

补充（左栏页头统一）：项目 / VPN 左栏页头照 LANLink 的样式改成「20px 主题蓝图标 + 14px/700 标题 + 右侧 30×30 图标按钮（+）」，高度用新 token `--pane-head-height: 56px`，去掉下边框；`.detail-tabs` 页签行高度跟着改成 56，左右标题同一水平线。LANLink 页头图标换成和左侧 rail 同一枚（三点连线），三处图标不再各画一套。

补充（左侧菜单顺序）：rail 里「系统清理」和「LANLink」调换，顺序变成 项目 → 代理 → LANLink → 系统（设置仍在底部）；快捷键映射未动（⌘4 仍是系统清理、⌘5 仍是 LANLink，与 docs/features.md 的描述一致）。

补充（参考 VS Code 的四周间距）：右栏内容卡片改成四周留 12px 边距（左 = 两栏间隙，上/右/下同为 12px），整圈 1px 边框 + 12px 圆角；项目树收起（`.workspace.tree-collapsed`）、VPN 配置列表收起（`.vpn-body.is-collapsed`）、窗口 ≤900px 自动收树时，卡片自己补 12px 左边距。实测 1100×700 下 `.home-body`（含收起态）、`.detail`、`.lan-main` 四边都是 12px。

补充（间距再收一轮）：左栏列表离 rail 12px；两栏间隙从 12px 收到 8px（`.resizer-tree` 宽度、`.vpn-body` / `.lan-share-chat` 的 gap）；右栏卡片去掉上边距（顶部贴标题栏），只留右/下 12px；左栏收起时卡片补 12px 左边距。实测 1348×780 下左栏 x=68、卡片 x=336 / top=38 / right=1336 / bottom=768。

补充（左外边距挪到窗口边缘）：左栏不再和 rail 之间留 12px（改成紧贴、不被分割），12px 移到「窗口左边 ↔ rail」——`.rail { margin-left: 12px }`；其余保持：两栏间隙 8px、卡片顶部贴标题栏、右/下 12px。实测 1348×780 下 rail x=12–68、左栏 68–328、卡片 336–1336 / top=38 / bottom=768，收起左栏后卡片 x=80。

补充（左栏收成一张卡）：`.rail` 改成卡片——`margin: 0 0 12px 12px`、1px 边框、`border-radius: 12px 0 0 12px`（左上/左下圆角）、右边框作为 rail 与模块栏的分界；模块栏（`.tree-panel` / `.profile-list` / `.lan-side`）去掉左边框、只做右侧两只圆角，与 rail 连成同一张卡，底部同样留 12px。没有模块栏时（清理 / 设置 / 收起项目树或 VPN 列表）rail 加 `.rail-solo` 四角全圆。实测 1348×780：rail 12–68 / 38–768，左栏 68–328，卡片 336–1336，三者底部都在 768。

补充（外边距与两栏间隙收到 4px）：rail 的 `margin: 0 0 4px 4px`；左栏 `.tree-panel` / `.profile-list` / `.lan-side` 下边距 4px；右栏 `.home-body` / `.detail` / `.lan-main` 改成 `margin: 0 4px 4px 0`；两栏间隙（`.resizer-tree` 宽度、`.vpn-body` / `.lan-share-chat` 的 gap）统一 4px；左栏收起时卡片补 4px 左边距（`.workspace.tree-collapsed`、`.vpn-body.is-collapsed`、窄窗口媒体查询）。实测 1348×780：rail 4–60 / 38–776，左栏 60–320，卡片 324–1344。

补充（底部单独放到 8px）：左右外边距与两栏间隙保持 4px，底部统一 8px——`.rail { margin: 0 0 8px 4px }`、左栏 `margin-bottom: 8px`、右栏 `.home-body` / `.detail` / `.lan-main` 改成 `margin: 0 4px 8px 0`。实测 1348×780：rail 4–60 / 38–772，左栏 60–320，卡片 324–1344 / bottom=772（距窗口底 8px）。

补充（外边距统一 8px）：左右外边距也放到 8px——`.rail { margin: 0 0 8px 8px }`、右栏 `.home-body` / `.detail` / `.lan-main` 改成 `margin: 0 8px 8px 0`，左栏收起时补 8px 左边距；两栏间隙仍是 4px。实测 1348×780：rail 8–64、左栏 64–324、卡片 328–1340 / bottom=772，收起左栏后卡片 x=72。

补充（笔记侧栏页头对齐左栏页头）：`.note-sidebar` 的 `.sidebar-tabs` 从 40 高、13px 胶囊页签改成 `--pane-head-height`（56）高、14px 页签，选中态品牌蓝 + 700 + 底部 2px 指示条（去掉了 `--bg-active` 底色），左侧内边距 20 与左栏标题对齐、页签间距 24；收起按钮从 antd `<Button type="text" size="small">` 换成项目页头同款 30×30 图标按钮（hover 浅灰底）；计数徽标选中态改用 `--primary-soft`，去掉写死的 `rgba(59, 130, 246, 0.14)`。实测页头 1084–1318 / 108–164，页签字号 14px/700。

补充（笔记侧栏还原成 tag 切换）：上一版把笔记侧栏页头改成了左栏页头那一套（56 高 + 2px 指示条），按要求还原成原来的两个 tag 页签（26 高、圆角 6、选中铺 `--bg-active` 浅底、字号 14 保持偶数），并去掉 `.sidebar-tabs` 的 1px 下边框线；页头高度回到 `--panel-header-height`（40）。计数徽标保留 `--primary-soft`（不再写死 rgba）。

补充（笔记侧栏最终确定为 tag 切换）：试过左栏页头那套（56 高 + 2px 指示条）和 VS Code 面板页签那套（纯文字 + 1px 下划线），最终都退回原来的两个 tag 页签：26 高、圆角 6、14px、选中品牌蓝 + `--bg-active` 浅底，行高 40，不画整行分隔线。计数徽标保留 `--primary-soft`。

补充（笔记侧栏改用 antd Tabs 组件）：试过自定义下划线页签、tag 胶囊，最终按要求换成 antd `Tabs`——大纲 / 附件两个 `TabPane`，页签行 40 高、14px、2px ink bar，`::before` 那条整行分隔线隐藏，收起按钮放 `#rightExtra`。注意笔记侧栏渲染在 `.detail-tabs` 内部，ProjectDetail 的 `:deep(.ant-tabs-*)` 会盖过来，样式统一加 `.note-sidebar` 前缀提权。实测页签行 1084–1318 / 108–148，字号 14，tab margin 0 8px 0 0，无 runtime 报错。

补充（去掉页签滑动下划线）：`.note-sidebar .note-tabs :deep(.ant-tabs-ink-bar) { display: none }`，页签选中只靠文字色（实测 `rgb(37, 99, 235)`）区分；`.ant-tabs-nav::before` 仍是 `none`，整行没有分隔线。

补充（VPN 详情页去卡片）：`.proxy-mode-card` 与 `VpnRoutePreview` 的 `.preview` 都去掉 border / radius / background / padding，直接铺在详情页上，`.detail` 的列间距从 16 调到 24 补回层次；路由表 `.routes` 去外框，表头加 1px 下边线、行间保留 1px 边线；顺手把 `via-tag` 的 `rgba()` 换成 `--primary-soft` / `--success-soft`。实测 `.proxy-mode-card` / `.preview` / `.routes` 的 border 均为 0、背景透明。核对路由表时临时在 `src/dev/browserTauriShim.ts` 里塞了一个「已连接 + 两条路由」的 `vpn_get_snapshot`，截图后已还原。

补充（设置页去卡片）：`SettingsPanel` 的 `.card` 去掉 border / radius / background，只留 `padding: 0`，`.settings` 列间距 16 → 24；「运行环境 → 网络」子块的分隔线由虚线改实线；`.toast` 的 `rgba()` 换成 `--primary-soft`；`ResourcePanel` 的 `res-tile` 悬停去掉位移和阴影，只保留边框变品牌蓝。实测 5 个 `.card` 段落 border 0 / padding 0 / 背景透明，`.settings` gap 24px。

补充（设置页去掉那根横线）：`.net-block` 去掉 `border-top` 和 `padding-top`，只留 `margin-top: 20px`，实测 border-top 0 / margin-top 20px / padding-top 0，「设置文件」和「网络」之间只剩留白。

补充（左右外边距 4px + 系统模块加内容卡）：`.rail` 左外边距 8 → 4，`.home-body` / `.detail` / `.lan-main` 右外边距 8 → 4（底部仍 8），`.tree-collapsed .home-body`、`.vpn-body.is-collapsed .detail` 的补位左边距同样 4；`CleanerPanel` 的 `.cleaner` 加 `margin: 0 4px 8px 4px` + 1px 边框 + 12px 圆角 + 白底，让「系统」模块也有一张内容卡。实测 1348×780：rail 4–60、左栏 60–320、右卡 324–1344、底 772（8px）。

补充（设置 / 系统页改成左侧标题导航）：`SettingsPanel` 从「5 个 `.card` 段落」改成一张内容卡 + antd `Tabs tab-position="left"`（外观 / 检查更新 / 运行环境 / 附件上传 / 资源），正文里的 h4 标题删掉、标题只留在左侧导航；`CleanerPanel` 的 tabs 同样换成 `tab-position="left"`（屏幕休眠 / Hosts / 剪贴板 / 软件卸载，Windows 多一个弹窗拦截）。两处都是：导航 168px、条目 32 高、圆角 6、选中 `--bg-active` + 品牌蓝 + 600，隐藏 ink bar 和 `nav::before`，导航右边 1px 竖线。注意 `Tabs` 竖向时根节点要 `flex-direction: row`（我们原先写死 column，会把导航挤到上面）、导航要 `height: 100%` 才让竖线通到底。实测：设置卡 64–1344 / 38–772，导航 65–233，内容 232–1343；系统页同样。

补充（设置 / 系统改两栏）：左栏不再用 antd 竖向 `Tabs`，改成「一张左栏卡片 + antd `Menu mode="inline"`」，样式对齐项目树：卡宽 `--pane-left-width`、栏头 56px（20px 品牌图标 + 14/700 标题，设置页右侧放「重新检测」btn-xs）、条目 30 高/6 圆角/选中 `--bg-active`+`--text-active`+500。右栏内容**不套卡片**，直接铺在工作区背景上，`overflow-y: auto` 独立滚动；`railSolo` 里去掉 cleaner/settings，让 rail 和左栏拼成一张卡（和项目一致）。同时把「屏幕休眠」两张内层 `.sleep-card` 拆成无边框段落（间距 12 → 24），「资源」段正文里重复的 h4「资源」删掉（标题只在左栏导航）。实测 1348×780：rail 4–60、左栏卡 60–320（宽 260）、右栏 324–1344（无边框、背景透明），`.sleep-card` border 0 / 无背景。

补充（右栏恢复内容卡 + 设置页滚动联动）：右栏内容重新套回一张内容卡（`.settings-main` / `.cleaner-main`：`margin: 0 4px 8px 0` + 1px 边框 + 12px 圆角 + `--bg-primary`），卡内 `.settings-body` 独立滚动。「设置」不再按分区切换页面：五个分区顺序排在一列里（每个区块带 14px/600 的 h4 标题），右侧滚动时按 `scrollTop + 40` 命中当前分区、左侧 antd Menu 自动选中（滚到底时强制选最后一节，否则最后一节永远选不上）；点导航用 `scrollTo({ top: offsetTop - 16, behavior: 'smooth' })` 滚过去。顺带修回上一轮 CSS 重写时漏掉的 `.info-grid / .info-row / .info-label / .info-value / .tag* / .mono / .btn-xs / .muted`（`.info-label` 宽度 104px、`.info-grid` 两列各 481px）。实测 1348×780：rail 4–60、左栏卡 60–320、右栏卡 324–1344/38–772（1px 边框、12 圆角、白底）。

补充（模块栏再点一次收起）：`useWorkbenchLayout` 新增 `cleanerMenuCollapsed` / `lanMenuCollapsed` / `settingsMenuCollapsed`，`selectTool` 在「已经在该工具」时切换对应收起状态，切到别的工具时重置为展开（和项目树 / VPN 列表同一套语义）；`railSolo` 把这三个也纳入，收起时 rail 自己成整张卡（四角 12）。`CleanerPanel` / `SettingsPanel` / `LanSharePanel` 收 `menu-collapsed` prop：左栏 `v-show`，根节点加 `is-collapsed`（`gap: 0`），右栏内容卡 `margin-left: 4px`。实测：展开 菜单 60–320 / 内容 324–1344；收起 菜单 display:none、内容 64–1344、rail `border-radius: 12px`；切走再回来菜单恢复 60–320。

补充（开发者模式改成隐藏入口）：`SettingsPanel` 里「开发者模式」行默认不渲染，`devRowVisible = props.developerMode || devRowUnlocked`；连点 `当前版本` 8 次（相邻间隔 < 1200ms，用时间戳判断，不起定时器）后 `devRowUnlocked = true` 并给一条 toast「开发者模式选项已显示」。`watch(props.active)` 在离开设置页时重置解锁态；关掉开发者模式时同时重置，这一行立刻消失。实测（浏览器预览）：默认只有「当前版本」一行 → 点 8 下出现「开发者模式」→ 打开（确认弹窗）后离开再回来仍在 → 关闭后立刻消失、再进来也没有。
