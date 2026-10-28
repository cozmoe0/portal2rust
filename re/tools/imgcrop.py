"""Crop the same region from a reference and our screenshot, enlarge it and
put the two side by side, for looking at small details (particles, decals).

    python -I re/tools/imgcrop.py re/out/ref/x.tga re/out/ours/x.png x y w h [scale] out.png

Standard library only.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from imgdiff import load, write_png


def crop(img, x, y, w, h, scale):
    iw, ih, rgb = img
    out = bytearray()
    for yy in range(h * scale):
        sy = min(ih - 1, y + yy // scale)
        for xx in range(w * scale):
            sx = min(iw - 1, x + xx // scale)
            i = (sy * iw + sx) * 3
            out += rgb[i:i + 3]
    return out


def main():
    ref, ours, x, y, w, h = sys.argv[1], sys.argv[2], *map(int, sys.argv[3:7])
    scale = int(sys.argv[7]) if len(sys.argv) > 8 else 2
    out = sys.argv[-1]
    a = crop(load(ref), x, y, w, h, scale)
    b = crop(load(ours), x, y, w, h, scale)
    W, H = w * scale, h * scale
    rows = bytearray()
    for r in range(H):
        rows += a[r * W * 3:(r + 1) * W * 3] + bytes([255, 0, 255]) * 4 + b[r * W * 3:(r + 1) * W * 3]
    write_png(out, W * 2 + 4, H, bytes(rows))


if __name__ == "__main__":
    main()
