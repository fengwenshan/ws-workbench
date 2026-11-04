# ws-workbench 图标

程序坞图标与菜单栏图标的源文件、生成脚本和产物。

- 设计方案：`../../docs/superpowers/specs/2026-11-04-app-icon-design.md`
- 预览图：`dist/preview/ws-workbench-icon-preview.png`

## 目录

| 路径 | 说明 |
| --- | --- |
| `src/ws-workbench-icon.svg` | 程序坞图标矢量源（纯矢量，无投影） |
| `src/tray-white.svg` | 菜单栏白色版矢量源 |
| `src/tray-template.svg` | 菜单栏模板黑版矢量源（macOS） |
| `dist/ws-workbench-icon-1024.png` | 主图，`pnpm tauri icon` 的输入 |
| `dist/tray/` | 菜单栏位图（18 / 32 / 36 / 48 / 54） |
| `tools/build.py` | 生成主图与菜单栏位图 |
| `tools/preview.py` | 生成交付预览图 |
| `tools/rasterize.swift` | AppKit/CoreSVG 光栅化小工具（源码） |

## 重新生成

```bash
cd design/icons/tools
TMPDIR="$PWD/.tmp" CLANG_MODULE_CACHE_PATH="$PWD/.modulecache" \
  swiftc -O -module-cache-path ./.modulecache rasterize.swift -o rasterize

python3 build.py
python3 preview.py

cd ../../..
node_modules/.bin/tauri icon design/icons/dist/ws-workbench-icon-1024.png
cp design/icons/dist/tray/tray-template-36.png design/icons/dist/tray/tray-white-32.png \
   design/icons/dist/tray/tray-white-48.png src-tauri/icons/tray/
```

`build.py` 依赖 Pillow；`rasterize` 需要 macOS（AppKit / CoreSVG）。

## 开发模式的 Dock 图标

`pnpm tauri dev` 跑的是 `target/debug/` 下的裸二进制，没有 `.app` 包，macOS 会退回系统那个黑底「exec」占位图标——`tauri.conf.json` 里的 `icon.icns` 只在打包后才生效。

所以 `src-tauri/src/lib.rs` 里加了 `set_dock_icon()`：启动时用 `include_bytes!` 内嵌的 `icons/icon-source.png` 设一次 `NSApplication.applicationIconImage`（macOS 专属，依赖 `objc2` / `objc2-app-kit`）。dev 和安装包观感一致；正式包仍由 `.icns` 提供图标。
