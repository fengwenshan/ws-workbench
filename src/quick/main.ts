import { createApp } from 'vue'
import '@/assets/styles/main.css'
import QuickPanel from './QuickPanel.vue'

// 托盘快捷面板是独立窗口（/quick.html），和主窗口共用一套 CSS 变量与组件约定
createApp(QuickPanel).mount('#quick-app')
