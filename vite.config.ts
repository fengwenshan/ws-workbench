import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import path from 'path'

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') }
  },
  clearScreen: false,
  server: {
    port: 8790,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
})
