"""生成安装/卸载程序的品牌图（NSIS MUI2）。

- header.bmp   150x57  ：安装/卸载页面顶部的横条，白底 + 图标 + 产品名
- sidebar.bmp  164x314 ：安装向导欢迎/完成页左侧竖图，品牌蓝渐变 + 图标

用法：python scripts/nsis-brand-images.py
输出到 src-tauri/nsis/。
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ICON = ROOT / "src-tauri" / "icons" / "icon.png"
OUT_DIR = ROOT / "src-tauri" / "nsis"

BRAND = (59, 130, 246)
BRAND_DARK = (37, 99, 235)
INK = (31, 41, 55)
MUTED = (107, 114, 128)

PRODUCT = "ws-workbench"
TAGLINE = "开发工作台"


def font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    candidates = ["msyhbd.ttc", "msyh.ttc"] if bold else ["msyh.ttc", "msyhbd.ttc"]
    for name in candidates:
        path = Path("C:/Windows/Fonts") / name
        if path.exists():
            return ImageFont.truetype(str(path), size)
    return ImageFont.load_default()


def rounded_icon(size: int) -> Image.Image:
    """把应用图标裁成圆角方块，透明底，便于贴到任意背景上。"""
    icon = Image.open(ICON).convert("RGBA")
    icon = icon.resize((size, size), Image.LANCZOS)
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1), radius=max(4, size // 6), fill=255)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(icon, (0, 0), mask)
    return out


def make_header() -> Image.Image:
    img = Image.new("RGB", (150, 57), "white")
    draw = ImageDraw.Draw(img)
    # 左侧一条品牌色细线，和右侧白底形成层次，不至于整张图空荡
    draw.rectangle((0, 0, 2, 56), fill=BRAND)
    icon = rounded_icon(26)
    img.paste(icon, (9, 15), icon)
    # 150px 宽的横条很窄：产品名用小号粗体，一行放得下（不裁字），副标题更小
    draw.text((41, 11), PRODUCT, font=font(12, bold=True), fill=INK)
    draw.text((41, 29), TAGLINE, font=font(10), fill=MUTED)
    return img


def make_sidebar() -> Image.Image:
    img = Image.new("RGB", (164, 314), BRAND)
    draw = ImageDraw.Draw(img)
    # 竖向渐变
    for y in range(314):
        t = y / 313
        color = tuple(
            round(BRAND[i] + (BRAND_DARK[i] - BRAND[i]) * t) for i in range(3)
        )
        draw.line((0, y, 163, y), fill=color)

    # 右上角装饰圆（半透明叠加，避免生硬的色块）
    overlay = Image.new("RGBA", (164, 314), (0, 0, 0, 0))
    odraw = ImageDraw.Draw(overlay)
    odraw.ellipse((96, -46, 226, 84), fill=(255, 255, 255, 26))
    odraw.ellipse((-52, 214, 62, 328), fill=(255, 255, 255, 20))
    img = Image.alpha_composite(img.convert("RGBA"), overlay).convert("RGB")

    draw = ImageDraw.Draw(img)
    icon = rounded_icon(72)
    img.paste(icon, (46, 54), icon)
    draw.text((82, 150), PRODUCT, font=font(19, bold=True), fill="white", anchor="mm")
    draw.text((82, 176), TAGLINE, font=font(12), fill=(219, 234, 254), anchor="mm")
    # 底部一条浅色短线，视觉收尾
    draw.rounded_rectangle((62, 268, 102, 270), radius=1, fill=(191, 219, 254))
    return img


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    header = make_header()
    sidebar = make_sidebar()
    header.save(OUT_DIR / "header.bmp")
    sidebar.save(OUT_DIR / "sidebar.bmp")
    print(f"[nsis] header.bmp {header.size} / sidebar.bmp {sidebar.size} -> {OUT_DIR}")


if __name__ == "__main__":
    main()
