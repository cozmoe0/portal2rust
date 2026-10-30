"""Builds the project's brand artwork: the README banner (dark and light),
the square logo and the social preview page.

The wordmark and tagline are converted to outlines so the SVGs look the
same everywhere (GitHub shows README images through <img>, which loads no
web fonts). Fonts: Inter SemiBold and JetBrains Mono Medium, both under the
SIL Open Font License. Needs fontTools:

    python -m pip install --target <dir> fonttools
    python docs/assets/brand.py --pylib <dir> --fonts <folder with the fonts>

Writes banner-dark.svg, banner-light.svg, logo.svg and social-preview.html
next to this file. Render social-preview.png from the HTML with a headless
browser, e.g.

    chrome --headless --hide-scrollbars --window-size=1280,640 \
        --screenshot=docs/assets/social-preview.png docs/assets/social-preview.html
"""

import argparse
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

BLUE = "#3aa6ff"
BLUE_DEEP = "#0a7cff"
ORANGE = "#ff8a1c"
ORANGE_DEEP = "#f26b00"


def outline(font, text, size, tracking=0.0):
    """Per-character SVG path data for `text`, laid out from x = 0 on the
    baseline (y down). Returns (paths, width) where paths[i] is the outline
    of text[i] (empty for spaces)."""
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.transformPen import TransformPen

    scale = size / font["head"].unitsPerEm
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    hmtx = font["hmtx"]
    paths, x = [], 0.0
    for ch in text:
        name = cmap[ord(ch)]
        pen = SVGPathPen(glyphs, ntos=lambda v: f"{v:.1f}".rstrip("0").rstrip("."))
        glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, x, 0)))
        paths.append(pen.getCommands())
        x += hmtx[name][0] * scale + tracking * size
    return paths, x - tracking * size


def portal(cx, cy, rx, ry, color, deep, glow_id, inner_id, spin_class, phase):
    """One portal: dark interior, rotating static rings and a glowing rim."""
    return f"""
  <g transform="translate({cx} {cy})">
    <ellipse rx="{rx}" ry="{ry}" fill="url(#{inner_id})"/>
    <g class="{spin_class}"><ellipse rx="{rx * 0.82:.1f}" ry="{ry * 0.86:.1f}" fill="none" stroke="{color}" stroke-opacity=".38" stroke-width="2" stroke-dasharray="3 11" stroke-linecap="round"/></g>
    <g class="{spin_class} rev"><ellipse rx="{rx * 0.62:.1f}" ry="{ry * 0.7:.1f}" fill="none" stroke="{color}" stroke-opacity=".22" stroke-width="1.5" stroke-dasharray="18 22" stroke-linecap="round"/></g>
    <ellipse class="rim" style="animation-delay:{phase}s" rx="{rx}" ry="{ry}" fill="none" stroke="{color}" stroke-width="9" filter="url(#{glow_id})"/>
    <ellipse rx="{rx}" ry="{ry}" fill="none" stroke="{deep}" stroke-width="2.5"/>
    <ellipse rx="{rx - 3}" ry="{ry - 3}" fill="none" stroke="#ffffff" stroke-opacity=".55" stroke-width="1.2"/>
  </g>"""


def particles(path_id, count, dur, light):
    """Sparks travelling along the arc from one portal to the other."""
    out = []
    for i in range(count):
        begin = -dur * i / count
        r = 2.6 if i % 3 else 3.6
        start, end = ("#7cc4ff", "#ffb36b") if not light else ("#0a7cff", "#f26b00")
        out.append(
            f'<circle r="{r}" fill="{start}" filter="url(#spark)">'
            f'<animateMotion dur="{dur}s" begin="{begin:.2f}s" repeatCount="indefinite" rotate="auto">'
            f'<mpath href="#{path_id}"/></animateMotion>'
            f'<animate attributeName="fill" values="{start};{start};{end}" keyTimes="0;0.45;1" dur="{dur}s" begin="{begin:.2f}s" repeatCount="indefinite"/>'
            f'<animate attributeName="opacity" values="0;1;1;0" keyTimes="0;0.08;0.92;1" dur="{dur}s" begin="{begin:.2f}s" repeatCount="indefinite"/>'
            "</circle>"
        )
    return "\n    ".join(out)


