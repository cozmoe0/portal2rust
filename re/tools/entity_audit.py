"""Campaign entity audit: the classes, keyvalues, inputs and spawnflags the
maps use, checked against the quoted strings in the Rust code, to find
engine features not implemented yet. Classes, keys and inputs the code
never names are listed by how often the maps use them, with an example.

    python -I re/tools/entity_audit.py . maps.txt report.txt

`maps.txt` lists one map name per line (`p2tools gamels maps`); the
release p2tools must be built.
"""
import collections
import os
import re
import subprocess
import sys

root = sys.argv[1]
maps = [m.strip() for m in open(sys.argv[2]) if m.strip()]
out = sys.argv[3]
tool = os.path.join(root, "target", "release", "p2tools.exe")

# Every quoted string in the game code, lowercased.
code_strings = set()
for d, _, files in os.walk(os.path.join(root, "crates")):
    for f in files:
        if f.endswith(".rs"):
            text = open(os.path.join(d, f), encoding="utf-8", errors="ignore").read()
            for s in re.findall(r'"([^"\\]{1,80})"', text):
                code_strings.add(s.lower())

GENERIC_KEYS = {
    "origin", "angles", "targetname", "classname", "hammerid", "spawnflags", "parentname", "model", "skin",
    "rendercolor", "renderamt", "rendermode", "renderfx", "disableshadows", "disablereceiveshadows", "fademindist",
    "fademaxdist", "fadescale", "mindxlevel", "maxdxlevel", "drawinfastreflection", "shadowdepthnocache",
    "disableflashlight", "disableshadowdepth", "vrad_brush_cast_shadows", "lightingorigin", "lightingoriginhack",
    "gmod_allowphysgun", "_minlight", "_lightmode", "texframeindex", "linedivider_vscript", "vscripts", "thinkfunction",
    "responsecontext", "globalname", "solidbsp", "invert_exclusion", "inputfilter", "pressuredelay", "performancemode",
    "explodedamage", "exploderadius", "minanimtime", "maxanimtime", "randomanimation", "holdanimation", "setbodygroup",
    "disablebonefollowers", "animateeveryframe", "health", "speed", "target",
}
GENERIC_INPUTS = {
    "kill", "killhierarchy", "addoutput", "fireuser1", "fireuser2", "fireuser3", "fireuser4", "setparent",
    "setparentattachment", "setparentattachmentmaintainoffset", "clearparent", "runscriptcode", "runscriptfile",
    "callscriptfunction", "use", "alpha", "color", "enabledraw", "disabledraw", "dispatcheffect", "setlocalorigin",
    "setlocalangles", "disableshadow", "enableshadow", "skin", "setdamagefilter", "enablereceivingflashlight",
    "disablereceivingflashlight",
}

kv = collections.defaultdict(lambda: collections.Counter())
kv_examples = {}
classes = collections.Counter()
inputs = collections.defaultdict(collections.Counter)
input_maps = {}
flags = collections.defaultdict(collections.Counter)
for m in maps:
    text = subprocess.run([tool, "entdump", m, ""], capture_output=True, text=True, errors="ignore").stdout
    ents = []
    cur = None
    for line in text.splitlines():
        if line.startswith("#"):
            cur = []
            ents.append(cur)
        elif cur is not None and " = " in line:
            k, v = line.strip().split(" = ", 1)
            cur.append((k, v))
    names = {}
    for e in ents:
        d = dict((k.lower(), v) for k, v in e)
        if "targetname" in d:
            names.setdefault(d["targetname"].lower(), d.get("classname", "?"))
    for e in ents:
        d = {}
        for k, v in e:
            d.setdefault(k.lower(), v)
        c = d.get("classname", "?")
        classes[c] += 1
        try:
            sf = int(d.get("spawnflags", "0"))
        except ValueError:
            sf = 0
        for bit in range(32):
            if sf & (1 << bit):
                flags[c][1 << bit] += 1
        for k, v in e:
            kl = k.lower()
            if kl.startswith("on") or kl.startswith("out") or kl in GENERIC_KEYS:
                if kl.startswith("on") or kl.startswith("out"):
                    parts = v.replace("\x1b", ",").split(",")
                    if len(parts) >= 2:
                        tgt, inp = parts[0].lower(), parts[1].lower()
                        tc = names.get(tgt)
                        if tc is None and "*" in tgt:
                            pre = tgt.rstrip("*")
                            tc = next((cc for n, cc in names.items() if n.startswith(pre)), None)
                        if tc is None and tgt.startswith("!"):
                            tc = tgt
                        if tc:
                            inputs[tc][inp] += 1
                            input_maps.setdefault((tc, inp), m)
                continue
            kv[c][kl] += 1
            kv_examples.setdefault((c, kl), (m, v))

with open(out, "w", encoding="utf-8") as f:
    f.write("== keyvalues the code never names (class key count example)\n")
    rows = []
    for c, keys in kv.items():
        for k, n in keys.items():
            if k not in code_strings:
                rows.append((n, c, k))
    for n, c, k in sorted(rows, reverse=True):
        m, v = kv_examples[(c, k)]
        f.write(f"{n:5} {c:32} {k:28} {m}: {v[:50]}\n")
    f.write("\n== inputs the code never names (class input count first-map)\n")
    rows = []
    for c, ins in inputs.items():
        for i, n in ins.items():
            if i not in code_strings and i not in GENERIC_INPUTS:
                rows.append((n, c, i))
    for n, c, i in sorted(rows, reverse=True):
        f.write(f"{n:5} {c:32} {i:28} {input_maps[(c, i)]}\n")
    f.write("\n== classes the code never names\n")
    for c, n in classes.most_common():
        if c.lower() not in code_strings:
            f.write(f"{n:5} {c}\n")
    f.write("\n== spawnflags per class (class: bit=count)\n")
    for c, fl in sorted(flags.items()):
        f.write(f"{c}: " + " ".join(f"{b}={n}" for b, n in sorted(fl.items())) + "\n")
print("done")
