#!/usr/bin/env python3
"""ws-workbench 图标生成器（自包含，无第三方依赖，Pillow 除外）。

图形用 SVG 描述，交给 AppKit 的 CoreSVG（tools/rasterize）光栅化；
瓦片与字形分层渲染，再用 Pillow 叠一层真正的软投影——SVG 里做不出高斯模糊。

用法：
  python3 build.py                 # 生成程序坞图标与菜单栏图标

产物：
  ../src/ws-workbench-icon.svg         程序坞图标（纯矢量，无投影）
  ../src/tray-white.svg                菜单栏白色版矢量源
  ../src/tray-template.svg             菜单栏模板黑矢量源（macOS 用）
  ../dist/ws-workbench-icon-1024.png   交给 `pnpm tauri icon` 的主图
  ../dist/ws-workbench-icon-dev-1024.png  开发版主图（右下角带 dev 角标）
  ../dist/tray/tray-*.png              菜单栏位图（18 / 32 / 36 / 48 / 54）
  ../dist/tray/tray-dev-template-36.png  开发版菜单栏位图（模板图 + 正中镂空点）
  ../src/tray-dev-template.svg           开发版菜单栏矢量源
  ../../src-tauri/icons/dev/           开发版运行时用的图标档位
"""
from __future__ import annotations

import math
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ICONS = Path(__file__).resolve().parents[1]
SRC = ICONS / "src"
DIST = ICONS / "dist"
TRAY = DIST / "tray"
TMP = ICONS / "tools" / ".tmp"
RASTER = Path(__file__).resolve().with_name("rasterize")
# 仓库里 src-tauri/icons：开发版那套直接写到运行时 include_bytes! 的位置
TAURI_ICONS = ICONS.parents[1] / "src-tauri" / "icons"
for _d in (SRC, DIST, TRAY):
    _d.mkdir(parents=True, exist_ok=True)

# ---------------------------------------------------------------- 设计参数
GRAD = [("0", "#7A4BE8"), ("0.42", "#2C63F0"), ("1", "#0FB3D8")]
ACCENT = "#2C63F0"      # W 暗纹
INK = "#061023"         # 投影 / 内阴影的墨色
INK_RGB = (6, 16, 35)   # 同一墨色的元组形式（Pillow 画布用）
CANVAS = 1024
TILE = 840              # 内容方块边长；1024 画布内 840 ≈ Apple 的 824 网格
CX = CY = 512

# 工具箱几何（1024 画布）
HANDLE = (356, 278, 312, 52, 26)                                # x, y, w, h, r
POSTS = [(372, 298, 34, 120, 10), (618, 298, 34, 120, 10)]
LID = (232, 388, 560, 92, 46)
BODY = [(272, 494), (752, 494), (722, 780), (302, 780)]         # 上宽下窄的箱体
BODY_R = 30
LATCH = (462, 436, 100, 82, 20)
W_PATH = "M 412 592 L 456 700 L 512 632 L 568 700 L 612 592"


# ---------------------------------------------------------------- SVG 工具
def squircle(cx: float, cy: float, half: float, n: float = 5.0, steps: int = 720):
    """超椭圆，近似 macOS 的连续曲率圆角。"""
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        pts.append((cx + half * math.copysign(abs(c) ** (2 / n), c),
                    cy + half * math.copysign(abs(s) ** (2 / n), s)))
    return pts


def path_d(pts) -> str:
    return "M " + " L ".join(f"{x:.2f} {y:.2f}" for x, y in pts) + " Z"


def rounded_polygon(points, r: float) -> str:
    """带圆角的闭合多边形（用二次贝塞尔近似圆角）。"""
    n = len(points)
    segs = []
    for i in range(n):
        p = points[i]
        a, b = points[(i - 1) % n], points[(i + 1) % n]
        v1 = (a[0] - p[0], a[1] - p[1])
        l1 = math.hypot(*v1)
        v2 = (b[0] - p[0], b[1] - p[1])
        l2 = math.hypot(*v2)
        rr = min(r, l1 / 2, l2 / 2)
        p1 = (p[0] + v1[0] / l1 * rr, p[1] + v1[1] / l1 * rr)
        p2 = (p[0] + v2[0] / l2 * rr, p[1] + v2[1] / l2 * rr)
        segs.append((p1, p, p2))
    d = f"M {segs[0][2][0]:.1f} {segs[0][2][1]:.1f} "
    for i in range(1, n):
        p1, p, p2 = segs[i]
        d += f"Q {p[0]:.1f} {p[1]:.1f} {p2[0]:.1f} {p2[1]:.1f} "
    p1, p, p2 = segs[0]
    return d + f"Q {p[0]:.1f} {p[1]:.1f} {p2[0]:.1f} {p2[1]:.1f} Z"