def banner(word, tag, light):
    w, h = 1280, 380
    bg0, bg1 = ("#ffffff", "#eef1f5") if light else ("#0d121b", "#06080c")
    panel = ("#1f2328", 0.055) if light else ("#ffffff", 0.045)
    ink = "#1f2328" if light else "#eef2f7"
    muted = "#57606a" if light else "#8b949e"
    vignette = "rgba(120,130,145,0.20)" if light else "rgba(0,0,0,0.70)"
    blue, orange = (BLUE_DEEP, ORANGE_DEEP) if light else (BLUE, ORANGE)
    inner_blue = ("#dbeeff", "#9fd0ff") if light else ("#06182c", "#0b2d52")
    inner_orange = ("#ffe9d6", "#ffc58f") if light else ("#2a1204", "#4d2306")

    word_paths, word_w = word
    tag_paths, tag_w = tag
    wx, wy = (w - word_w) / 2, 196
    tx, ty = (w - tag_w) / 2, 258
    plain = "".join(p for i, p in enumerate(word_paths) if i != 6)
    two = word_paths[6]
    tagline = "".join(tag_paths)

    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-labelledby="title desc">
  <title id="title">portal2rust</title>
  <desc id="desc">A Portal 2 engine reimplementation written from scratch in Rust on top of Bevy</desc>
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{bg0}"/><stop offset="1" stop-color="{bg1}"/></linearGradient>
    <radialGradient id="vignette" cx="50%" cy="46%" r="72%"><stop offset=".55" stop-color="{vignette}" stop-opacity="0"/><stop offset="1" stop-color="{vignette}"/></radialGradient>
    <pattern id="panels" width="96" height="96" patternUnits="userSpaceOnUse" x="16" y="-6">
      <rect x=".5" y=".5" width="95" height="95" rx="3" fill="none" stroke="{panel[0]}" stroke-opacity="{panel[1]}"/>
      <rect x="44" y="44" width="8" height="8" rx="1" fill="{panel[0]}" fill-opacity="{panel[1] * 0.7:.3f}"/>
    </pattern>
    <radialGradient id="innerBlue"><stop offset="0" stop-color="{inner_blue[0]}"/><stop offset="1" stop-color="{inner_blue[1]}"/></radialGradient>
    <radialGradient id="innerOrange"><stop offset="0" stop-color="{inner_orange[0]}"/><stop offset="1" stop-color="{inner_orange[1]}"/></radialGradient>
    <radialGradient id="floorBlue" cx="50%" cy="50%" r="50%"><stop offset="0" stop-color="{blue}" stop-opacity=".35"/><stop offset="1" stop-color="{blue}" stop-opacity="0"/></radialGradient>
    <radialGradient id="floorOrange" cx="50%" cy="50%" r="50%"><stop offset="0" stop-color="{orange}" stop-opacity=".35"/><stop offset="1" stop-color="{orange}" stop-opacity="0"/></radialGradient>
    <linearGradient id="two" x1="0" y1="0" x2="1" y2="1"><stop offset=".1" stop-color="{blue}"/><stop offset=".9" stop-color="{orange}"/></linearGradient>
    <filter id="glowBlue" x="-60%" y="-40%" width="220%" height="180%"><feGaussianBlur stdDeviation="{5 if light else 7}"/></filter>
    <filter id="glowOrange" x="-60%" y="-40%" width="220%" height="180%"><feGaussianBlur stdDeviation="{5 if light else 7}"/></filter>
    <filter id="spark" x="-200%" y="-200%" width="500%" height="500%"><feGaussianBlur stdDeviation="1.2"/></filter>
    <clipPath id="frame"><rect width="{w}" height="{h}" rx="22"/></clipPath>
    <path id="arc" d="M 204 168 C 430 22, 850 22, 1076 168"/>
    <linearGradient id="trail" gradientUnits="userSpaceOnUse" x1="204" y1="0" x2="1076" y2="0"><stop offset="0" stop-color="{blue}"/><stop offset="1" stop-color="{orange}"/></linearGradient>
  </defs>
  <style>
    @keyframes pulse {{ 0%, 100% {{ opacity: .72 }} 50% {{ opacity: 1 }} }}
    @keyframes spin {{ to {{ transform: rotate(360deg) }} }}
    .rim {{ animation: pulse 3.6s ease-in-out infinite }}
    .spin {{ transform-box: fill-box; transform-origin: center; animation: spin 16s linear infinite }}
    .spin.rev {{ animation-duration: 26s; animation-direction: reverse }}
    @media (prefers-reduced-motion: reduce) {{ .rim, .spin {{ animation: none }} }}
  </style>
  <g clip-path="url(#frame)">
    <rect width="{w}" height="{h}" fill="url(#bg)"/>
    <rect width="{w}" height="{h}" fill="url(#panels)"/>
    <ellipse cx="190" cy="330" rx="150" ry="26" fill="url(#floorBlue)"/>
    <ellipse cx="1090" cy="330" rx="150" ry="26" fill="url(#floorOrange)"/>
    <rect width="{w}" height="{h}" fill="url(#vignette)"/>
    {portal(190, 190, 66, 118, blue, BLUE_DEEP, "glowBlue", "innerBlue", "spin", 0)}
    {portal(1090, 190, 66, 118, orange, ORANGE_DEEP, "glowOrange", "innerOrange", "spin", 1.8)}
    <use href="#arc" fill="none" stroke="url(#trail)" stroke-width="1.6" stroke-dasharray="1 10" stroke-linecap="round" opacity="{0.55 if light else 0.45}"/>
    {particles("arc", 9, 4.5, light)}
    <g transform="translate({wx:.1f} {wy})">
      <path d="{plain}" fill="{ink}"/>
      <path d="{two}" fill="url(#two)"/>
    </g>
    <g transform="translate({tx:.1f} {ty})"><path d="{tagline}" fill="{muted}"/></g>
  </g>
  <rect x=".5" y=".5" width="{w - 1}" height="{h - 1}" rx="22" fill="none" stroke="{panel[0]}" stroke-opacity="{panel[1] * 1.6:.3f}"/>
