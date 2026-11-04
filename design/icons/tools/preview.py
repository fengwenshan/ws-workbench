#!/usr/bin/env python3
"""生成交付用预览图：大图 + 程序坞尺寸 + 菜单栏实景 + Dock 模拟。"""
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ICONS = Path(__file__).resolve().parents[1]
DIST = ICONS / "dist"
OUT = DIST / "preview"
OUT.mkdir(parents=True, exist_ok=True)

ICON = Image.open(DIST / "ws-workbench-icon-1024.png").convert("RGBA")
TRAY_WHITE = Image.open(DIST / "tray/tray-white-36.png").convert("RGBA")
TRAY_TEMPLATE = Image.open(DIST / "tray/tray-template-36.png").convert("RGBA")

W, H = 2000, 1420
BG = (15, 18, 27, 255)
sheet = Image.new("RGBA", (W, H), BG)
d = ImageDraw.Draw(sheet)

# 背景柔光
glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
gd = ImageDraw.Draw(glow)
gd.ellipse((80, 60, 1200, 1000), fill=(90, 70, 220, 26))
gd.ellipse((700, 300, 1900, 1300), fill=(26, 140, 220, 20))
sheet.alpha_composite(glow)

FONT_CJK = "/System/Library/Fonts/Hiragino Sans GB.ttc"
FONT_LATIN = "/System/Library/Fonts/HelveticaNeue.ttc"
f_title = ImageFont.truetype(FONT_CJK, 46)
f_sub = ImageFont.truetype(FONT_CJK, 24)
f_label = ImageFont.truetype(FONT_CJK, 22)
f_small = ImageFont.truetype(FONT_CJK, 18)

d.text((80, 56), "ws-workbench", font=ImageFont.truetype(FONT_LATIN, 26), fill=(150, 160, 185, 255))
d.text((80, 92), "工具箱 · 图标定稿", font=f_title, fill=(238, 242, 250, 255))

# 大图
big = ICON.resize((520, 520), Image.LANCZOS)
sheet.alpha_composite(big, (80, 210))
d.text((84, 748), "程序坞图标 1024 × 1024", font=f_sub, fill=(160, 170, 195, 255))

