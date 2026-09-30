# 引入 ant-design-vue@4 组件库（按需导入）设计文档

- 日期：2026-09-29
- 分支：feat/vpn
- 状态：待评审

## 1. 背景与目标

`ws-tools` 是一个 Tauri 2 + Vue 3.5 + Vite 6 + TypeScript 桌面应用（macOS WebKit 运行）。
当前前端没有引入任何 UI 组件库，界面完全由原生标签（`button` / `input` / `textarea` / `select` / `div` 自绘弹窗等）加手写 CSS 构成，样式集中在 `src/assets/styles/main.css` 的一套 CSS 变量与 `.btn*` 全局类上。

目标：引入 `ant-design-vue@4.x`，采用**按需导入**方式，并把现有界面的原始标签替换为 antd 组件，统一交互与视觉。

### 目标（In Scope）

1. 引入 `ant-design-vue@4`、`@ant-design/icons-vue`，以**按需导入**方式接入（不全局注册组件）。
2. 全量迁移：13 个 `.vue` 文件中的原始 `button` / `input` / `textarea` / `select` / `checkbox` 等标签替换为 antd 组件。
3. 自绘弹窗统一替换为 `a-modal` / `Modal.confirm()`。
4. 视觉采用 antd 默认风格（主色 `#1677ff`），不映射现有 `#3b82f6`。
5. 顶栏、徽标、进度、提示条等按映射表替换为对应 antd 组件。

### 非目标（Out of Scope）

- 不引入暗色模式（仅浅色）。
- 不改变任何业务逻辑、Tauri 命令调用、数据结构与状态管理方式。
- 不做与本次迁移无关的重构。
- 不新增页面或功能。

## 2. 现状盘点

- `.vue` 文件共 13 个：
  - 根：`src/App.vue`
  - 通用：`src/components/ProjectList.vue`、`ProjectConfig.vue`、`ExcludeRules.vue`、`LogPanel.vue`、`PackProgress.vue`
  - VPN：`src/components/vpn/VpnPanel.vue`、`VpnProfileForm.vue`、`VpnRuleTable.vue`、`VpnLearnCard.vue`、`VpnRoutePreview.vue`、`VpnInstallGuide.vue`
  - 设置：`src/components/settings/SettingsPanel.vue`
- 原始标签规模（约）：`button` ≈ 69、`input` ≈ 18、`textarea` 3、`select`/`option` 各 1、`checkbox` 若干。
- 自绘弹窗 5 处：`App.vue` 内 3 个 `add-form-overlay` + 1 个 `confirm-dialog`；`VpnPanel.vue` 内 1 个 `dialog-overlay`。
- 进度条 2 处：`PackProgress.vue` 等基于 `div` 实现。
- 无原生 `<table>`；`VpnRuleTable.vue` 与列表类页面用 `div` + grid 自绘。
- 样式：`src/assets/styles/main.css` 定义 CSS 变量（`--primary/#3b82f6` 等）与全局 `.btn/-primary/-secondary/-danger`。
- 入口：`src/main.ts` 仅 `createApp(App).mount('#app')` 并引入 `main.css`。
- 构建：`pnpm build` = `vue-tsc --noEmit && vite build`；dev 端口 8790。

## 3. 依赖与构建接入（按需导入）

### 3.1 新增依赖

| 包 | 类型 | 用途 |
| --- | --- | --- |
| `ant-design-vue@^4` | dependencies | 组件库本体 |
| `@ant-design/icons-vue` | dependencies | 图标 |
| `unplugin-vue-components` | devDependencies | 组件按需自动注册（`<a-button>` 等零 import） |
| `unplugin-auto-import` | devDependencies | `message` / `Modal` / `notification` 等命令式 API 按需自动导入 |

### 3.2 `vite.config.ts`

```ts
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import Components from 'unplugin-vue-components/vite'
import AutoImport from 'unplugin-auto-import/vite'
import { AntDesignVueResolver } from 'unplugin-vue-components/resolvers'
import path from 'path'

export default defineConfig({
  plugins: [
    vue(),
    Components({
      resolvers: [
        AntDesignVueResolver({
          importStyle: false, // antd v4 使用 CSS-in-JS，无需引入样式文件
          resolveIcons: true, // 图标同样按需解析
        }),
      ],
      dts: 'src/components.d.ts',
    }),
    AutoImport({
      resolvers: [AntDesignVueResolver({ importStyle: false })],
      dts: 'src/auto-imports.d.ts',
    }),
  ],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') },
  },
  clearScreen: false,
  server: {
    port: 8790,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
})
```

> `importStyle: false` 是 antd v4（CSS-in-JS）的官方正确配置：组件样式运行时注入，不需要按需引 less/css 样式文件。

### 3.3 `src/main.ts`

```ts
import { createApp } from 'vue'
import App from './App.vue'
import 'ant-design-vue/dist/reset.css'
import './assets/styles/main.css'

createApp(App).mount('#app')
```

