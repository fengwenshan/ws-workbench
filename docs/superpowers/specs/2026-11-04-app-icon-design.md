# 应用图标与托盘图标 设计文档

- 日期：2026-11-04
- 状态：已定稿（方案 B · 工具箱）
- 关联：`design/icons/`、`src-tauri/icons/`、`src-tauri/src/lib.rs`、`src-tauri/Cargo.toml`

## 1. 背景与目标

`ws-workbench` 是一个常驻托盘的桌面工作台：项目分组与打包、Markdown 笔记与加密附件、代理分流、运行环境自检都收在一个窗口里，关掉窗口也不退出。原来 `src-tauri/icons/` 里是一套通用的蓝色下载占位图标，托盘则直接复用窗口小图标，在 macOS 深色菜单栏上几乎不可辨识。

本次交付两枚图标：

1. **程序坞图标**：macOS Big Sur 风格的圆角方形（squircle），放进 Dock / 访达 / 关于窗口 / 安装包。
2. **菜单栏图标**：18pt 单色小图标，白色 + 透明；macOS 用模板图由系统自动反色。

### 目标（In Scope）

- 一套 1024 主图，派生 macOS / Windows / Android / iOS 全套尺寸与 `icon.icns`、`icon.ico`。
- 菜单栏图标：白色版（Windows / Linux）+ 模板黑版（macOS），含 18 / 32 / 36 / 48 / 54px 位图。
- 托盘代码改用专用图标，macOS 走 `icon_as_template(true)`。
- 可重复生成的脚本、预览图与本文档。

### 非目标（Out of Scope）

- 不改前端界面与品牌色（界面主色仍是 `#3b82f6`）。
- 不做启动图、安装器背景、官网等其它物料。
- 不改托盘菜单结构与窗口行为。

## 2. 隐喻与方向

产品文案是「把交付日常收进一个窗口」，托盘常驻、随开随用。图标取 **工具箱**：盖子、提手、锁扣、箱体，一只装满工具、随时可拎走的箱子，对应「打包 / 交付 / 随手可用」。

候选方向曾比较过三种：W 字标、工具箱、叠放面板；评审后选定工具箱。

## 3. 视觉规范

### 3.1 程序坞图标（1024 × 1024）

| 项目 | 值 |
| --- | --- |
| 画布 | 1024 × 1024，透明背景 |
| 内容区 | 840 × 840 居中（同 Apple 824 网格的安全边距） |
| 外形 | 超椭圆 `n = 5.0`，近似 macOS 连续曲率圆角 |
| 渐变 | 135°，`#7A4BE8` → `#2C63F0`（42%）→ `#0FB3D8` |
| 高光 | 顶部偏左白色径向渐变 28% |
| 暗部 | 右下角墨色径向渐变 18% |
| 描边 | 内圈 8px 白色渐变，压边提亮 |
| 字形 | 工具箱：提手横杆 + 两根立柱 + 盖（比箱体宽 40px）+ 上宽下窄箱体 + 中央锁扣 + W 暗纹 |
| 字形色 | 白色渐变 `#FFFFFF → #E6EFFF` |
| 内阴影 | 盖下 12%、箱底 8%、盖沿 10% |
| 投影 | 墨色 `#061023`，偏移 (0,16)，高斯 24，整体 42% |

W 暗纹是唯一品牌记号：`#2C63F0`、18px 圆头描边、16% 不透明度，128px 以上可辨认，32px 以下自然消失。

### 3.2 菜单栏图标（18pt）

- 结构同构但做极简：提手 + 盖 + 梯形箱体 + 锁扣，去掉暗纹与内阴影。
- 画布 36 × 36（2x），内容约 26px 高；`tray-icon` 会把 NSImage 高度固定为 18pt。
- **模板图**：纯黑 + alpha（`tray-template-36.png`），macOS 深色菜单栏自动反白、浅色自动转黑。
- **白色版**：纯白 + alpha（`tray-white-32.png` / `tray-white-48.png`），给 Windows 深色任务栏与 Linux 面板。
- 额外提供 18 / 54px 两档，方便其它平台或高倍屏取用。

## 4. 资产清单