# 尺寸阶梯
sizes = [256, 128, 64, 48, 32, 16]
x, y = 700, 210
d.text((x, y - 34), "尺寸阶梯", font=f_label, fill=(160, 170, 195, 255))
for s in sizes:
    im = ICON.resize((s, s), Image.LANCZOS)
    sheet.alpha_composite(im, (x, y + (256 - s) // 2))
    d.text((x, y + 270), f"{s}px", font=f_small, fill=(130, 140, 165, 255))
    x += s + 28

# 菜单栏实景
BAR_W = 1180
def menubar(y, dark, label):
    bar_h = 56
    bar = Image.new("RGBA", (BAR_W, bar_h), (0, 0, 0, 0))
    bd = ImageDraw.Draw(bar)
    if dark:
        for i in range(bar_h):
            t = i / bar_h
            bd.line([(0, i), (BAR_W, i)], fill=(int(28 + 10*t), int(30 + 8*t), int(44 + 6*t), 255))
        fg = (255, 255, 255, 255)
        left_fg = (255, 255, 255, 165)
    else:
        bd.rectangle((0, 0, BAR_W, bar_h), fill=(243, 243, 245, 255))
        fg = (30, 30, 34, 255)
        left_fg = (30, 30, 34, 165)
    # 左侧菜单
    bd.text((24, 15), "文件    编辑    显示    窗口", font=ImageFont.truetype(FONT_CJK, 18), fill=left_fg)
    # 右侧状态区：托盘图标 → Wi-Fi → 电池 → 时间
    tfont = ImageFont.truetype(FONT_CJK, 20)
    time_x = BAR_W - 118
    bd.text((time_x, 14), "周五 12:22", font=tfont, fill=fg)
    battery_x = time_x - 62
    bd.rounded_rectangle((battery_x, 20, battery_x + 40, 36), radius=5, outline=fg, width=2)
    bd.rounded_rectangle((battery_x + 42, 24, battery_x + 45, 32), radius=2, fill=fg)
    bd.rounded_rectangle((battery_x + 4, 24, battery_x + 32, 32), radius=2, fill=fg)
    wifi_x = battery_x - 46
    for r in (22, 14, 7):
        bd.arc((wifi_x - r, 29 - r // 2, wifi_x + r, 29 + r // 2), 200, 340, fill=fg, width=2)
    bd.ellipse((wifi_x - 2, 27, wifi_x + 2, 31), fill=fg)
    tray_x = wifi_x - 42
    sheet.alpha_composite(bar, (700, y))
    if dark:
        white = TRAY_TEMPLATE.copy()
        white.putdata([(255, 255, 255, a) for (_r, _g, _b, a) in white.getdata()])
        tinted = white
    else:
        tinted = TRAY_TEMPLATE
    sheet.alpha_composite(tinted, (700 + tray_x, y + (bar_h - 36) // 2))
    d.text((700, y + bar_h + 10), label, font=f_small, fill=(140, 150, 175, 255))

menubar(210 + 520 - 120, True, "菜单栏（深色）· 模板图自动反白")
menubar(210 + 520 + 0, False, "菜单栏（浅色）· 同一张模板图自动转黑")

# Dock 模拟
dock_y = 1000
dock = Image.new("RGBA", (1740, 150), (0, 0, 0, 0))
dd = ImageDraw.Draw(dock)
dd.rounded_rectangle((0, 0, 1740, 150), radius=34, fill=(255, 255, 255, 26))
dd.rounded_rectangle((0, 0, 1740, 150), radius=34, outline=(255, 255, 255, 40), width=2)
sheet.alpha_composite(dock, (130, dock_y))
d.text((130, dock_y + 168), "程序坞实景（同一尺寸与其他应用并排）", font=f_label, fill=(160, 170, 195, 255))

TMP = OUT / "_sysicons"
TMP.mkdir(exist_ok=True)
system_icons = [
    ("Finder", "/System/Library/CoreServices/Finder.app/Contents/Resources/Finder.icns"),
    ("Notes", "/System/Applications/Notes.app/Contents/Resources/AppIcon.icns"),
    ("Calendar", "/System/Applications/Calendar.app/Contents/Resources/AppIcon.icns"),
    ("Calculator", "/System/Applications/Calculator.app/Contents/Resources/AppIcon.icns"),
    ("Music", "/System/Applications/Music.app/Contents/Resources/AppIcon.icns"),
    ("App Store", "/System/Applications/App Store.app/Contents/Resources/AppIcon.icns"),
]
imgs = [ICON]
for name, path in system_icons:
    out = TMP / f"{name}.png"
    if not out.exists():
        subprocess.run(["sips", "-s", "format", "png", "-Z", "256", path, "--out", str(out)],
                       check=True, capture_output=True)
    imgs.append(Image.open(out).convert("RGBA"))

gap = 18
n = len(imgs)
total = n * 128 + (n - 1) * gap
cx = (W - total) // 2
for i, im in enumerate(imgs):
    x = cx + i * (128 + gap)
    sheet.alpha_composite(im.resize((128, 128), Image.LANCZOS), (x, dock_y + 12))
    if im is ICON:
        d.rounded_rectangle((x - 8, dock_y + 2, x + 136, dock_y + 148), radius=22,
                            outline=(120, 190, 255, 200), width=3)

# 底部说明
d.text((80, H - 96), "渐变 #7A4BE8 → #2C63F0 → #0FB3D8     字形白色渐变 #FFFFFF → #E6EFFF     菜单栏模板图 36px @2x",
       font=f_small, fill=(110, 120, 145, 255))
sheet.convert("RGB").save(OUT / "ws-workbench-icon-preview.png")
print("saved", OUT / "ws-workbench-icon-preview.png")
