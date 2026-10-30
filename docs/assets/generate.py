"""Regenerates the README diagrams and icons from the workspace.

    python -I docs/assets/generate.py

Writes into docs/assets/: architecture-{dark,light}.svg (the crate graph,
read from the crates' Cargo.toml files, with lines of Rust per crate),
stats-{dark,light}.svg (lines of Rust per crate), workflow-{dark,light}.svg,
tick-{dark,light}.svg and icons/*.svg. Standard library only.
"""

import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))

SANS = "Inter, 'Segoe UI', -apple-system, BlinkMacSystemFont, Roboto, Helvetica, Arial, sans-serif"
MONO = "'JetBrains Mono', 'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace"

THEMES = {
    "dark": {
        "bg": "#0d1117", "border": "#30363d", "node": "#161b22", "ink": "#e6edf3", "muted": "#8b949e",
        "faint": "#21262d", "blue": "#3aa6ff", "orange": "#ff8a1c", "edge": "#484f58",
    },
    "light": {
        "bg": "#ffffff", "border": "#d0d7de", "node": "#f6f8fa", "ink": "#1f2328", "muted": "#57606a",
        "faint": "#eaeef2", "blue": "#0a7cff", "orange": "#f26b00", "edge": "#afb8c1",
    },
}

# One line per crate for the architecture diagram.
ROLES = {
    "portal2": "the game: app, menus, HUD, test harness",
    "portal2_tools": "p2tools: data probes for game files",
    "portal2_game": "entities, I/O, player, portals, physics, scripts, saves",
    "portal2_particles": "PCF particle systems: operators, sim, sprites",
    "portal2_world": "BSP world, sky, ropes, collision, paint map, camera",
    "portal2_ui": "VGUI toolkit, fonts, captions, Bink playback",
    "portal2_render": "Source materials, shaders, textures, studio models",
    "portal2_audio": "sound engine: operator stacks, mixers, device",
    "portal2_core": "clock, console, filesystem, save registry, sets",
    "source_formats": "readers: VPK, BSP, MDL, VTF, VMT, PCF, VCD, WAV...",
    "squirrel": "Squirrel 2.2 interpreter (VScript)",
    "bink": "Bink 1 video + audio decoder",
}

# Accent per crate group: "base" (no Bevy), "engine", "game", "app".
GROUP = {
    "source_formats": "base", "squirrel": "base", "bink": "base", "portal2_core": "engine",
    "portal2_render": "engine", "portal2_audio": "engine", "portal2_ui": "engine", "portal2_world": "engine",
    "portal2_particles": "engine", "portal2_game": "game", "portal2": "app", "portal2_tools": "app",
}


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def rust_lines(path):
    n = 0
    for base, _, files in os.walk(path):
        for f in files:
            if f.endswith(".rs"):
                with open(os.path.join(base, f), encoding="utf-8", errors="replace") as fh:
                    n += sum(1 for _ in fh)
    return n


def workspace():
    """{crate: {"deps": set, "lines": int, "bin": bool}} for crates/*."""
    crates = {}
    cdir = os.path.join(ROOT, "crates")
    for name in sorted(os.listdir(cdir)):
        manifest = os.path.join(cdir, name, "Cargo.toml")
        if not os.path.isfile(manifest):
            continue
        text = open(manifest, encoding="utf-8").read()
        pkg = re.search(r'^\s*name\s*=\s*"([^"]+)"', text, re.M).group(1)
        deps = set()
        section = None
        for line in text.splitlines():
            m = re.match(r"^\s*\[([^\]]+)\]", line)
            if m:
                section = m.group(1)
                continue
            if section in ("dependencies",) or (section and section.endswith(".dependencies") and "dev" not in section):
                # `name.workspace = true`, `name = { workspace = true, ... }` or `name = { path = ... }`.
                m = re.match(r"^\s*([A-Za-z0-9_\-]+)(?:\.workspace)?\s*=", line)
                if m:
                    deps.add(m.group(1))
        src = os.path.join(cdir, name, "src")
        crates[pkg] = {
            "deps": deps,
            "lines": rust_lines(src),
            "bin": os.path.isfile(os.path.join(src, "main.rs")) and not os.path.isfile(os.path.join(src, "lib.rs")),
        }
    return crates


