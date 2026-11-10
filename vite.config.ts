import { defineConfig, type Plugin } from 'vite'
import vue from '@vitejs/plugin-vue'
import Components from 'unplugin-vue-components/vite'
import AutoImport from 'unplugin-auto-import/vite'
import { AntDesignVueResolver } from 'unplugin-vue-components/resolvers'
import fs from 'node:fs'
import path from 'node:path'

/**
 * Vditor 的运行时资源（lute / highlight.js / i18n / 图标 / 内容主题）默认从 CDN 拉，
 * 桌面应用必须离线可用。这里把 node_modules/vditor/dist 里用得到的子目录：
 * - dev：用中间件从 node_modules 直出 `/vditor/dist/*`
 * - build：拷进 `dist/vditor/dist/*`，随前端产物一起进安装包
 * 编辑器侧只需要配置 `cdn: '/vditor'`。
 */
const VDITOR_CDN_PREFIX = '/vditor/dist/'
const VDITOR_ASSET_DIRS = [
  'js/lute',
  'js/highlight.js',
  'js/i18n',
  'js/icons',
  'css/content-theme',
]

const CONTENT_TYPES: Record<string, string> = {
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.json': 'application/json; charset=utf-8',
}

function vditorOfflineAssets(): Plugin {
  const vditorDist = path.resolve(__dirname, 'node_modules/vditor/dist')
  const outDir = path.resolve(__dirname, 'dist/vditor/dist')

  return {
    name: 'vditor-offline-assets',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const url = (req.url ?? '').split('?')[0]
        if (!url.startsWith(VDITOR_CDN_PREFIX)) {
          next()
          return
        }
        const filePath = path.join(vditorDist, url.slice(VDITOR_CDN_PREFIX.length))
        // 防止 ../ 穿越到 vditor/dist 之外
        if (!filePath.startsWith(vditorDist + path.sep) && filePath !== vditorDist) {
          res.statusCode = 403
          res.end('forbidden')
          return
        }
        fs.readFile(filePath, (err, data) => {
          if (err) {
            res.statusCode = 404
            res.end('not found')
            return
          }
          res.setHeader(
            'Content-Type',
            CONTENT_TYPES[path.extname(filePath).toLowerCase()] ??
              'application/octet-stream',
          )
          res.setHeader('Cache-Control', 'no-cache')
          res.end(data)
        })
      })
    },
    closeBundle() {
      for (const dir of VDITOR_ASSET_DIRS) {
        const from = path.join(vditorDist, dir)
        if (!fs.existsSync(from)) continue
        fs.mkdirSync(outDir, { recursive: true })
        fs.cpSync(from, path.join(outDir, dir), { recursive: true })
      }
    },
  }
}

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
    vditorOfflineAssets(),
  ],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') }
  },
  build: {
    rollupOptions: {
      // 四个页面入口：主窗口 index.html + 托盘快捷面板 quick.html
      // + 剪贴板历史 clipboard.html + 独立卸载窗口 uninstall.html
      input: {
        main: path.resolve(__dirname, 'index.html'),
        quick: path.resolve(__dirname, 'quick.html'),
        clipboard: path.resolve(__dirname, 'clipboard.html'),
        uninstall: path.resolve(__dirname, 'uninstall.html'),
      },
    },
  },
  clearScreen: false,
  server: {
    port: 8790,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
})
