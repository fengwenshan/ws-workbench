# Developer Minimalism 界面规范落地设计

- 日期：2026-11-17
- 状态：已确认（用户选定「全量一次落地」+「三档主题，默认跟随系统」）
- 范围：主窗口全部页面 + 快捷面板 + 剪贴板窗口；卸载窗口例外（固定浅色）

## 1. 目标

把用户给出的 Developer Minimalism 规范变成项目里可执行的单一来源：

1. 90% 界面由黑白灰构成，色彩只用于主操作与状态；
2. 去掉所有渐变、玻璃拟态、重阴影，层级靠 1px 边框与微弱底色差；
3. 建立亮/暗两套 token，主题三档可切换（跟随系统 / 浅色 / 深色）；
4. 新代码不再允许硬编码颜色，规范写进 `docs/ui.md` 并由 `AGENTS.md` 引用。

## 2. 色板

### 2.1 令牌表

| 语义 | 变量 | 亮色 | 暗色 |
| --- | --- | --- | --- |
| 全局背景 | `--bg-secondary` / `--bg-layout` | `#f9fafb` | `#0f172a` |
| 卡片/输入面 | `--bg-primary` / `--bg-surface` | `#ffffff` | `#1e293b` |
| 悬浮/选中 | `--bg-hover` | `#f3f4f6` | `#334155` |
| 选中弱底 | `--bg-active` | `#eff6ff` | `rgba(59,130,246,.16)` |
| 只读底 | `--bg-readonly` | `#f9fafb` | `#0f172a` |
| 标签底 | `--bg-tag` | `#e5e7eb` | `#334155` |
| 主文字 | `--text-primary` | `#111827` | `#f1f5f9` |
| 次文字 | `--text-secondary` | `#4b5563` | `#cbd5e1` |
| 说明文字 | `--text-muted` | `#6b7280` | `#94a3b8` |
| 占位/禁用 | `--text-disabled` | `#9ca3af` | `#64748b` |
| 边框/分割线 | `--border-color` | `#e5e7eb` | `#334155` |
| 品牌色 | `--primary` | `#2563eb` | `#3b82f6` |
| 品牌悬浮 | `--primary-hover` | `#1d4ed8` | `#60a5fa` |
| 品牌弱底 | `--primary-soft` | `#eff6ff` | `rgba(59,130,246,.16)` |
| 成功 | `--success` | `#10b981` | `#34d399` |
| 危险 | `--danger` | `#ef4444` | `#f87171` |
| 警告 | `--warning` | `#f59e0b` | `#fbbf24` |
| 危险弱底 | `--danger-soft` | `#fef2f2` | `rgba(248,113,113,.16)` |

说明：

- 规范未给警告色与暗色主色，这里补齐：警告沿用现有 `#f59e0b`；暗色下品牌蓝提亮到 `#3b82f6`（保证在 `#0f172a` 上可读），主按钮底色仍用 `#2563eb` + 白字。
- 暗色下不使用纯黑/纯白：文字用 `#f1f5f9`，避免刺眼。
- 禁止在组件里出现 `#` 开头的字面色值（`main.css` 的令牌定义、`docs/` 示例除外）；半透明色用 `rgb(… / …)` 形式并集中放在令牌里。

### 2.2 用色规则

- 一个页面只能有一个主按钮（品牌蓝底白字）。
- 危险操作按钮：白底 + 极浅红边框 + 红字，禁止大红底铺满。
- 列表选中态不用亮蓝铺满：`--bg-hover` 底 + 主文字色 + 字重 600；左侧导航树选中项加 2px 品牌蓝竖线。
- 状态色只用于状态本身（绿点/红点/警告条），不做装饰。

## 3. 字体与排版

- UI 字体：`-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, "PingFang SC", "Microsoft YaHei", sans-serif`。
- 等宽字体（新增 `--font-mono`）：`ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace`。
  强制用于：IP、端口、MAC/设备 ID、文件路径、版本号、代码、日志。
