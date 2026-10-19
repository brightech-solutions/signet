"""Generates the PDF Utility app icon: a blue rounded-square background, a
white page with a folded corner and text lines (view/fill), and an orange
pen squiggle (sign) crossing the bottom-right corner.

Requires Pillow (`pip install Pillow`) and macOS's `iconutil` to finish the
conversion to .icns.

Usage:
    python3 generate_icon.py <output_dir>
    iconutil -c icns <output_dir>/AppIcon.iconset -o assets/icon/AppIcon.icns
"""
import sys
import math
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

CANVAS = 1024


def build_master() -> Image.Image:
    img = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))

    # Background squircle (macOS-style rounded square).
    bg_margin = 32
    corner_radius = 220
    draw = ImageDraw.Draw(img)
    draw.rounded_rectangle(
        [bg_margin, bg_margin, CANVAS - bg_margin, CANVAS - bg_margin],
        radius=corner_radius,
        fill=(28, 94, 194, 255),
    )
    # Subtle vertical gradient highlight on the background for depth.
    grad = Image.new("L", (1, CANVAS), 0)
    for y in range(CANVAS):
        grad.putpixel((0, y), int(40 * (1 - y / CANVAS)))
    grad = grad.resize((CANVAS, CANVAS))
    highlight = Image.new("RGBA", (CANVAS, CANVAS), (255, 255, 255, 0))
    highlight.putalpha(grad)
    mask = Image.new("L", (CANVAS, CANVAS), 0)
    mdraw = ImageDraw.Draw(mask)
    mdraw.rounded_rectangle(
        [bg_margin, bg_margin, CANVAS - bg_margin, CANVAS - bg_margin],
        radius=corner_radius,
        fill=255,
    )
    img = Image.composite(Image.alpha_composite(img, highlight), img, mask)

    # The page.
    page_left, page_top = 320, 220
    page_right, page_bottom = 760, 860
    fold = 110  # folded-corner size, top-right

    page_points = [
        (page_left, page_top),
        (page_right - fold, page_top),
        (page_right, page_top + fold),
        (page_right, page_bottom),
        (page_left, page_bottom),
    ]
    # Soft shadow under the page.
    shadow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(shadow)
    sdraw.polygon([(x + 14, y + 20) for x, y in page_points], fill=(0, 0, 0, 70))
    shadow = shadow.filter(ImageFilter.GaussianBlur(18))
    img = Image.alpha_composite(img, shadow)
    draw = ImageDraw.Draw(img)

    draw.polygon(page_points, fill=(255, 255, 255, 255))
    # Folded corner flap.
    draw.polygon(
        [
            (page_right - fold, page_top),
            (page_right, page_top + fold),
            (page_right - fold, page_top + fold),
        ],
        fill=(206, 219, 236, 255),
    )

    # Text lines on the page (view/fill).
    line_color = (170, 184, 204, 255)
    line_left = page_left + 60
    line_right = page_right - 60
    for i, y in enumerate(range(page_top + 160, page_top + 420, 70)):
        right = line_right if i != 2 else line_right - 140
        draw.rounded_rectangle([line_left, y, right, y + 26], radius=13, fill=line_color)

    # Pen/signature squiggle (sign), crossing the lower-right of the page.
    squiggle_color = (240, 140, 30, 255)
    cx, cy = page_right - 60, page_bottom - 90
    points = []
    for i in range(60):
        t = i / 59
        x = cx - 420 + t * 420
        y = cy + 60 * math.sin(t * math.pi * 2.4) * (1 - t * 0.3)
        points.append((x, y))
    draw.line(points, fill=squiggle_color, width=34, joint="curve")
    for p in (points[0], points[-1]):
        r = 17
        draw.ellipse([p[0] - r, p[1] - r, p[0] + r, p[1] + r], fill=squiggle_color)
    # Pen nib at the end of the stroke.
    nib_tip = points[-1]
    nib_dx = points[-1][0] - points[-5][0]
    nib_dy = points[-1][1] - points[-5][1]
    norm = math.hypot(nib_dx, nib_dy) or 1
    nib_dx, nib_dy = nib_dx / norm, nib_dy / norm
    nib_len = 70
    nib_tail = (nib_tip[0] - nib_dx * nib_len, nib_tip[1] - nib_dy * nib_len)
    perp = (-nib_dy, nib_dx)
    half_w = 20
    draw.polygon(
        [
            (nib_tail[0] + perp[0] * half_w, nib_tail[1] + perp[1] * half_w),
            (nib_tail[0] - perp[0] * half_w, nib_tail[1] - perp[1] * half_w),
            (nib_tip[0] - nib_dx * 8, nib_tip[1] - nib_dy * 8),
        ],
        fill=(60, 60, 64, 255),
    )

    return img


def main() -> None:
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(".")
    iconset = out_dir / "AppIcon.iconset"
    iconset.mkdir(parents=True, exist_ok=True)

    master = build_master()

    sizes = [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ]
    for name, size in sizes:
        resized = master.resize((size, size), Image.LANCZOS)
        resized.save(iconset / name)

    print(f"Wrote {iconset}")


if __name__ == "__main__":
    main()