def transitive_reduction(crates):
    def reach(a, seen=None):
        seen = set() if seen is None else seen
        for d in crates[a]["deps"]:
            if d in crates and d not in seen:
                seen.add(d)
                reach(d, seen)
        return seen

    edges = set()
    for a, c in crates.items():
        for d in c["deps"]:
            if d not in crates:
                continue
            if not any(d in reach(o) for o in c["deps"] if o != d and o in crates):
                edges.add((a, d))
    return edges


def layers(crates):
    memo = {}

    def depth(a):
        if a not in memo:
            ds = [depth(d) for d in crates[a]["deps"] if d in crates]
            memo[a] = 1 + max(ds) if ds else 0
        return memo[a]

    top = max(depth(a) for a in crates) + 1
    rows = {}
    for a, c in crates.items():
        rows.setdefault(top if c["bin"] else depth(a), []).append(a)
    order = sorted(rows)
    return [sorted(rows[k], key=lambda n: (-crates[n]["lines"], n)) for k in reversed(order)]


def card(w, h, t, body, title=None):
    head = f'<title>{esc(title)}</title>' if title else ""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img">{head}\n'
        f'<rect x=".5" y=".5" width="{w - 1}" height="{h - 1}" rx="14" fill="{t["bg"]}" stroke="{t["border"]}"/>\n'
        f"{body}\n</svg>\n"
    )


def architecture(crates, t):
    rows = layers(crates)
    edges = transitive_reduction(crates)
    nw, nh, gx, gy, pad, side = 268, 70, 26, 50, 34, 70
    width = max(len(r) for r in rows) * (nw + gx) - gx + side * 2
    height = len(rows) * (nh + gy) - gy + pad * 2 + 40
    pos, row_of = {}, {}
    for i, row in enumerate(rows):
        rw = len(row) * (nw + gx) - gx
        x0 = (width - rw) / 2
        for j, name in enumerate(row):
            pos[name] = (x0 + j * (nw + gx), pad + 40 + i * (nh + gy))
            row_of[name] = i
    accent = {"base": t["muted"], "engine": t["blue"], "game": t["orange"], "app": "url(#appGrad)"}
    out = [
        f'<defs><linearGradient id="appGrad" x1="0" x2="1"><stop offset="0" stop-color="{t["blue"]}"/>'
        f'<stop offset="1" stop-color="{t["orange"]}"/></linearGradient>'
        f'<marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
        f'<path d="M0 0 L10 5 L0 10 z" fill="{t["edge"]}"/></marker></defs>',
        f'<text x="{side}" y="{pad + 8}" font-family="{SANS}" font-size="15" font-weight="600" fill="{t["ink"]}">Workspace crates</text>',
        f'<text x="{width - side}" y="{pad + 8}" text-anchor="end" font-family="{SANS}" font-size="12.5" fill="{t["muted"]}">'
        f"arrows point to dependencies · lines of Rust per crate</text>",
    ]
    # Edges between neighbouring rows curve straight down; longer ones run
    # down a channel at the side, so they never cross another crate.
    channels = {"left": 0, "right": 0}
    for a, d in sorted(edges, key=lambda e: (row_of[e[1]] - row_of[e[0]], e)):
        (ax, ay), (dx, dy) = pos[a], pos[d]
        x1, y1 = ax + nw / 2, ay + nh
        x2, y2 = dx + nw / 2, dy
        style = f'fill="none" stroke="{t["edge"]}" stroke-width="1.4" stroke-linejoin="round" marker-end="url(#arrow)"'
        if row_of[d] - row_of[a] == 1:
            my = (y1 + y2) / 2
            out.append(f'<path d="M{x1:.1f} {y1:.1f} C{x1:.1f} {my:.1f} {x2:.1f} {my:.1f} {x2:.1f} {y2 - 2:.1f}" {style}/>')
            continue
        left = (x1 + x2) / 2 < width / 2
        k = channels["left" if left else "right"]
        channels["left" if left else "right"] += 1
        cx = side / 2 - 8 + k * 9 if left else width - side / 2 + 8 - k * 9
        top, bottom = y1 + gy / 2 - 6 + k * 4, y2 - gy / 2 + 6 - k * 4
        out.append(
            f'<path d="M{x1:.1f} {y1:.1f} V{top:.1f} H{cx:.1f} V{bottom:.1f} H{x2:.1f} V{y2 - 2:.1f}" {style} opacity=".85"/>'
        )
    for name, (x, y) in pos.items():
        c = crates[name]
        a = accent[GROUP.get(name, "engine")]
        lines = f'{c["lines"] / 1000:.1f}k' if c["lines"] >= 1000 else str(c["lines"])
        out.append(
            f'<g transform="translate({x:.1f} {y:.1f})">'
            f'<rect width="{nw}" height="{nh}" rx="11" fill="{t["node"]}" stroke="{t["border"]}"/>'
            f'<rect x="0" y="12" width="4" height="{nh - 24}" rx="2" fill="{a}"/>'
            f'<text x="18" y="29" font-family="{MONO}" font-size="14.5" font-weight="700" fill="{t["ink"]}">{esc(name)}</text>'
            f'<text x="{nw - 14}" y="29" text-anchor="end" font-family="{MONO}" font-size="12" fill="{t["muted"]}">{lines}</text>'
            f'<text x="18" y="51" font-family="{SANS}" font-size="12" fill="{t["muted"]}">{esc(ROLES.get(name, ""))}</text>'
            "</g>"
        )
    return card(width, height, t, "\n".join(out), "portal2rust workspace crates")