- 字号：页面标题 20/600；卡片标题 15/600；正文与列表 13–14/400；辅助说明 12/400。
- 行高：正文 1.6，编辑器/代码 1.8。
- 全局基准字号 13px（antd `fontSize: 13`），间距按 4px 网格。

## 4. 间距、圆角、阴影、过渡

- 间距只允许 4 的倍数：4 / 8 / 12 / 16 / 24 / 32。
- 圆角只用三档：小元素 4px、中元素（按钮/输入/下拉）6px、大元素（卡片/弹窗）12px；现有 8/9/10px 向 6px 收敛，16px 向 12px 收敛。
- 阴影只用两级：
  - 卡片：`0 1px 3px rgba(0, 0, 0, 0.02)`；暗色 `0 1px 2px rgba(0, 0, 0, 0.35)`。
  - 浮层（弹窗/菜单/悬浮按钮）：`0 4px 6px -1px rgba(0, 0, 0, 0.05)`。
- 所有 hover / active 用 `transition: background-color .2s ease, color .2s ease, border-color .2s ease`（不用 `all`，避免布局属性动画）。

## 5. antd v4 落地

项目是 ant-design-vue 4.2.6（token 体系对应 antd 5），`ConfigProvider` 支持 `theme`：

```ts
// src/theme/antdTheme.ts
import { theme as antdTheme, type ThemeConfig } from 'ant-design-vue'

export function buildAntdTheme(dark: boolean): ThemeConfig {
  return {
    algorithm: dark ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
    token: {
      colorPrimary: dark ? '#3b82f6' : '#2563eb',
      colorSuccess: dark ? '#34d399' : '#10b981',
      colorError: dark ? '#f87171' : '#ef4444',
      colorWarning: dark ? '#fbbf24' : '#f59e0b',
      colorText: dark ? '#f1f5f9' : '#111827',
      colorTextSecondary: dark ? '#cbd5e1' : '#4b5563',
      colorBorder: dark ? '#334155' : '#e5e7eb',
      colorBgLayout: dark ? '#0f172a' : '#f9fafb',
      colorBgContainer: dark ? '#1e293b' : '#ffffff',
      borderRadius: 6,
      fontSize: 13,
      fontFamily: SYSTEM_FONT,
      boxShadow: '0 1px 3px rgba(0, 0, 0, 0.02)',
    },
    components: {
      Button: { controlHeight: 30, defaultShadow: 'none', primaryShadow: 'none' },
      Tree: { nodeSelectedBg: dark ? '#334155' : '#f3f4f6', titleHeight: 28 },
      Segmented: { trackBg: dark ? '#0f172a' : '#f3f4f6', itemSelectedBg: dark ? '#1e293b' : '#ffffff' },
      Card: { paddingLG: 20 },
    },
  }
}
```

- v4 不支持的 token 用 CSS 覆盖，不硬写组件内色值。
- `ConfigProvider` 只加 `:theme`，locale 不动；主题切换时 `theme` 计算属性变化即热更新。

## 6. 主题切换

- `src/composables/useTheme.ts`：
  - `mode: 'system' | 'light' | 'dark'`，默认 `system`；
  - `resolved` 由 `matchMedia('(prefers-color-scheme: dark)')` 推导，监听变化；
  - `apply()` 写 `document.documentElement.dataset.theme = 'dark' | 'light'`；
  - 持久化键 `ws-workbench:ui:theme`，走 `safeStorage`（加密）；
  - 导出 `antdTheme` computed 给 ConfigProvider。
- 设置页「外观」卡片：`Segmented` 三档（跟随系统 / 浅色 / 深色），放在「检查更新」卡片之前。
- 独立窗口：`quick/main.ts`、`clipboard/main.ts` 调用同一个初始化函数（读同一偏好、加 `data-theme`）。
- 启动闪烁：主窗口 `visible: false`，首帧后才显示，不会有白闪；快捷面板/剪贴板窗口读取加密偏好是异步的，理论上可能先亮一帧浅色，这是可接受的已知限制。
- 卸载窗口 `src/uninstall/**` 固定浅色：它是系统级一次性窗口，自带独立样式，不接主题；文档注明例外。

