"""Extract ConVar registrations from a 32-bit Source engine PE (static analysis only).

Looks for the MSVC static-initializer shape
    push <help> ; push <flags> ; push <default> ; push <name> ; mov ecx, <obj> ; call <ctor>
and prints name / default / flags / help as TSV.
"""
import struct
import sys


def load_pe(path):
    data = open(path, "rb").read()
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    nsec = struct.unpack_from("<H", data, e_lfanew + 6)[0]
    opt_size = struct.unpack_from("<H", data, e_lfanew + 20)[0]
    opt = e_lfanew + 24
    image_base = struct.unpack_from("<I", data, opt + 28)[0]
    secs = []
    off = opt + opt_size
    for i in range(nsec):
        name = data[off:off + 8].rstrip(b"\0").decode("latin1")
        vsize, va, rsize, rptr = struct.unpack_from("<IIII", data, off + 8)
        secs.append((name, image_base + va, max(vsize, rsize), rptr, rsize))
        off += 40
    return data, image_base, secs


def va_to_off(secs, va):
    for name, sva, size, rptr, rsize in secs:
        if sva <= va < sva + size:
            rel = va - sva
            if rel < rsize:
                return rptr + rel
    return None


def read_cstr(data, secs, va, limit=512):
    off = va_to_off(secs, va)
    if off is None:
        return None
    end = data.find(b"\0", off, off + limit)
    if end < 0:
        return None
    raw = data[off:end]
    try:
        s = raw.decode("utf-8")
    except UnicodeDecodeError:
        return None
    if any(ord(c) < 9 for c in s):
        return None
    return s


def main():
    path = sys.argv[1]
    data, base, secs = load_pe(path)
    text = [s for s in secs if s[0] == ".text"][0]
    _, tva, tsize, tptr, trsize = text
    code = data[tptr:tptr + trsize]
    seen = set()
    out = []
    i = 0
    n = len(code)
    while i < n - 10:
        # mov ecx, imm32 followed by call rel32
        if code[i] == 0xB9 and code[i + 5] == 0xE8:
            # walk backwards collecting pushes
            pushes = []
            j = i
            for _ in range(10):
                if j >= 5 and code[j - 5] == 0x68:
                    pushes.append(struct.unpack_from("<I", code, j - 4)[0])
                    j -= 5
                elif j >= 2 and code[j - 2] == 0x6A:
                    pushes.append(code[j - 1])
                    j -= 2
                else:
                    break
            if len(pushes) >= 3:
                name = read_cstr(data, secs, pushes[0]) if pushes[0] > base else None
                default = read_cstr(data, secs, pushes[1]) if pushes[1] > base else None
                if name and default is not None and name.replace("_", "").replace("+", "").replace("-", "").isalnum() and len(name) < 80:
                    flags = pushes[2] if len(pushes) > 2 else 0
                    help_s = ""
                    if len(pushes) > 3 and pushes[3] > base:
                        help_s = read_cstr(data, secs, pushes[3]) or ""
                    key = (name, default)
                    if key not in seen:
                        seen.add(key)
                        out.append((name, default, flags, help_s.replace("\t", " ").replace("\n", " ")))
        i += 1
    for name, default, flags, help_s in sorted(out):
        print(f"{name}\t{default}\t0x{flags:x}\t{help_s}")


if __name__ == "__main__":
    main()
