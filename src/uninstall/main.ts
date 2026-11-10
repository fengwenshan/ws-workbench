import { createApp } from 'vue'
import './styles.css'
import UninstallWindow from './UninstallWindow.vue'
import { platform } from '@/platform'

/**
 * 独立卸载窗口（`uninstall.html`，由后端 `cleaner_open_uninstall_window` 创建）。
 *
 * 页面不依赖 antd，也不引入主窗口的全局样式：这是一个 820×560 的小窗口，
 * 视觉按设计稿自己画。macOS 用 hiddenTitle + 红绿灯浮层，需要给内容让出顶部
 * 36px 安全区，用 platform 同步兜底值（userAgent）加 body class，浏览器预览同样生效。
 */
const isMac = platform.is_macos
document.body.classList.toggle('is-macos', isMac)
document.body.classList.toggle('is-windows', platform.is_windows)
document.body.classList.toggle('is-linux', platform.is_linux)

createApp(UninstallWindow).mount('#uninstall-app')