```text
design/icons/
├─ src/                         可编辑矢量源
│  ├─ ws-workbench-icon.svg     程序坞图标（纯矢量，无投影）
│  ├─ tray-white.svg            菜单栏白色版
│  └─ tray-template.svg         菜单栏模板黑版
├─ dist/
│  ├─ ws-workbench-icon-1024.png  主图（已叠软投影），`pnpm tauri icon` 的输入
│  ├─ tray/tray-{white,template}-{18,32,36,48,54}.png
│  └─ preview/ws-workbench-icon-preview.png
└─ tools/
   ├─ build.py                  生成主图与菜单栏位图
   ├─ preview.py                生成交付预览图
   ├─ rasterize.swift           AppKit/CoreSVG 光栅化小工具（源码）
   └─ rasterize                 编译产物（不入库）

src-tauri/icons/                由 `pnpm tauri icon` 生成
├─ icon.icns / icon.ico / icon.png / 32x32.png / 128x128.png ...
└─ tray/                        托盘位图（随包编译进二进制）
   ├─ tray-template-36.png
   ├─ tray-white-32.png
   └─ tray-white-48.png
```

## 5. 集成说明

### 5.1 托盘图标（`src-tauri/src/lib.rs`）

```rust
fn tray_icon_image() -> Option<tauri::image::Image<'static>> {
    let bytes: &'static [u8] = if cfg!(target_os = "macos") {
        include_bytes!("../icons/tray/tray-template-36.png")
    } else if cfg!(target_os = "windows") {
        include_bytes!("../icons/tray/tray-white-32.png")
    } else {
        include_bytes!("../icons/tray/tray-white-48.png")
    };
    tauri::image::Image::from_bytes(bytes).ok()
}
```

- `TrayIconBuilder::icon(...)` 使用上面的位图；macOS 额外 `.icon_as_template(true)`，菜单栏深浅色交给系统着色。
- `tauri` 依赖新增 `image-png` feature（`Image::from_bytes` 解码 PNG 需要）。
- 位图走 `include_bytes!` 编进二进制，不需要在 `bundle.resources` 里额外声明。

### 5.2 应用图标

`src-tauri/tauri.conf.json` 的 `bundle.icon` 已引用 `icons/32x32.png`、`icons/128x128.png`、`icons/128x128@2x.png`、`icons/icon.icns`、`icons/icon.ico`，重新生成同名文件即可，无需改配置。

## 6. 构建方式

```bash
# 一次性：编译 CoreSVG 光栅化工具（macOS）
cd design/icons/tools
TMPDIR="$PWD/.tmp" CLANG_MODULE_CACHE_PATH="$PWD/.modulecache" \
  swiftc -O -module-cache-path ./.modulecache rasterize.swift -o rasterize

# 生成主图 + 菜单栏位图
python3 build.py
python3 preview.py

# 生成全平台图标集（写入 src-tauri/icons/）
cd ../../..
pnpm tauri icon design/icons/dist/ws-workbench-icon-1024.png

# 同步托盘位图
cp design/icons/dist/tray/tray-template-36.png design/icons/dist/tray/tray-white-32.png \
   design/icons/dist/tray/tray-white-48.png src-tauri/icons/tray/
```

## 7. 验证

- `cargo check --manifest-path src-tauri/Cargo.toml` 通过（含 `image-png` 新特性）。
- 校验尺寸：`icon.png` 512、`32x32.png` 32、`128x128@2x.png` 256；`icon.icns` 为 `is32` 类型、`icon.ico` 含 6 档 PNG（16–256）。
- 菜单栏位图 36px 白色版与模板黑版 alpha 正常，模板图 RGB 为纯黑。
- 预览图见 `design/icons/dist/preview/ws-workbench-icon-preview.png`（大图、尺寸阶梯、菜单栏深浅色实景、Dock 并排）。

## 8. 已知取舍

- 程序坞图标的字形投影在高斯模糊阶段完成，纯 SVG 源里没有投影，方便二次编辑。
- 工具箱正面视角在超大尺寸下略带「手提箱」感；128px 以下 Dock 尺寸与其它应用并排时辨识良好（见预览图）。若后续想更强工具感，可在箱体加两道冲压筋，但会牺牲小尺寸干净度。
- Linux 托盘没有模板图机制，白色图标在浅色面板上对比度有限，必要时可换深色版或按主题切换。
