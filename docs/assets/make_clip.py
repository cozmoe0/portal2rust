"""Turns a folder of numbered frames from `--record` into an animated WebP.

    python -I docs/assets/make_clip.py re/out/record docs/media/clip.webp --fps 30 --step 2 --width 960

`--step 2` keeps every second frame (30 fps capture -> 15 fps clip);
`--start`/`--end` trim by frame number. Needs Pillow.
"""

import argparse
import os

from PIL import Image


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("frames")
    ap.add_argument("out")
    ap.add_argument("--fps", type=float, default=30.0, help="capture rate of the frames")
    ap.add_argument("--step", type=int, default=1, help="keep every n-th frame")
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--end", type=int, default=None)
    ap.add_argument("--width", type=int, default=960)
    ap.add_argument("--quality", type=int, default=78)
    a = ap.parse_args()

    names = sorted(n for n in os.listdir(a.frames) if n.lower().endswith(".png"))
    names = names[a.start : a.end : a.step]
    if not names:
        raise SystemExit("no frames")
    frames = []
    for n in names:
        im = Image.open(os.path.join(a.frames, n)).convert("RGB")
        if im.width != a.width:
            im = im.resize((a.width, round(im.height * a.width / im.width)), Image.LANCZOS)
        frames.append(im)
    duration = round(1000 * a.step / a.fps)
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    frames[0].save(
        a.out, save_all=True, append_images=frames[1:], duration=duration, loop=0, quality=a.quality, method=6
    )
    size = os.path.getsize(a.out) / 1e6
    print(f"{a.out}: {len(frames)} frames, {duration} ms each, {size:.1f} MB")


if __name__ == "__main__":
    main()
