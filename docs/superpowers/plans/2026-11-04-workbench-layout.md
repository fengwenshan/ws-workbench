# 多工具工作台布局 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `App.vue` 的「项目树 + 整页子页」布局改成图标栏 + 可调项目树 + 工作区 + 底部日志面板的三栏工作台。

**Architecture:** 布局状态与拖拽/快捷键逻辑放在 `App.vue` 脚本；模板重组为 titlebar / rail / tree / workspace 四块；日志从独立视图改为 `v-show` 的底部面板；树宽、树折叠、日志高度、日志开合通过 `safeStorage` 持久化。

**Tech Stack:** Vue 3 `<script setup>`、CSS（scoped）、ant-design-vue 现有组件、Tauri 事件（托盘）。

---

## 文件结构

- Modify `src/App.vue`：脚本新增布局状态、快捷键、拖拽处理；替换模板与布局样式。
- 不改 `src/components/**`、`src/composables/**`（仅引用 `safeStorage`）。

## Task 1: 布局状态、持久化与快捷键

**Files:** `src/App.vue`

- [ ] 引入 `safeGet` / `safeSet`，新增常量与状态：
  ```ts
  const TREE_WIDTH_KEY = "ws-workbench:layout:tree-width";
  const TREE_COLLAPSED_KEY = "ws-workbench:layout:tree-collapsed";
  const LOG_HEIGHT_KEY = "ws-workbench:layout:log-height";
  const LOG_OPEN_KEY = "ws-workbench:layout:log-open";
  const TREE_WIDTH_DEFAULT = 260;
  const TREE_WIDTH_MIN = 200;
  const TREE_WIDTH_MAX = 420;
  const LOG_HEIGHT_DEFAULT = 220;
  const LOG_HEIGHT_MIN = 120;

  const treeWidth = ref(TREE_WIDTH_DEFAULT);
  const treeCollapsed = ref(false);
  const logHeight = ref(LOG_HEIGHT_DEFAULT);
  const logOpen = ref(false);
  ```
- [ ] `onMounted` 用 `safeGet` 恢复四项状态，并 clamp 到合法范围；`onUnmounted` 无需写回。
- [ ] 写回函数：`persistTreeWidth()`、`persistTreeCollapsed()`、`persistLogHeight()`、`persistLogOpen()`，调用 `void safeSet(key, String(value))`。
- [ ] 快捷键监听：`keydown` 处理 `metaKey/ctrlKey` + `1/2/3/B/J`，跳过输入框与 contenteditable；`onMounted` 注册、`onUnmounted` 移除。
- [ ] 验证：`pnpm build` 通过；刷新后状态恢复。

## Task 2: 模板重组

**Files:** `src/App.vue`

- [ ] 删除 `subpageTitle` 与 `SUBPAGE_TITLES`；视图类型改为 `"home" | "proxy" | "settings"`，日志不再是 view。
- [ ] 托盘映射：`nav-proxy` / `nav-settings` 切换 view；`nav-logs` 打开日志面板并保持当前 view。
- [ ] 新模板骨架：
  ```vue
  <div class="app-layout">
    <header class="titlebar" :class="{ 'titlebar-mac': isMac }" data-tauri-drag-region>
      <div class="titlebar-brand" data-tauri-drag-region>
        <span class="app-name">ws-workbench</span>
        <span class="tool-name">{{ toolTitle }}</span>
      </div>
      <div class="titlebar-actions">
        <span v-if="appVersion" class="version-text">v{{ appVersion }}</span>
        <button class="update-btn" :class="{ 'update-btn-primary': ... }" :disabled="updateBusy" @click="handleUpdateAction">
          {{ updateButtonLabel }}
        </button>
      </div>
    </header>

    <div class="workbench">
      <nav class="rail" aria-label="工具">
        <div class="rail-group">
          <button v-for="tool in tools" :key="tool.id" class="rail-btn" :class="{ active: isToolActive(tool.id) }" :title="tool.title" @click="selectTool(tool.id)">
            <component :is="tool.icon" />
          </button>
        </div>
        <div class="rail-bottom">
          <button class="rail-btn" :class="{ active: logOpen }" title="日志 (⌘J)" @click="toggleLogs()">…</button>
          <button class="rail-btn rail-vpn" :title="vpnTitle" @click="selectTool('proxy')">
            <span class="rail-vpn-dot" :class="vpnDotClass"></span>
          </button>
        </div>
      </nav>

      <aside v-show="view === 'home' && !treeCollapsed" class="tree-panel" :style="{ width: treeWidth + 'px' }">
        <ProjectList ... />
        <button class="tree-collapse" title="折叠项目树 (⌘B)" @click="setTreeCollapsed(true)">‹</button>
      </aside>
      <div v-show="view === 'home' && !treeCollapsed" class="resizer resizer-tree" @pointerdown="startTreeResize" @dblclick="resetTreeWidth"></div>

      <main class="workspace">
        <div v-if="hasUpdatePrompt" class="update-banner">…</div>
        <div v-show="view === 'home'" class="workspace-body home-body">
          <div class="config-section">GroupDetail / ProjectDetail / no-selection</div>
          <PackProgress … />
        </div>
        <div v-show="view === 'proxy'" class="workspace-body"><VpnPanel :vpn="vpn" :active="view === 'proxy'" /></div>
        <div v-show="view === 'settings'" class="workspace-body"><SettingsPanel … /></div>

        <div v-show="logOpen" class="resizer resizer-log" @pointerdown="startLogResize" @dblclick="resetLogHeight"></div>
        <section v-show="logOpen" class="log-panel" :style="{ height: logHeight + 'px' }">
          <div class="log-header">
            <div class="log-tabs">应用 / VPN</div>
            <div class="log-actions"><button @click="clearActiveLogs">清空</button><button @click="toggleLogs(false)">×</button></div>
          </div>
          <LogPanel :logs="activeLogs" @clear="clearActiveLogs" />
        </section>
      </main>
    </div>

    <!-- 新建分组、添加项目、删除确认、更新提示等弹窗保持原样 -->
  </div>
  ```