按需模式下**不调用** `app.use(Antd)`，组件由 `unplugin-vue-components` 在编译期自动注册。

### 3.4 中文语言包与全局配置

在 `App.vue` 根部包一层 `a-config-provider`，locale 使用 antd 中文包：

```vue
<script setup lang="ts">
import zhCN from 'ant-design-vue/es/locale/zh_CN'
</script>

<template>
  <a-config-provider :locale="zhCN">
    <!-- 现有根布局 -->
  </a-config-provider>
</template>
```

生成的 `src/components.d.ts`、`src/auto-imports.d.ts` 由 `vue-tsc` 识别，纳入类型检查；两文件随源码提交。

## 4. 组件映射

| 现状 | 替换为 |
| --- | --- |
| `.btn` / `.btn-primary` / `.btn-secondary` / `.btn-danger` | `<a-button type="primary">` / `default` / `danger`（`size` 按原尺寸就近选择） |
| `<input>` | `<a-input>`；数字类可用 `<a-input-number>` |
| `<textarea>` | `<a-textarea>` |
| `<select>` + `<option>` | `<a-select>` + `<a-select-option>` |
| `<input type="checkbox">` | `<a-checkbox>`；开关语义用 `<a-switch>` |
| 3 个 `add-form-overlay` 表单弹窗 | `<a-modal>`（`v-model:open`） |
| `confirm-dialog` 确认弹窗 | `Modal.confirm()` |
| `VpnPanel.vue` 的 `dialog-overlay` | `<a-modal>` |
| `div` 进度条 | `<a-progress>` |
| 更新提示条 | `<a-alert type="info">` |
| 头部状态/徽标 | `<a-badge>` / `<a-tag>` |
| `VpnRuleTable.vue` 的 div-grid 表格 | `<a-table>`（含行内编辑） |
| `<h3>` 等标题 | `<a-typography-title :level="n">` |
| 现有字符图标（如有） | `@ant-design/icons-vue` 组件 |

## 5. 样式与布局清理

- 保留：`html/body/#app` 基础布局、滚动条样式、macOS `data-tauri-drag-region` 拖拽辅助、页面 flex 骨架。
- 删除：`main.css` 中的设计令牌 CSS 变量（`--primary` 等）与全局 `.btn*` 类（该职责移交 antd）。
- 各组件内的局部自绘样式：仅删除被 antd 取代的部分，保留布局/定位类样式。

## 6. 高危点与保护措施

### 6.1 ProjectList 拖拽排序

- 现状依赖原生 HTML5 drag-and-drop（`onGroupDragStart` / `onProjectDragStart`，通过 `closest('.group-header')` / `.list-item` 命中）。
- antd 无内置等价物，**必须保留原生拖拽**。
- 迁移只替换其中的按钮、输入框与重命名 `input`；`.group-header` / `.list-item` / drop-indicator 等拖拽命中结构与 `dragenter` preventDefault 逻辑保持原样。
- 替换后必须实测：分组拖拽、项目跨分组拖拽、拖拽放置均正常。

### 6.2 VpnRuleTable 行内编辑表格

- div-grid → `a-table`，行内编辑（input）、增行/删行/上下移动逻辑需逐项对照迁移，避免状态错位。
- 迁移后必须实测：规则的增删改、上下移动、保存/回填。

## 7. 验收标准

1. 13 个 `.vue` 中原生 `button/input/textarea/select/checkbox` 已全部替换为 antd 组件；弹窗统一为 `a-modal` / `Modal.confirm`。
2. 功能零回归：项目打包、VPN 规则增删改、配置保存、拖拽排序、进度与日志更新均与迁移前一致。
3. `pnpm build`（含 `vue-tsc --noEmit`）通过。
4. `pnpm exec tauri dev` 正常启动，界面可交互（遵循项目规则：改完重启验证）。

## 8. 风险

- **原生拖拽被破坏**：迁移中误改 DOM 结构导致拖拽失效——通过保持命中结构不变 + 实测规避。
- **命令式 API 未按需解析**：`Modal.confirm` / `message` 若未被 AutoImport 解析会报未定义——已在 3.2 配置 `AutoImport` + `AntDesignVueResolver` 覆盖。
- **类型声明文件缺失**：`components.d.ts` / `auto-imports.d.ts` 未生成或未纳入 tsconfig 会导致类型报错——构建前确认已生成。
- **样式回归**：删除 `main.css` 令牌后局部样式引用失效——逐组件检查残留引用。

## 9. 实施顺序（概览）

1. 依赖安装 + Vite/入口接入（3.1–3.4），先跑通空接入。
2. 由外及内替换：`App.vue`（顶栏、弹窗、徽标、提示条）→ 通用组件（`ProjectList` / `ProjectConfig` / `ExcludeRules` / `LogPanel` / `PackProgress`）→ VPN 组件 → `SettingsPanel`。
3. 每完成一层清理对应旧样式，构建 + 重启验证。
4. 最后做样式清理与整体回归验收。

具体任务拆分在实现计划中给出。
