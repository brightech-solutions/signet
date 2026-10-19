"""Generates the Signet app icon: a blue rounded-square background holding
a wax-seal medallion, embossed with an "S" set in the app's own bundled
handwriting font (the same one used for typed signatures).

Requires Pillow (`pip install Pillow`) and macOS's `iconutil` to finish the
conversion to .icns.

Usage:
    python3 generate_icon.py <output_dir>
    iconutil -c icns <output_dir>/AppIcon.iconset -o assets/icon/AppIcon.icns
"""
import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter, ImageFont

CANVAS = 1024
FONT_PATH = Path(__file__).resolve().parent.parent / "assets" / "fonts" / "ShadowsIntoLight.ttf"


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

    # The wax seal: a round medallion, slightly uneven at the rim like a
    # real dripped-wax seal, with a drop shadow and an embossed ring.
    cx, cy, r = 512, 540, 300

    shadow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(shadow)
    sdraw.ellipse([cx - r + 10, cy - r + 22, cx + r + 10, cy + r + 22], fill=(0, 0, 0, 90))
    shadow = shadow.filter(ImageFilter.GaussianBlur(20))
    img = Image.alpha_composite(img, shadow)
    draw = ImageDraw.Draw(img)

    wax = (122, 24, 38, 255)
    wax_dark = (92, 16, 28, 255)
    # A few small drips bulging past the main circle for an authentic,
    # hand-pressed edge rather than a perfect circle.
    for dx, dy, dr in [(-220, 190, 46), (200, 205, 52), (-60, 250, 40), (90, 240, 44)]:
        draw.ellipse(
            [cx + dx - dr, cy + dy - dr, cx + dx + dr, cy + dy + dr], fill=wax_dark
        )
    draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=wax)

    # Inner embossed ring, as if pressed by a signet ring's bezel.
    ring_r = r - 38
    draw.ellipse(
        [cx - ring_r, cy - ring_r, cx + ring_r, cy + ring_r],
        outline=wax_dark,
        width=10,
    )

    # Soft highlight, upper-left, for a rounded, glossy wax look.
    highlight = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    hdraw = ImageDraw.Draw(highlight)
    hdraw.ellipse(
        [cx - r * 0.55, cy - r * 0.95, cx + r * 0.35, cy - r * 0.1],
        fill=(255, 255, 255, 60),
    )
    highlight = highlight.filter(ImageFilter.GaussianBlur(40))
    img = Image.alpha_composite(img, highlight)
    draw = ImageDraw.Draw(img)

    # The embossed initial, set in the app's own handwriting font.
    font = ImageFont.truetype(str(FONT_PATH), 360)
    letter = "S"
    bbox = draw.textbbox((0, 0), letter, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    tx, ty = cx - tw / 2 - bbox[0], cy - th / 2 - bbox[1]
    emboss = (224, 198, 150, 255)
    draw.text((tx + 6, ty + 8), letter, font=font, fill=wax_dark)
    draw.text((tx, ty), letter, font=font, fill=emboss)

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
