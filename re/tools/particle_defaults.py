"""List the particle operators in a 32-bit Source engine client.dll with the
attributes each one reads from a .pcf and their default values (static
analysis of the installed binary only).

Every particle operator (renderer, operator, initializer, emitter, force,
constraint) has a table of attribute descriptors of 44 bytes each: name,
default string, attribute type, field offset, field size, then extra
fields. The first descriptor ("operator start fadein") is in the data
section; the rest are filled in on first use by a small initializer
function, which this script follows with a tiny emulator for the handful
of instructions those functions use. The table is then traced through the
"unpack params" variables to the operator factory's vtable, whose object
holds the function name; the factory registration call gives the stack.

Usage: python -I re/tools/particle_defaults.py [client.dll] > out.tsv
Output: one line per attribute: stack, functionName, class, attribute,
type, default, offset of the field in the operator object.
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from convar_dump import load_pe, va_to_off, read_cstr

ENTRY = 44
TYPES = {1: "element", 2: "int", 3: "float", 4: "bool", 5: "string", 6: "binary", 7: "time",
         8: "color", 9: "vector2", 10: "vector3", 11: "vector4", 12: "qangle", 13: "quaternion",
         14: "matrix"}
STACKS = {0: "renderer", 1: "operator", 2: "initializer", 3: "emitter", 5: "force", 6: "constraint"}
REGS = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"]


class Image:
    def __init__(self, path):
        self.data, self.base, self.secs = load_pe(path)
        text = [s for s in self.secs if s[0] == ".text"][0]
        self.text_va, self.text_ptr, self.text_size = text[1], text[3], text[4]
        self.code = self.data[self.text_ptr:self.text_ptr + self.text_size]

    def u32(self, va):
        off = va_to_off(self.secs, va)
        if off is None:
            return None
        return struct.unpack_from("<I", self.data, off)[0]

    def code_va(self, i):
        return self.text_va + i

    def find_code(self, pattern):
        out = []
        i = self.code.find(pattern)
        while i >= 0:
            out.append(i)
            i = self.code.find(pattern, i + 1)
        return out

    def find_data(self, value, section=None):
        pat = struct.pack("<I", value)
        out = []
        for name, sva, size, rptr, rsize in self.secs:
            if name == ".text" or (section and name != section):
                continue
            chunk = self.data[rptr:rptr + rsize]
            i = chunk.find(pat)
            while i >= 0:
                if i % 4 == 0:
                    out.append(sva + i)
                i = chunk.find(pat, i + 1)
        return out

    def string_va(self, s):
        needle = b"\0" + s.encode() + b"\0"
        out = []
        for name, sva, size, rptr, rsize in self.secs:
            if name == ".text":
                continue
            chunk = self.data[rptr:rptr + rsize]
            i = chunk.find(needle)
            while i >= 0:
                out.append(sva + i + 1)
                i = chunk.find(needle, i + 1)
        return out


def emulate(img, start, stop):
    """Runs the straight-line initializer from `start` until `ret` or `stop`,
    returning {address: value} of dword stores to absolute addresses."""
    code, i = img.code, start - img.text_va
    regs = {}
    stores = {}
    end = stop - img.text_va
    while i < len(code) and i < end + 16:
        op = code[i]
        if op == 0xC3:
            break
        if op == 0xF6 and code[i + 1] == 0x05:  # test byte [abs], imm8
            i += 7
        elif op in (0x84, 0x85, 0x09) and code[i + 1] & 0xC7 == 0x05:  # test / or [abs], reg
            i += 6
        elif op == 0x0F and 0x80 <= code[i + 1] <= 0x8F:  # jcc rel32
            i += 6
        elif 0x70 <= op <= 0x7F:  # jcc rel8
            i += 2
        elif op == 0xEB:  # jmp rel8
            i += 2
        elif op in (0x83, 0x81) and code[i + 1] == 0x0D:  # or dword [abs], imm
            i += 7 if op == 0x83 else 10
        elif op == 0x80 and code[i + 1] == 0x0D:  # or byte [abs], imm8
            i += 7
        elif op == 0x83 and code[i + 1] >> 6 == 3:  # op reg, imm8
            sub, r = (code[i + 1] >> 3) & 7, code[i + 1] & 7
            imm = struct.unpack_from("<b", code, i + 2)[0] & 0xFFFFFFFF
            if r != 4:
                v = regs.get(REGS[r])
                if sub == 1:  # or; `or reg, -1` sets all bits whatever reg held
                    regs[REGS[r]] = 0xFFFFFFFF if imm == 0xFFFFFFFF else (None if v is None else v | imm)
                elif sub == 0 and v is not None:
                    regs[REGS[r]] = (v + imm) & 0xFFFFFFFF
                elif sub == 5 and v is not None:
                    regs[REGS[r]] = (v - imm) & 0xFFFFFFFF
                else:
                    regs[REGS[r]] = None
            i += 3
        elif op in (0x31, 0x33) and code[i + 1] >> 6 == 3:  # xor reg, reg
            a, b = (code[i + 1] >> 3) & 7, code[i + 1] & 7
            regs[REGS[a]] = 0 if a == b else None
            i += 2
        elif op in (0x50, 0x51, 0x52, 0x53, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x5B, 0x5D, 0x5E, 0x5F):
            i += 1
        elif 0x40 <= op <= 0x47 or 0x48 <= op <= 0x4F:  # inc / dec reg
            r = REGS[op & 7]
            if regs.get(r) is not None:
                regs[r] = (regs[r] + (1 if op < 0x48 else -1)) & 0xFFFFFFFF
            i += 1
        elif 0xB8 <= op <= 0xBF:  # mov reg, imm32
            regs[REGS[op - 0xB8]] = struct.unpack_from("<I", code, i + 1)[0]
            i += 5
        elif op == 0xC7 and code[i + 1] == 0x05:  # mov [abs], imm32
            addr, imm = struct.unpack_from("<II", code, i + 2)
            stores[addr] = imm
            i += 10
        elif op == 0x89 and code[i + 1] & 0xC7 == 0x05:  # mov [abs], reg
            r = REGS[(code[i + 1] >> 3) & 7]
            addr = struct.unpack_from("<I", code, i + 2)[0]
            stores[addr] = regs.get(r)
            i += 6
        elif op == 0xA3:  # mov [abs], eax
            stores[struct.unpack_from("<I", code, i + 1)[0]] = regs.get("eax")
            i += 5
        elif op == 0x8B and code[i + 1] >> 6 == 3:  # mov reg, reg
            regs[REGS[(code[i + 1] >> 3) & 7]] = regs.get(REGS[code[i + 1] & 7])
            i += 2
        elif op == 0x8B and code[i + 1] & 0xC7 == 0x05:  # mov reg, [abs]
            addr = struct.unpack_from("<I", code, i + 2)[0]
            regs[REGS[(code[i + 1] >> 3) & 7]] = stores.get(addr, img.u32(addr))
            i += 6
        elif op == 0xA1:  # mov eax, [abs]
            addr = struct.unpack_from("<I", code, i + 1)[0]
            regs["eax"] = stores.get(addr, img.u32(addr))
            i += 5
        elif op == 0x8D and code[i + 1] >> 6 == 1 and code[i + 1] & 7 != 4:  # lea reg, [reg + disp8]
            dst, src = REGS[(code[i + 1] >> 3) & 7], REGS[code[i + 1] & 7]
            disp = struct.unpack_from("<b", code, i + 2)[0]
            regs[dst] = None if regs.get(src) is None else (regs[src] + disp) & 0xFFFFFFFF
            i += 3
        elif op == 0x8D and code[i + 1] >> 6 == 2 and code[i + 1] & 7 != 4:  # lea reg, [reg + disp32]
            dst, src = REGS[(code[i + 1] >> 3) & 7], REGS[code[i + 1] & 7]
            disp = struct.unpack_from("<i", code, i + 2)[0]
            regs[dst] = None if regs.get(src) is None else (regs[src] + disp) & 0xFFFFFFFF
            i += 6
        elif op == 0xC6 and code[i + 1] == 0x05:  # mov byte [abs], imm8
            i += 7
        elif op == 0x66 and code[i + 1] == 0xC7 and code[i + 2] == 0x05:  # mov word [abs], imm16
            i += 9
        elif op == 0x6A:
            i += 2
        elif op == 0x68:
            i += 5
        elif op == 0xE8:  # call: clobbers the scratch registers
            for r in ("eax", "ecx", "edx"):
                regs[r] = None
            i += 5
        elif op == 0x90 or op == 0xCC:
            i += 1
        else:
            raise ValueError("unknown opcode %02x at %08x" % (op, img.code_va(i)))
    return stores


def tables(img):
    """(table address, initializer start, initializer return) for every operator table."""
    fade = img.string_va("operator start fadein")
    out = []
    for f in fade:
        for t in img.find_data(f, ".data"):
            # The initializer ends with `mov eax, offset table`.
            rets = img.find_code(b"\xB8" + struct.pack("<I", t))
            if not rets:
                continue
            r = rets[0]
            # Its first instructions are `test byte [guard], 1` and a jnz to that `mov`.
            start = None
            for back in range(13, 0x6000):
                j = r - back
                if j < 7:
                    break
                if img.code[j] == 0x0F and img.code[j + 1] == 0x85:
                    rel = struct.unpack_from("<i", img.code, j + 2)[0]
                    if j + 6 + rel != r:
                        continue
                    if img.code[j - 7] == 0xF6 and img.code[j - 6] == 0x05:
                        start = j - 7  # test byte [guard], 1
                    elif img.code[j - 6] == 0x84 and img.code[j - 5] == 0x05:
                        start = j - 6  # test byte [guard], al
                        if img.code[j - 11] == 0xB8:
                            start = j - 11  # mov eax, 1
                    if start is not None:
                        break
            if start is None:
                continue
            out.append((t, img.code_va(start), img.code_va(r)))
    return out


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else r"C:\Program Files (x86)\Steam\steamapps\common\Portal 2\portal2\bin\client.dll"
    img = Image(path)
    code = img.code
    # Index direct calls: target -> call sites.
    calls = {}
    for j in range(len(code) - 5):
        if code[j] == 0xE8:
            tgt = (img.code_va(j) + 5 + struct.unpack_from("<i", code, j + 1)[0]) & 0xFFFFFFFF
            calls.setdefault(tgt, []).append(j)
    # `mov eax, [abs]` readers by address, and `mov [abs], eax` writers.
    loads, stores_eax = {}, {}
    for j in range(len(code) - 5):
        if code[j] == 0xA1:
            loads.setdefault(struct.unpack_from("<I", code, j + 1)[0], []).append(j)
        elif code[j] == 0xA3:
            stores_eax.setdefault(struct.unpack_from("<I", code, j + 1)[0], []).append(j)
    rdata_dwords = {}
    for name, sva, size, rptr, rsize in img.secs:
        if name != ".rdata":
            continue
        for k in range(0, rsize - 3, 4):
            rdata_dwords.setdefault(struct.unpack_from("<I", img.data, rptr + k)[0], []).append(sva + k)
    # `mov [abs], imm32` by immediate.
    imm_stores = {}
    for j in range(len(code) - 10):
        if code[j] == 0xC7 and code[j + 1] == 0x05:
            addr, imm = struct.unpack_from("<II", code, j + 2)
            imm_stores.setdefault(imm, []).append((addr, j))
    store_at = {}
    for imm, lst in imm_stores.items():
        for addr, j in lst:
            store_at.setdefault(addr, []).append(imm)

    def unpack_vars(init_va):
        # call init ; add esp, 4 ; mov [var], eax
        vars_ = []
        for j in calls.get(init_va, []):
            k = j + 5
            if code[k:k + 3] == b"\x83\xC4\x04":
                k += 3
            if code[k] == 0xA3:
                vars_.append(struct.unpack_from("<I", code, k + 1)[0])
        # Copies: mov eax, [var] ; mov [var2], eax
        seen = list(vars_)
        for v in seen:
            for j in loads.get(v, []):
                if code[j + 5] == 0xA3:
                    v2 = struct.unpack_from("<I", code, j + 6)[0]
                    if v2 not in seen:
                        seen.append(v2)
        return seen

    def getter_vtables(var):
        out = []
        for j in loads.get(var, []):
            if code[j + 5] == 0xC3:
                fn = img.code_va(j)
                for slot in rdata_dwords.get(fn, []):
                    out.append(slot - 8)
        return out

    def rtti_name(vtable):
        col = img.u32(vtable - 4)
        if col is None:
            return None
        td = img.u32(col + 12)
        if td is None:
            return None
        return read_cstr(img.data, img.secs, td + 8)

    # Registration: push offset factory ; push stack id ; ... call
    def stack_of(obj):
        pat = b"\x68" + struct.pack("<I", obj)
        for j in img.find_code(pat):
            if code[j - 2] == 0x6A:
                return code[j - 1]
            if code[j + 5] == 0x6A:
                return code[j + 6]
        return None

    rows = []
    for t, start, ret in tables(img):
        try:
            stores = emulate(img, start, ret)
        except ValueError as e:
            print("# table %08x: %s" % (t, e), file=sys.stderr)
            continue
        entries = []
        k = 0
        while True:
            base = t + k * ENTRY
            name_ptr = stores.get(base, img.u32(base))
            if not name_ptr:
                break
            default_ptr = stores.get(base + 4, img.u32(base + 4))
            ty = stores.get(base + 8, img.u32(base + 8))
            field = stores.get(base + 12, img.u32(base + 12))
            name = read_cstr(img.data, img.secs, name_ptr) if name_ptr else None
            default = read_cstr(img.data, img.secs, default_ptr) if default_ptr else ""
            entries.append((name, TYPES.get(ty, str(ty)), default, field))
            k += 1
            if k > 200:
                break
        names = set()
        for var in unpack_vars(start):
            for vt in getter_vtables(var):
                cls = rtti_name(vt)
                # Factories are static objects: vtable, function name, id.
                for obj in img.find_data(vt, ".data"):
                    n = read_cstr(img.data, img.secs, img.u32(obj + 4) or 0)
                    if n:
                        stack = STACKS.get(stack_of(obj))
                        if stack is None and cls:
                            # Registered some other way; the class name tells.
                            stack = ("initializer" if "VC_INIT_" in cls
                                     else "renderer" if "VC_OP_Render" in cls else "operator")
                        names.add((stack or "?", n, cls or "?"))
        if not names:
            names.add(("?", "?", "%08x" % t))
        for stack, n, cls in sorted(names):
            for attr, ty, default, field in entries:
                rows.append((stack, n, cls, attr, ty, default, "" if field is None else "+%d" % field))
    for r in rows:
        print("\t".join(str(x) for x in r))


if __name__ == "__main__":
    main()