def stats(crates, t):
    items = sorted(((c["lines"], n) for n, c in crates.items()), reverse=True)
    total = sum(n for n, _ in items)
    w, pad, bh, gap, label_w = 900, 30, 22, 12, 190
    h = pad * 2 + 46 + len(items) * (bh + gap)
    maxv = items[0][0]
    out = [
        f'<defs><linearGradient id="bar" x1="0" x2="1"><stop offset="0" stop-color="{t["blue"]}"/>'
        f'<stop offset="1" stop-color="{t["orange"]}"/></linearGradient></defs>',
        f'<text x="{pad}" y="{pad + 12}" font-family="{SANS}" font-size="15" font-weight="600" fill="{t["ink"]}">'
        f"{total:,} lines of Rust across {len(items)} crates</text>",
    ]
    for i, (n, name) in enumerate(items):
        y = pad + 40 + i * (bh + gap)
        bw = (w - pad * 2 - label_w - 70) * n / maxv
        out.append(
            f'<text x="{pad}" y="{y + 16}" font-family="{MONO}" font-size="13" fill="{t["ink"]}">{esc(name)}</text>'
            f'<rect x="{pad + label_w}" y="{y}" width="{w - pad * 2 - label_w - 70}" height="{bh}" rx="5" fill="{t["faint"]}"/>'
            f'<rect x="{pad + label_w}" y="{y}" width="{max(bw, 6):.1f}" height="{bh}" rx="5" fill="url(#bar)"/>'
            f'<text x="{w - pad}" y="{y + 16}" text-anchor="end" font-family="{MONO}" font-size="12.5" fill="{t["muted"]}">{n:,}</text>'
        )
    return card(w, h, t, "\n".join(out), "Lines of Rust per crate")