- [ ] 工具定义用内联 SVG（folder / shield / gear / panel）代替 emoji；图标统一 `stroke-width: 1.8`、`fill: none`、`stroke: currentColor`。
- [ ] 删除旧的 `.subpage`、返回按钮、日志页切换块、侧栏 `window-drag-box` 与更新栏。
- [ ] 验证：`pnpm build` 通过，三个工具可切换。

## Task 3: 拖拽、折叠与响应式样式

**Files:** `src/App.vue`

- [ ] 拖拽处理：
  ```ts
  let drag: { kind: "tree"; startX: number; startWidth: number } | { kind: "log"; startY: number; startHeight: number } | null = null;

  function startTreeResize(event: PointerEvent) {
    drag = { kind: "tree", startX: event.clientX, startWidth: treeWidth.value };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }
  function startLogResize(event: PointerEvent) {
    drag = { kind: "log", startY: event.clientY, startHeight: logHeight.value };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }
  function onResizeMove(event: PointerEvent) {
    if (!drag) return;
    if (drag.kind === "tree") {
      treeWidth.value = clamp(drag.startWidth + event.clientX - drag.startX, TREE_WIDTH_MIN, TREE_WIDTH_MAX);
    } else {
      const max = Math.round(window.innerHeight * 0.5);
      logHeight.value = clamp(drag.startHeight + drag.startY - event.clientY, LOG_HEIGHT_MIN, max);
    }
  }
  function onResizeEnd() {
    if (!drag) return;
    if (drag.kind === "tree") persistTreeWidth();
    else persistLogHeight();
    drag = null;
  }
  ```
  `pointermove` / `pointerup` 挂 window，`onMounted` 注册、`onUnmounted` 移除。
- [ ] 折叠：`setTreeCollapsed(v)` 写状态并持久化；`selectTool('home')` 在非项目页时展开树，在项目页时切换折叠。
- [ ] 样式：`.rail` 56px、`.rail-btn` 40×40、active 左侧 3px 高亮条、focus-visible 外环；`.tree-panel` 带 `border-right`；`.resizer` 5px `col-resize` / `row-resize`；`.log-panel` 顶部边框与 44px 头部；`@media (max-width: 900px)` 隐藏树与树分隔条；`@media (max-width: 720px)` 日志面板 `position:absolute; left:0; right:0; bottom:0` 加阴影。
- [ ] 保留弹窗、表单、`no-selection`、`update-banner` 样式；标题栏更新按钮复用 `.update-btn`。
- [ ] 验证：拖拽/双击/折叠表现正确，`pnpm build` 通过。

## Task 4: 收尾与验收

- [ ] `pnpm build` 通过。
- [ ] `cd src-tauri && cargo test --lib` 与改动前一致（266 passed / 5 个既有环境失败）。
- [ ] 手动验收：三工具切换、树与日志状态持久化、⌘1/2/3/B/J、托盘入口、窄窗口行为、macOS 标题栏可拖动。
- [ ] 提交：`feat(ui): 多工具三栏工作台布局`。

## 验收标准

对应 spec 第 5 节 7 条；实现后逐条手动确认。
