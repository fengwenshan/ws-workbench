import { createApp } from 'vue'
import { installBrowserTauriShim } from '@/dev/browserTauriShim'
import '@/assets/styles/main.css'
import QuickPanel from './QuickPanel.vue'
import { applyDocumentTitle } from '@/api/app'
import { initTheme } from '@/composables/useTheme'

// 浏览器里 `pnpm dev` 预览界面时补一份假 Tauri 后端；桌面应用里是空操作
if (import.meta.env.DEV) installBrowserTauriShim()

// 托盘快捷面板是独立窗口（/quick.html），和主窗口共用一套 CSS 变量与组件约定
void initTheme()

createApp(QuickPanel).mount('#quick-app')

// 快捷面板也是独立窗口，标题同样按构建名（开发版 ws-workbench-dev）
void applyDocumentTitle()
