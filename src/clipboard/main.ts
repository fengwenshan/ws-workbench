import { createApp } from 'vue'
import { installBrowserTauriShim } from '@/dev/browserTauriShim'
import '@/assets/styles/main.css'
import ClipboardPanel from './ClipboardPanel.vue'
import { initTheme } from '@/composables/useTheme'

// 浏览器里 `pnpm dev` 预览界面时补一份假 Tauri 后端；桌面应用里是空操作
if (import.meta.env.DEV) installBrowserTauriShim()

// 剪贴板历史是独立窗口（/clipboard.html），和快捷面板共用一套 CSS 变量与主题
void initTheme()

createApp(ClipboardPanel).mount('#clipboard-app')