## 7. 组件规范（落地细节）

| 组件 | 规则 |
| --- | --- |
| 按钮 | 高 30（紧凑 28 仅弹窗内）、圆角 6、左右内边距 16；次按钮白底 + 1px `--border-color`；危险按钮白底红字；hover 背景 `--bg-hover` |
| Segmented | 容器 `--bg-hover`，选中项 `--bg-primary` + 极弱阴影 |
| 列表/树 | 选中 `--bg-hover` + 文字加深；导航树选中项左侧 2px 品牌蓝；树层级用 1px 竖线（`--border-color`） |
| 数据网格 | 两列 `.info-grid`：标签 `--text-muted`，值 `--text-primary` + `--font-mono`（IP/端口/路径/版本） |
| 卡片 | 白底（暗色 `#1e293b`）+ 1px 边框 + 卡片级阴影 + 圆角 12；标题 15/600 |
| 弹窗 | 圆角 12，遮罩 `rgba(0,0,0,.45)`（暗色 `.6`），内容底 `--bg-primary` |
| 分段/标签页 | 选中用 `--bg-hover`，不用彩色填充 |
| 输入框 | 高 30，1px 边框，focus 边框 `--primary` + 2px 品牌弱底描边 |

## 8. 迁移范围

- 全量替换 `src/**` 下的硬编码色值（当前 121 处，主要分布在 `main.css`、`ProjectDetail`、`ResourcePanel`、`settings/ResourcePanel`、`QuickPanel` 等）。
- 删除渐变（4 处）：
  - `components/settings/ResourcePanel.vue` 三处图标/封面渐变；
  - `components/cleaner/PopupPanel.vue` 头部渐变。
- LANLink 去玻璃拟态：`backdrop-filter`、半透明白、16px 圆角、光晕全部去掉，改为白卡 + 1px 边框 + 12px 圆角。
- `src/uninstall/styles.css` 保持浅色，但把色值换成同名语义变量（不引入 dark 覆盖）。
- 标题层级统一：工作区页面标题 20/600，卡片/分组标题 15/600。

## 9. 工程约束

- 新增 `docs/ui.md`：规范正文（本文的去项目化版本），`AGENTS.md` §10 引用它。
- `AGENTS.md` 硬约束新增：禁止新增硬编码色值；禁止渐变/玻璃拟态/超出两级的阴影；圆角只用 4/6/12；间距 4 的倍数；IP/端口/路径/版本必须等宽；每个页面只有一个主按钮。
- 提交前检查新增：亮色 + 暗色各看一遍、`grep` 无新增字面色值。

## 10. 验证

- `pnpm build`（含 `vue-tsc --noEmit`）通过。
- `grep -rnE '#[0-9a-fA-F]{3,8}' src` 只允许出现在 `main.css` 令牌定义与卸载窗口变量定义。
- 浏览器预览（Tabbit + `__TAURI_INTERNALS__` mock）逐页截图：首页、项目详情、附件、笔记、VPN、清理、打包日志、LANLink 聊天/设置、设置页、快速面板、剪贴板，亮暗各一张。
- 对暗色截图人工检查对比度与漏改的浅色块。
- 不做 Git 提交与推送（用户未要求）。

## 11. 风险

- 一次性改 50+ 文件，diff 大；按阶段提交式推进，每阶段 `pnpm build`。
- antd v4 与规范里的 v5 示例存在 token 名差异，按 v4.2.6 实际支持项落地，缺口用 CSS 变量补。
- 全局 13px 会让部分两行布局变紧，需要逐页截图确认。
- 暗色下漏改的硬编码色会发白，靠 grep + 双主题截图兜底。
