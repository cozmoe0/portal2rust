"""Split-view comparison of one view in the original game and in ours.

    python -I re/tools/splitview.py re/out/ref/portal_linked_near.tga \
        re/out/ours/portal_linked_near.png docs/media/parity/portals.webp

The left part of the output is the original (`capture.py`), the right part
ours (`--shots`), divided at `--split` (fraction of the width) with a thin
line and a label on each side. Needs Pillow.
"""

import argparse
import os

from PIL import Image, ImageDraw, ImageFilter, ImageFont

FONTS = ["C:/Windows/Fonts/segoeuib.ttf", "/System/Library/Fonts/SFNS.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"]


def font(path, size):
    for p in [path] + FONTS:
        if p and os.path.exists(p):
            return ImageFont.truetype(p, size)
    return ImageFont.load_default()


def label(draw, xy, text, fnt, anchor_right=False):
    pad_x, pad_y = 14, 8
    box = draw.textbbox((0, 0), text, font=fnt)
    tw, th = box[2] - box[0], box[3] - box[1]
    x, y = xy
    if anchor_right:
        x -= tw + pad_x * 2
    draw.rounded_rectangle((x, y, x + tw + pad_x * 2, y + th + pad_y * 2), radius=(th + pad_y * 2) // 2, fill=(13, 17, 23, 200))
    draw.text((x + pad_x - box[0], y + pad_y - box[1]), text, font=fnt, fill=(240, 246, 252, 255))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("original")
    ap.add_argument("ours")
    ap.add_argument("out")
    ap.add_argument("--split", type=float, default=0.5)
    ap.add_argument("--width", type=int, default=1600)
    ap.add_argument("--left", default="Portal 2 (Source engine)")
    ap.add_argument("--right", default="portal2rust (Rust + Bevy)")
    ap.add_argument("--font", default=None)
    ap.add_argument("--quality", type=int, default=86)
    a = ap.parse_args()

    ours = Image.open(a.ours).convert("RGB")
    orig = Image.open(a.original).convert("RGB").resize(ours.size, Image.LANCZOS)
    w, h = ours.size
    cut = round(w * a.split)
    out = ours.copy()
    out.paste(orig.crop((0, 0, cut, h)), (0, 0))
    # Divider: a soft shadow, then a bright line.
    over = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(over)
    d.rectangle((cut - 3, 0, cut + 3, h), fill=(0, 0, 0, 120))
    over = over.filter(ImageFilter.GaussianBlur(4))
    d = ImageDraw.Draw(over)
    d.rectangle((cut - 1, 0, cut + 1, h), fill=(255, 255, 255, 230))
    fnt = font(a.font, max(18, h // 34))
    label(d, (24, 22), a.left, fnt)
    label(d, (w - 24, 22), a.right, fnt, anchor_right=True)
    out = Image.alpha_composite(out.convert("RGBA"), over).convert("RGB")
    if out.width != a.width:
        out = out.resize((a.width, round(h * a.width / w)), Image.LANCZOS)
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    out.save(a.out, quality=a.quality, method=6) if a.out.lower().endswith(".webp") else out.save(a.out)
    print(f"{a.out}: {out.width}x{out.height}, {os.path.getsize(a.out) // 1024} KB")


if __name__ == "__main__":
    main()