def flow(t, title, steps, loop_label, width=1200):
    """A left-to-right row of numbered step cards with a loop arrow back."""
    pad, gap = 30, 34
    n = len(steps)
    cw = (width - pad * 2 - gap * (n - 1)) / n
    ch = 150
    h = pad * 2 + 34 + ch + 64
    out = [
        f'<defs><marker id="ah" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse">'
        f'<path d="M0 0 L10 5 L0 10 z" fill="{t["muted"]}"/></marker>'
        f'<linearGradient id="loop" x1="0" x2="1"><stop offset="0" stop-color="{t["orange"]}"/><stop offset="1" stop-color="{t["blue"]}"/></linearGradient></defs>',
        f'<text x="{pad}" y="{pad + 12}" font-family="{SANS}" font-size="15" font-weight="600" fill="{t["ink"]}">{esc(title)}</text>',
    ]
    y = pad + 34
    for i, (head, tool, lines) in enumerate(steps):
        x = pad + i * (cw + gap)
        color = t["blue"] if i % 2 == 0 else t["orange"]
        body = "".join(
            f'<text x="18" y="{86 + k * 18}" font-family="{SANS}" font-size="12.5" fill="{t["muted"]}">{esc(s)}</text>'
            for k, s in enumerate(lines)
        )
        out.append(
            f'<g transform="translate({x:.1f} {y})">'
            f'<rect width="{cw:.1f}" height="{ch}" rx="12" fill="{t["node"]}" stroke="{t["border"]}"/>'
            f'<circle cx="30" cy="30" r="13" fill="none" stroke="{color}" stroke-width="2"/>'
            f'<text x="30" y="34.5" text-anchor="middle" font-family="{MONO}" font-size="13" font-weight="700" fill="{color}">{i + 1}</text>'
            f'<text x="52" y="35" font-family="{SANS}" font-size="15" font-weight="700" fill="{t["ink"]}">{esc(head)}</text>'
            f'<text x="18" y="62" font-family="{MONO}" font-size="12" fill="{color}">{esc(tool)}</text>{body}</g>'
        )
        if i < n - 1:
            ax = x + cw + 4
            out.append(f'<path d="M{ax:.1f} {y + ch / 2} H{ax + gap - 10:.1f}" stroke="{t["muted"]}" stroke-width="1.6" marker-end="url(#ah)"/>')
    ly = y + ch + 14
    x_first, x_last = pad + cw / 2, pad + (n - 1) * (cw + gap) + cw / 2
    out.append(
        f'<path d="M{x_last:.1f} {ly} V{ly + 22} H{x_first:.1f} V{ly + 4}" fill="none" stroke="url(#loop)" stroke-width="2" '
        f'stroke-dasharray="5 6" marker-end="url(#ah)"/>'
        f'<text x="{(x_first + x_last) / 2:.1f}" y="{ly + 40}" text-anchor="middle" font-family="{SANS}" font-size="12.5" fill="{t["muted"]}">{esc(loop_label)}</text>'
    )
    return card(width, h, t, "\n".join(out), title)


WORKFLOW = [
    ("Study", "IDA Pro + MCP · REA", ["Read the installed game's", "binaries for facts: constants,", "defaults, file layouts"]),
    ("Write it down", "docs/re/*.md", ["Notes in our own words;", "no decompiled code is", "translated into the engine"]),
    ("Implement", "Claude Code", ["AI writes the Rust against", "the notes, directed and", "reviewed by a human"]),
    ("Compare", "parity harness", ["Same views in both engines,", "image diffs, campaign smoke", "and chain runs, VM oracle"]),
]

TICK = [
    ("FixedFirst", "clock", ["Advance curtime by one", "tick (60 per second)", "and time the I/O queue"]),
    ("FixedUpdate", "player → thinks", ["Movement first, then entity", "thinks and scripts, as the", "original server frame"]),
    ("FixedPostUpdate", "physics", ["Rigid bodies, carried props", "and portal travel on the", "same tick"]),
    ("FixedLast", "I/O queue", ["Fire the entity outputs", "due this tick in order", "(logic_relay, triggers...)"]),
    ("Frame", "render", ["Animation, particles, sound,", "portal views, VGUI, then", "draw the HDR scene"]),
]

