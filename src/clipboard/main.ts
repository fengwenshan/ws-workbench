import { createApp } from 'vue'
import '@/assets/styles/main.css'
import ClipboardPanel from './ClipboardPanel.vue'

// 剪贴板历史是独立窗口（/clipboard.html），和快捷面板共用一套 CSS 变量
createApp(ClipboardPanel).mount('#clipboard-app')
