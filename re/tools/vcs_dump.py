"""Unpacks the compiled shader combos in a Source `.vcs` file (version 6)
and disassembles them with the Windows SDK's fxc.

    python -I re/tools/vcs_dump.py <file.vcs> <out dir> [static combo ...]

Each static combo's blocks are raw or LZMA packed; a block holds dynamic
combo records (id, size, D3D9 bytecode). Writes `s<static>_d<dynamic>.bin`
and, when fxc is found, a `.asm` listing next to it.
"""
import glob
import lzma
import os
import struct
import subprocess
import sys


def fxc_path():
    hits = sorted(glob.glob(r"C:\Program Files (x86)\Windows Kits\10\bin\*\x64\fxc.exe"))
    return hits[-1] if hits else None


def unpack_block(d, o):
    """Returns (payload, next offset) or (None, next) at the end marker."""
    head = struct.unpack_from("<I", d, o)[0]
    if head == 0xFFFFFFFF:
        return None, o + 4
    size = head & 0x3FFFFFFF
    body = d[o + 4:o + 4 + size]
    if body[:4] == b"LZMA":
        actual, packed = struct.unpack_from("<II", body, 4)
        props = body[12:17]
        lc, rest = props[0] % 9, props[0] // 9
        lp, pb = rest % 5, rest // 5
        dict_size = struct.unpack_from("<I", props, 1)[0]
        filt = [{"id": lzma.FILTER_LZMA1, "dict_size": dict_size, "lc": lc, "lp": lp, "pb": pb}]
        out = lzma.LZMADecompressor(format=lzma.FORMAT_RAW, filters=filt).decompress(body[17:17 + packed], actual)
        return out[:actual], o + 4 + size
    return body, o + 4 + size


def main():
    path, out_dir = sys.argv[1], sys.argv[2]
    wanted = {int(s) for s in sys.argv[3:]}
    d = open(path, "rb").read()
    ver, total, dyn, flags, centroid, nstatic, crc = struct.unpack_from("<7I", d, 0)
    if ver != 6:
        sys.exit(f"unsupported version {ver}")
    recs = [struct.unpack_from("<II", d, 28 + 8 * i) for i in range(nstatic)]
    os.makedirs(out_dir, exist_ok=True)
    fxc = fxc_path()
    print(f"{path}: {total} combos, {dyn} dynamic, statics {[r[0] for r in recs[:-1]]}")
    for sid, off in recs[:-1]:
        if wanted and sid not in wanted:
            continue
        o = off
        while True:
            block, o = unpack_block(d, o)
            if block is None:
                break
            p = 0
            while p + 8 <= len(block):
                did, size = struct.unpack_from("<II", block, p)
                code = block[p + 8:p + 8 + size]
                p += 8 + size
                name = os.path.join(out_dir, f"s{sid}_d{did}.bin")
                with open(name, "wb") as f:
                    f.write(code)
                if fxc:
                    subprocess.run([fxc, "/nologo", "/dumpbin", "/Fc", name[:-4] + ".asm", name], capture_output=True)
                print(f"  static {sid} dynamic {did}: {size} bytes")


if __name__ == "__main__":
    main()