def svg(body: str, size: int = 1024) -> str:
    return (f'<svg width="{size}" height="{size}" viewBox="0 0 {size} {size}" '
            f'xmlns="http://www.w3.org/2000/svg">\n{body}\n</svg>\n')


TILE_D = path_d(squircle(CX, CY, TILE / 2))
BODY_D = rounded_polygon(BODY, BODY_R)

TRAY_GLYPH = """
<rect x="10.4" y="4.6" width="15.2" height="3.8" rx="1.9" fill="CURRENT"/>
<rect x="11.8" y="7.6" width="2.4" height="6.0" rx="0.9" fill="CURRENT"/>
<rect x="21.8" y="7.6" width="2.4" height="6.0" rx="0.9" fill="CURRENT"/>
<rect x="6.0" y="12.4" width="24.0" height="5.2" rx="2.6" fill="CURRENT"/>
<path d="M 7.2 19.0 H 28.8 L 27.7 29.2 Q 27.6 30.4 26.4 30.4 H 9.6 Q 8.4 30.4 8.3 29.2 Z" fill="CURRENT"/>
<rect x="15.8" y="16.2" width="4.4" height="4.2" rx="1.5" fill="CURRENT"/>
"""


# ---------------------------------------------------------------- 瓦片
def tile_svg() -> str:
    stops = "".join(f'<stop offset="{o}" stop-color="{c}"/>' for o, c in GRAD)
    return svg(f'''<defs>
  <linearGradient id="bg" x1="0.03" y1="0" x2="0.97" y2="1">{stops}</linearGradient>
  <radialGradient id="hi" cx="0.34" cy="-0.05" r="0.85">
    <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.28"/>
    <stop offset="0.42" stop-color="#FFFFFF" stop-opacity="0.06"/>
    <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/></radialGradient>
  <radialGradient id="dk" cx="0.66" cy="1.05" r="0.92">
    <stop offset="0" stop-color="{INK}" stop-opacity="0.18"/>
    <stop offset="0.5" stop-color="{INK}" stop-opacity="0.045"/>
    <stop offset="1" stop-color="{INK}" stop-opacity="0"/></radialGradient>
  <linearGradient id="rim" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.55"/>
    <stop offset="0.45" stop-color="#FFFFFF" stop-opacity="0.10"/>
    <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/></linearGradient>
  <clipPath id="clip"><path d="{TILE_D}"/></clipPath>
</defs>
<path d="{TILE_D}" fill="url(#bg)"/>
<g clip-path="url(#clip)">
  <path d="{TILE_D}" fill="url(#hi)"/>
  <path d="{TILE_D}" fill="url(#dk)"/>
  <path d="{TILE_D}" fill="none" stroke="url(#rim)" stroke-width="8"/>
</g>''')


# ---------------------------------------------------------------- 字形
def glyph_shapes(paint: str) -> str:
    hx, hy, hw, hh, hr = HANDLE
    parts = [f'<rect x="{hx}" y="{hy}" width="{hw}" height="{hh}" rx="{hr}" fill="{paint}"/>']
    for px, py, pw, ph, pr in POSTS:
        parts.append(f'<rect x="{px}" y="{py}" width="{pw}" height="{ph}" rx="{pr}" fill="{paint}"/>')
    lx, ly, lw, lh, lr = LID
    parts.append(f'<rect x="{lx}" y="{ly}" width="{lw}" height="{lh}" rx="{lr}" fill="{paint}"/>')
    parts.append(f'<path d="{BODY_D}" fill="{paint}"/>')
    lax, lay, law, lah, lar = LATCH
    parts.append(f'<rect x="{lax}" y="{lay}" width="{law}" height="{lah}" rx="{lar}" fill="{paint}"/>')
    return "\n".join(parts)


GLYPH_DEFS = f'''<defs>
  <linearGradient id="glyph" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#FFFFFF"/>
    <stop offset="1" stop-color="#E6EFFF"/></linearGradient>
  <linearGradient id="underLid" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="{INK}" stop-opacity="0.12"/>
    <stop offset="1" stop-color="{INK}" stop-opacity="0"/></linearGradient>
  <linearGradient id="bottomShade" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="{INK}" stop-opacity="0"/>
    <stop offset="1" stop-color="{INK}" stop-opacity="0.08"/></linearGradient>
  <linearGradient id="lipShade" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="{INK}" stop-opacity="0"/>
    <stop offset="1" stop-color="{INK}" stop-opacity="0.10"/></linearGradient>
</defs>'''


