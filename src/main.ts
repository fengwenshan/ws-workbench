import { createApp } from 'vue'
import App from './App.vue'
import 'ant-design-vue/dist/reset.css'
import './assets/styles/main.css'

/**
 * 把文件拖进窗口时，WKWebView 的默认动作是直接导航到 file:// 显示那个文件，
 * 整个应用界面就被一张图片顶替了（现场坏掉就是这么来的）。
 *
 * 页面这一层先把默认动作全部取消；不 stopPropagation，上传区自己的 drop 处理照常收到事件。
 * 原生层还有一道硬拦（src-tauri/src/lib.rs 的 on_navigation 拒绝 file:// 导航），
 * 这里只是第一道防线，免得先发起一次没必要的导航。
 */
function isFileDrag(event: DragEvent): boolean {
  const transfer = event.dataTransfer
  if (!transfer) return false
  if (Array.from(transfer.types ?? []).includes('Files')) return true
  // 某些 WebKit 版本在 dragover 阶段 types 不全，兜底看 files
  return (transfer.files?.length ?? 0) > 0
}

function blockFileDropDefault(event: DragEvent) {
  if (isFileDrag(event)) event.preventDefault()
}

// 捕获阶段拦，避免某个子元素 stopPropagation 之后 WebView 仍然执行默认导航
for (const type of ['dragenter', 'dragover', 'drop'] as const) {
  window.addEventListener(type, blockFileDropDefault, true)
}

createApp(App).mount('#app')