# 48x48 line icons for the README feature grid. B/O/G become blue, orange, grey.
ICONS = {
    "rendering": '<path d="M24 7 L39 15.5 V32.5 L24 41 L9 32.5 V15.5 Z" stroke="G"/><path d="M9 15.5 L24 24 L39 15.5 M24 24 V41" stroke="G"/><path d="M24 7 L39 15.5 L24 24 L9 15.5 Z" fill="B" fill-opacity=".28" stroke="B"/>',
    "portals": '<ellipse cx="16.5" cy="24" rx="8" ry="15" stroke="B" stroke-width="3"/><ellipse cx="31.5" cy="24" rx="8" ry="15" stroke="O" stroke-width="3"/>',
    "physics": '<path d="M7 40 Q24 -2 41 40" stroke="G" stroke-dasharray="2 5"/><rect x="19.5" y="11" width="9" height="9" rx="2" fill="O" fill-opacity=".3" stroke="O"/><path d="M4 41 H14" stroke="B" stroke-width="3"/><path d="M34 41 H44" stroke="G"/>',
    "gels": '<path d="M16 8 C16 8 7 19 7 26 a9 9 0 0 0 18 0 C25 19 16 8 16 8 Z" fill="B" fill-opacity=".25" stroke="B"/><path d="M33 16 C33 16 26 25 26 30.5 a7 7 0 0 0 14 0 C40 25 33 16 33 16 Z" fill="O" fill-opacity=".25" stroke="O"/>',
    "particles": '<path d="M24 9 L27 21 L39 24 L27 27 L24 39 L21 27 L9 24 L21 21 Z" fill="O" fill-opacity=".25" stroke="O"/><circle cx="10" cy="10" r="1.6" fill="B" stroke="B"/><circle cx="38" cy="11" r="1.2" fill="G" stroke="G"/><circle cx="39" cy="38" r="1.6" fill="B" stroke="B"/><circle cx="9" cy="37" r="1.2" fill="G" stroke="G"/>',
    "audio": '<path d="M8 19 H15 L24 11 V37 L15 29 H8 Z" fill="G" fill-opacity=".15" stroke="G"/><path d="M30 18 Q34 24 30 30" stroke="B"/><path d="M35 13 Q42 24 35 35" stroke="O"/>',
    "scripting": '<path d="M17 9 Q11 9 11 15 V20 Q11 24 7 24 Q11 24 11 28 V33 Q11 39 17 39" stroke="G"/><path d="M31 9 Q37 9 37 15 V20 Q37 24 41 24 Q37 24 37 28 V33 Q37 39 31 39" stroke="G"/><circle cx="20" cy="24" r="2" fill="B" stroke="B"/><circle cx="28" cy="24" r="2" fill="O" stroke="O"/>',
    "interface": '<rect x="6" y="9" width="36" height="30" rx="4" stroke="G"/><path d="M6 16 H42" stroke="G"/><circle cx="11" cy="12.5" r="1" fill="G" stroke="G"/><path d="M20 22 L30 27.5 L20 33 Z" fill="O" fill-opacity=".3" stroke="O"/>',
    "saves": '<path d="M12.5 15 A14 14 0 1 1 10 26" stroke="B"/><path d="M12.5 8 V15 H19.5" stroke="B"/><path d="M24 17 V24 L29 28" stroke="O"/>',
    "console": '<rect x="5" y="9" width="38" height="30" rx="5" stroke="G"/><path d="M12 19 L18 24 L12 29" stroke="B"/><path d="M22 30 H33" stroke="O"/>',
    "formats": '<path d="M13 6 H28 L37 15 V42 H13 Z" stroke="G"/><path d="M28 6 V15 H37" stroke="G"/><path d="M18 24 H32 M18 30 H32 M18 36 H26" stroke="B"/>',
    "video": '<rect x="5" y="12" width="28" height="24" rx="4" stroke="G"/><path d="M33 20 L43 14 V34 L33 28" stroke="O"/><path d="M15 19 L23 24 L15 29 Z" fill="B" fill-opacity=".3" stroke="B"/>',
}


def icon(body, t):
    body = body.replace('"B"', f'"{t["blue"]}"').replace('"O"', f'"{t["orange"]}"').replace('"G"', '"#8b949e"')
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48" width="48" height="48" fill="none" '
        f'stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">{body}</svg>\n'
    )


def write(name, text):
    path = os.path.join(HERE, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def main():
    crates = workspace()
    for theme, t in THEMES.items():
        write(f"architecture-{theme}.svg", architecture(crates, t))
        write(f"stats-{theme}.svg", stats(crates, t))
        write(f"workflow-{theme}.svg", flow(t, "How the engine is built", WORKFLOW, "repeat until the views and the behaviour match the original"))
        write(f"tick-{theme}.svg", flow(t, "One server tick, then one rendered frame", TICK, "60 ticks per second; frames render as fast as the display allows"))
    for name, body in ICONS.items():
        # Mid-tone colours read on GitHub's light and dark themes alike.
        write(f"icons/{name}.svg", icon(body, {"blue": "#2f8fff", "orange": "#ff7f11"}))
    total = sum(c["lines"] for c in crates.values())
    print(f"{len(crates)} crates, {total:,} lines of Rust; wrote diagrams and {len(ICONS)} icons")


if __name__ == "__main__":
    main()