def glyph_svg(shaded: bool = True) -> str:
    parts = [glyph_shapes("url(#glyph)")]
    if shaded:
        lx, ly, lw, lh, lr = LID
        parts += [
            f'<clipPath id="bodyClip"><path d="{BODY_D}"/></clipPath>',
            f'<g clip-path="url(#bodyClip)">'
            f'<rect x="272" y="494" width="480" height="90" fill="url(#underLid)"/>'
            f'<rect x="302" y="700" width="420" height="80" fill="url(#bottomShade)"/></g>',
            f'<clipPath id="lidClip"><rect x="{lx}" y="{ly}" width="{lw}" height="{lh}" rx="{lr}"/></clipPath>',
            f'<g clip-path="url(#lidClip)">'
            f'<rect x="{lx}" y="{ly + lh - 22}" width="{lw}" height="22" fill="url(#lipShade)"/></g>',
        ]
    parts.append(f'<path d="{W_PATH}" fill="none" stroke="{ACCENT}" stroke-width="18" '
                 f'stroke-linecap="round" stroke-linejoin="round" opacity="0.16"/>')
    return "\n".join(parts)


# ---------------------------------------------------------------- 渲染
def render(svg_text: str, out_svg: Path, out_png: Path, size: int) -> Image.Image:
    out_svg.write_text(svg_text, encoding="utf-8")
    subprocess.run([str(RASTER), str(out_svg), str(out_png), str(size)], check=True)
    return Image.open(out_png).convert("RGBA")


def build_app_icon() -> None:
    tile = render(tile_svg(), TMP / "tile.svg", TMP / "tile.png", CANVAS)
    white = render(svg(GLYPH_DEFS + glyph_svg(shaded=True)),
                   SRC / "ws-workbench-icon.svg", TMP / "glyph-white.png", CANVAS)
    dark = render(svg(glyph_shapes(INK)),
                  TMP / "glyph-dark.svg", TMP / "glyph-dark.png", CANVAS)

    shadow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    shadow.alpha_composite(dark, (0, 16))
    shadow = shadow.filter(ImageFilter.GaussianBlur(24))
    shadow.putalpha(shadow.split()[3].point(lambda a: int(a * 0.42)))

    out = tile.copy()
    out.alpha_composite(shadow)
    out.alpha_composite(white)
    out.save(DIST / "ws-workbench-icon-1024.png")


def build_tray() -> None:
    white_svg = svg(TRAY_GLYPH.replace("CURRENT", "#FFFFFF"), size=36)
    black_svg = svg(TRAY_GLYPH.replace("CURRENT", "#000000"), size=36)
    (SRC / "tray-white.svg").write_text(white_svg, encoding="utf-8")
    (SRC / "tray-template.svg").write_text(black_svg, encoding="utf-8")
    for tone, text in (("white", white_svg), ("template", black_svg)):
        for size in (18, 32, 36, 48, 54):
            render(text, TMP / f"tray-{tone}-{size}.svg",
                   TRAY / f"tray-{tone}-{size}.png", size)


# ---------------------------------------------------------------- 开发版标识
# 开发版图标 = 正式图标 + 右下角一枚白底蓝字的 dev 小标签。
# 位置按 1024 画布给：右/下边缘落在 squircle 圆弧的安全区里（瓦片边 932 再内缩 72），
# 上边缘正好压在箱体下沿之下，不挡工具箱字形。
DEV_BADGE = {"right": 860, "bottom": 884, "w": 220, "h": 100, "radius": 30}
DEV_FILL = (255, 255, 255, 242)   # 白底压一点透明度，让底下的渐变透出来
DEV_TEXT = "#2C63F0"              # 和界面主色同色
DEV_TEXT_SIZE = 56
DEV_FONT = "/System/Library/Fonts/HelveticaNeue.ttc"   # index=1 是 Bold
# 开发版菜单栏图标 = 发布版同一张模板图 + 正中间镂空一个透明小圆点。
# 模板图由系统按菜单栏明暗染色（深色栏白、浅色栏黑），所以开发版不能换颜色；
# 标识做成「挖掉一个点」——镂空处直接透出菜单栏背景，不引入新颜色，深浅栏都成立。
# 圆心按 36 画布给：(18, 18) 正好是箱体锁扣、挖掉会显得缺一块，所以落在箱体的
# 视觉中心 (18, 24.4)；直径 8（18pt 下约 4pt），画布不额外加宽。
TRAY_DEV_HOLE = (18, 24.4, 4.0)
# 运行时用到的档位：512 给 macOS Dock（128pt @2x 只要 256，512 够用），
# 32 / 64 / 128 / 256 给 Windows 任务栏与托盘。
DEV_RUNTIME_SIZES = {
    "icon-dev.png": 512,
    "128x128@2x-dev.png": 256,
    "128x128-dev.png": 128,
    "64x64-dev.png": 64,
    "32x32-dev.png": 32,
}


