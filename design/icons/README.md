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
| `dist/ws-workbench-icon-dev-1024.png` | 开发版主图（右下角压 `dev` 角标） |
| `dist/tray/` | 菜单栏位图（18 / 32 / 36 / 48 / 54）；`tray-dev-template-36.png` 是开发版（发布版模板图 + 正中镂空点） |
| `../../src-tauri/icons/dev/` | 开发版运行时图标档位（512 / 256 / 128 / 64 / 32）与 Windows exe 图标 `icon-dev.ico`（`build.py` 直接写入） |
| `../../src-tauri/icons/tray/tray-dev-template-36.png` | 开发版托盘位图（`build.py` 直接写入） |
| `tools/build.py` | 生成主图、菜单栏位图与开发版整套资产 |
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

`build.py` 依赖 Pillow；`rasterize` 需要 macOS（AppKit / CoreSVG）。开发版资产（`dist/ws-workbench-icon-dev-1024.png`、`dist/tray/tray-dev-template-36.png`、`src-tauri/icons/dev/`、`src-tauri/icons/tray/tray-dev-template-36.png`）也由 `build.py` 一并写出，不需要再手工拷。

## 开发模式的 Dock 图标

`pnpm tauri dev` 跑的是 `target/debug/` 下的裸二进制，没有 `.app` 包，macOS 会退回系统那个黑底「exec」占位图标——`tauri.conf.json` 里的 `icon.icns` 只在打包后才生效。

所以 `src-tauri/src/lib.rs` 里加了 `set_dock_icon()`：启动时用 `include_bytes!` 内嵌的图标设一次 `NSApplication.applicationIconImage`（macOS 专属，依赖 `objc2` / `objc2-app-kit`）。开发版（`debug_assertions`）用的是 `icons/dev/icon-dev.png`——正式图标 + 右下角白底蓝字的 `dev` 小标签；发布版仍用 `icons/icon-source.png`，正式包由 `.icns` 提供图标。

macOS 菜单栏托盘图标同理：开发版沿用发布版**同一张模板图**（系统按菜单栏明暗染色，颜色和发布版完全一样），只在箱体正中镂空一个透明小圆点——镂空处直接透出菜单栏背景，不引入任何新颜色、也不占宽度。之前试过的两种方案都已弃用：「工具箱 + `dev` 字样」加宽版太占位置；「彩色渐变字形 + 黄点」会改掉图标原色（模板图整张会被系统染色，画不出黄点）。托盘右键菜单在开发版最上面多一行禁用的 `ws-workbench-dev（开发版）`。

Windows 开发构建还会把 `icons/dev/icon-dev.ico` 嵌进 exe：`src-tauri/build.rs` 在 debug + windows 时把它交给 `WindowsAttributes::window_icon_path`，这样资源管理器里的文件图标、桌面快捷方式、任务栏固定项也都是带角标的（运行时 `set_icon` 只改窗口图标，改不到 exe 文件图标）。
