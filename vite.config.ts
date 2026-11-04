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
          importStyle: false,
          resolveIcons: true,
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
    alias: { '@': path.resolve(__dirname, './src') }
  },
  clearScreen: false,
  server: {
    port: 8790,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
})