def dev_badge_layer(canvas: int) -> Image.Image:
    """右下角的 dev 圆角标签，返回同尺寸透明图层。"""
    right, bottom, w, h, radius = (DEV_BADGE[k] for k in ("right", "bottom", "w", "h", "radius"))
    x0, y0 = right - w, bottom - h
    layer = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    draw.rounded_rectangle((x0, y0, right, bottom), radius=radius, fill=DEV_FILL)
    font = ImageFont.truetype(DEV_FONT, DEV_TEXT_SIZE, index=1)
    bbox = draw.textbbox((0, 0), "dev", font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    # 按字形包围盒视觉居中，再上移 2px 抵消小写字母的重心偏下
    draw.text(((x0 + right - tw) / 2 - bbox[0], (y0 + bottom - th) / 2 - bbox[1] - 2),
              "dev", font=font, fill=DEV_TEXT)
    return layer


def with_dev_badge(icon: Image.Image) -> Image.Image:
    """给主图压上 dev 角标（角标自带一层软投影，免得白底糊在渐变上）。"""
    layer = dev_badge_layer(icon.width)
    alpha = layer.split()[3].filter(ImageFilter.GaussianBlur(10))
    shadow = Image.new("RGBA", icon.size, INK_RGB + (0,))
    shadow.putalpha(alpha.point(lambda a: int(a * 0.38)))
    out = icon.copy()
    out.alpha_composite(shadow, (0, 8))
    out.alpha_composite(layer)
    return out


def build_dev_icon() -> None:
    master = Image.open(DIST / "ws-workbench-icon-1024.png").convert("RGBA")
    dev = with_dev_badge(master)
    dev.save(DIST / "ws-workbench-icon-dev-1024.png")
    out_dir = TAURI_ICONS / "dev"
    out_dir.mkdir(parents=True, exist_ok=True)
    for name, size in DEV_RUNTIME_SIZES.items():
        dev.resize((size, size), Image.LANCZOS).save(out_dir / name)
    # Windows 开发构建的 exe 图标：`build.rs` 在 debug + windows 时把它传给
    # `WindowsAttributes::window_icon_path`，资源管理器里的文件图标 / 桌面快捷方式 /
    # 任务栏固定项都读 exe 里嵌的这枚图标（运行时 set_icon 改不到文件图标）。
    dev.save(out_dir / "icon-dev.ico",
             sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])


def build_tray_dev() -> None:
    """开发版菜单栏图标：发布版同款模板图，正中间镂空一个透明小圆点。

    保持模板图（纯黑 + alpha）而不是彩色图：系统会按菜单栏明暗给整张图染色，
    这样开发版的颜色和发布版完全一样；开发标识只靠镂空一个点——透明处透出
    菜单栏背景，深浅色下都成立，也不占宽度。
    """
    cx, cy, r = TRAY_DEV_HOLE
    body = f'''<defs>
  <mask id="devHole">
    <rect x="0" y="0" width="36" height="36" fill="#FFFFFF"/>
    <circle cx="{cx}" cy="{cy}" r="{r}" fill="#000000"/>
  </mask>
</defs>
<g mask="url(#devHole)">{TRAY_GLYPH.replace("CURRENT", "#000000")}</g>'''
    tray_dev_svg = svg(body, size=36)
    render(tray_dev_svg, SRC / "tray-dev-template.svg", TRAY / "tray-dev-template-36.png", 36)
    tray_dir = TAURI_ICONS / "tray"
    tray_dir.mkdir(parents=True, exist_ok=True)
    (tray_dir / "tray-dev-template-36.png").write_bytes(
        (TRAY / "tray-dev-template-36.png").read_bytes())


if __name__ == "__main__":
    build_app_icon()
    build_tray()
    build_dev_icon()
    build_tray_dev()
    print("built:", DIST / "ws-workbench-icon-1024.png")
    print("dev:  ", DIST / "ws-workbench-icon-dev-1024.png")
