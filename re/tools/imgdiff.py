"""Compare reference screenshots from the original game with ours.

For every shot present in both folders (ref: <name>.tga/.png, ours:
<name>.png) prints the mean absolute error, the share of clearly different
pixels and the brightness ratio, and writes <out>/<name>.png showing the
reference, ours and an amplified difference side by side at half size.

    python -I re/tools/imgdiff.py re/out/ref re/out/ours re/out/diff

Standard library only.
"""
import os
import struct
import sys
import zlib


def read_tga(path):
    data = open(path, "rb").read()
    id_len, cmap_type, img_type = data[0], data[1], data[2]
    w, h = struct.unpack_from("<HH", data, 12)
    bpp, desc = data[16], data[17]
    if cmap_type != 0 or img_type not in (2, 10) or bpp not in (24, 32):
        raise ValueError(f"unsupported TGA {path}")
    px = bpp // 8
    off = 18 + id_len
    if img_type == 2:
        raw = data[off:off + w * h * px]
    else:
        out = bytearray()
        while len(out) < w * h * px:
            head = data[off]
            off += 1
            n = (head & 0x7F) + 1
            if head & 0x80:
                out += data[off:off + px] * n
                off += px
            else:
                out += data[off:off + n * px]
                off += n * px
        raw = bytes(out)
    rows = [raw[y * w * px:(y + 1) * w * px] for y in range(h)]
    if not desc & 0x20:
        rows.reverse()
    rgb = bytearray(w * h * 3)
    i = 0
    for row in rows:
        for x in range(w):
            b, g, r = row[x * px], row[x * px + 1], row[x * px + 2]
            rgb[i], rgb[i + 1], rgb[i + 2] = r, g, b
            i += 3
    return w, h, rgb


def read_png(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"not a PNG: {path}")
    off = 8
    idat = b""
    w = h = depth = ctype = 0
    while off < len(data):
        length, = struct.unpack_from(">I", data, off)
        kind = data[off + 4:off + 8]
        body = data[off + 8:off + 8 + length]
        off += 12 + length
        if kind == b"IHDR":
            w, h, depth, ctype = struct.unpack_from(">IIBB", body, 0)
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
    if depth != 8 or ctype not in (2, 6):
        raise ValueError(f"unsupported PNG {path}")
    px = 3 if ctype == 2 else 4
    raw = zlib.decompress(idat)
    stride = w * px
    prev = bytearray(stride)
    rgb = bytearray(w * h * 3)
    pos = 0
    for y in range(h):
        f = raw[pos]
        line = bytearray(raw[pos + 1:pos + 1 + stride])
        pos += 1 + stride
        for i in range(stride):
            a = line[i - px] if i >= px else 0
            b = prev[i]
            c = prev[i - px] if i >= px else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + ((a + b) >> 1)) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        for x in range(w):
            o = (y * w + x) * 3
            rgb[o:o + 3] = line[x * px:x * px + 3]
        prev = line
    return w, h, rgb


def write_png(path, w, h, rgb):
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw += rgb[y * w * 3:(y + 1) * w * 3]

    def chunk(kind, body):
        c = struct.pack(">I", len(body)) + kind + body
        return c + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(bytes(raw), 6)))
        f.write(chunk(b"IEND", b""))


def load(path):
    return read_tga(path) if path.lower().endswith(".tga") else read_png(path)


def half(w, h, rgb):
    hw, hh = w // 2, h // 2
    out = bytearray(hw * hh * 3)
    for y in range(hh):
        for x in range(hw):
            for c in range(3):
                s = 0
                for dy in (0, 1):
                    for dx in (0, 1):
                        s += rgb[((2 * y + dy) * w + 2 * x + dx) * 3 + c]
                out[(y * hw + x) * 3 + c] = s >> 2
    return hw, hh, out


def compare(ref, ours):
    (w, h, a), (w2, h2, b) = ref, ours
    if (w, h) != (w2, h2):
        raise ValueError(f"size mismatch {w}x{h} vs {w2}x{h2}")
    n = w * h
    err_sum = 0
    big = 0
    lum_a = lum_b = 0
    diff = bytearray(n * 3)
    for i in range(n):
        o = i * 3
        da = abs(a[o] - b[o]) + abs(a[o + 1] - b[o + 1]) + abs(a[o + 2] - b[o + 2])
        err_sum += da
        if da > 48:
            big += 1
        lum_a += a[o] * 2 + a[o + 1] * 7 + a[o + 2]
        lum_b += b[o] * 2 + b[o + 1] * 7 + b[o + 2]
        v = min(255, da * 2)
        diff[o], diff[o + 1], diff[o + 2] = v, v // 2, 0
    return {
        "mae": err_sum / (n * 3),
        "big": 100.0 * big / n,
        "bright": (lum_b / lum_a) if lum_a else 0.0,
    }, diff


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    ref_dir, ours_dir, out_dir = sys.argv[1:4]
    os.makedirs(out_dir, exist_ok=True)
    refs = {os.path.splitext(f)[0]: f for f in os.listdir(ref_dir) if f.lower().endswith((".tga", ".png"))}
    rows = []
    for name in sorted(refs):
        ours_path = os.path.join(ours_dir, name + ".png")
        if not os.path.isfile(ours_path):
            continue
        ref = load(os.path.join(ref_dir, refs[name]))
        ours = load(ours_path)
        try:
            stats, diff = compare(ref, ours)
        except ValueError as e:
            print(f"{name}: {e}")
            continue
        w, h = ref[0], ref[1]
        panels = [half(w, h, ref[2]), half(w, h, ours[2]), half(w, h, diff)]
        hw, hh = panels[0][0], panels[0][1]
        montage = bytearray(hw * 3 * hh * 3)
        for p, (_, _, img) in enumerate(panels):
            for y in range(hh):
                dst = (y * hw * 3 + p * hw) * 3
                montage[dst:dst + hw * 3] = img[y * hw * 3:(y + 1) * hw * 3]
        write_png(os.path.join(out_dir, name + ".png"), hw * 3, hh, montage)
        rows.append((name, stats))
        print(f"{name:50} mae {stats['mae']:6.2f}  diff>48 {stats['big']:5.1f}%  brightness x{stats['bright']:.3f}")
    if rows:
        mae = sum(r[1]["mae"] for r in rows) / len(rows)
        bright = sum(r[1]["bright"] for r in rows) / len(rows)
        print(f"{'mean of ' + str(len(rows)) + ' shots':50} mae {mae:6.2f}  brightness x{bright:.3f}")


if __name__ == "__main__":
    main()