</svg>
"""


def logo():
    s = 512
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {s} {s}" width="{s}" height="{s}" role="img" aria-label="portal2rust logo">
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#151c28"/><stop offset="1" stop-color="#07090d"/></linearGradient>
    <pattern id="grid" width="64" height="64" patternUnits="userSpaceOnUse"><rect x=".5" y=".5" width="63" height="63" rx="2" fill="none" stroke="#fff" stroke-opacity=".05"/></pattern>
    <filter id="glow" x="-50%" y="-30%" width="200%" height="160%"><feGaussianBlur stdDeviation="9"/></filter>
    <radialGradient id="inB"><stop offset="0" stop-color="#06182c"/><stop offset="1" stop-color="#0b2d52"/></radialGradient>
    <radialGradient id="inO"><stop offset="0" stop-color="#2a1204"/><stop offset="1" stop-color="#4d2306"/></radialGradient>
    <clipPath id="clip"><rect width="{s}" height="{s}" rx="112"/></clipPath>
  </defs>
  <g clip-path="url(#clip)">
    <rect width="{s}" height="{s}" fill="url(#tile)"/>
    <rect width="{s}" height="{s}" fill="url(#grid)"/>
    <ellipse cx="190" cy="256" rx="92" ry="164" fill="url(#inB)"/>
    <ellipse cx="322" cy="256" rx="92" ry="164" fill="url(#inO)"/>
    <ellipse cx="190" cy="256" rx="92" ry="164" fill="none" stroke="{BLUE}" stroke-width="16" filter="url(#glow)"/>
    <ellipse cx="322" cy="256" rx="92" ry="164" fill="none" stroke="{ORANGE}" stroke-width="16" filter="url(#glow)"/>
    <ellipse cx="190" cy="256" rx="92" ry="164" fill="none" stroke="{BLUE}" stroke-width="11"/>
    <ellipse cx="322" cy="256" rx="92" ry="164" fill="none" stroke="{ORANGE}" stroke-width="11"/>
    <!-- Blue passes over orange at the top crossing, under it at the bottom. -->
    <path d="M 236 114 A 92 164 0 0 1 271.2 179" fill="none" stroke="{BLUE}" stroke-width="11"/>
  </g>
</svg>
"""


def social_html():
    return """<!doctype html>
<html><head><meta charset="utf-8"><title>portal2rust social preview</title>
<style>
  html, body { margin: 0; width: 1280px; height: 640px; overflow: hidden; background: #06080c; }
  body { display: grid; place-items: center; font: 500 22px/1.4 "JetBrains Mono", Consolas, monospace; color: #8b949e; }
  .wrap { width: 1280px; height: 640px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 28px;
          background: radial-gradient(ellipse at 50% 40%, #111827 0%, #06080c 70%); }
  img { width: 1240px; display: block; }
  .chips { display: flex; gap: 14px; }
  .chip { border: 1px solid #30363d; border-radius: 999px; padding: 8px 18px; color: #c9d1d9; background: #0d1117; }
  .chip b { color: #3aa6ff; font-weight: 600; } .chip i { color: #ff8a1c; font-style: normal; font-weight: 600; }
</style></head>
<body><div class="wrap">
  <img src="banner-dark.svg" alt="">
  <div class="chips">
    <span class="chip"><b>Rust</b> 2024</span><span class="chip"><i>Bevy</i> 0.20</span>
    <span class="chip">plays <b>your own</b> copy</span><span class="chip">clean-room <i>engine</i></span>
  </div>
</div></body></html>
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pylib", help="folder fontTools was installed into")
    ap.add_argument("--fonts", required=True, help="folder holding Inter-SemiBold.otf and JetBrainsMono-Medium.ttf")
    a = ap.parse_args()
    if a.pylib:
        sys.path.insert(0, a.pylib)
    from fontTools.ttLib import TTFont

    inter = TTFont(os.path.join(a.fonts, "Inter-SemiBold.otf"))
    mono = TTFont(os.path.join(a.fonts, "JetBrainsMono-Medium.ttf"))
    word = outline(inter, "portal2rust", 104, -0.025)
    tag = outline(mono, "A PORTAL 2 ENGINE, REBUILT FROM SCRATCH IN RUST", 19, 0.16)
    files = {
        "banner-dark.svg": banner(word, tag, light=False),
        "banner-light.svg": banner(word, tag, light=True),
        "logo.svg": logo(),
        "social-preview.html": social_html(),
    }
    for name, text in files.items():
        with open(os.path.join(HERE, name), "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        print(f"wrote {name} ({len(text.encode()) // 1024} KB)")


if __name__ == "__main__":
    main()
