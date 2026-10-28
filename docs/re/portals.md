# Portal openings

How the installed game draws a portal's opening and rim. Gathered from the
installed `client.dll` (IDA), the shipped materials in
`materials/models/portals`, and the shipped compiled shaders
`shaders/fxc/portal_refract_vs20.vcs` and `portal_refract_ps20b.vcs`
(unpacked with `re/tools/vcs_dump.py` and disassembled with the Windows
SDK's `fxc /dumpbin`; their constant tables keep the parameter names), and
compared with screenshots from the game (`re/shots/m10_portals.txt`).
Written in our own words; no code is copied.

## Materials

The opening uses the `PortalRefract` shader in three stages:
`portal_refract_1` (stage 0, a refraction of the scene used while
switching), `portal_stencil_hole` (stage 1, marks the hole) and
`portalstaticoverlay_1` / `_2` (stage 2, the rim and static). The overlay
material sets `$PortalMaskTexture models/portals/noise-blur-256x256`,
`$PortalColorTexture models/portals/portal-blue-color` (orange for the
second portal; 256 x 1 sRGB gradients) and `$PortalColorScale 4.0`, with
proxies for the time, the open amount and the static amount. A tinted
variant takes two colours instead of the gradient (co-op colours).

## Open and static amounts (client)

- Each frame the open amount rises by `2 / host timescale × frame time` up
  to 1, so a new portal opens in half a second.
- The static amount is set to 1 when a portal activates (on it and on its
  partner) and falls by the frame time to 0. An unlinked portal always
  reports static 1.

## Hole (stage 1)

With `t` the texture coordinate across the portal quad (0 to 1) and
`d = |2t - 1|`, the hole is `d <= s²`, where `s` is the smoothstep
(`3x² - 2x³`) of the open amount.

## Rim and static (stage 2)

- Vertex side: the rim coordinate is `t × 1.075 - 0.0375`. Two noise
  coordinates start at `((t - 0.5) / clamp(open + 0.001, 0, 1) + 0.5) × 0.3`
  and scroll along x at +0.0275 and -0.0275 per second.
- Pixel side, with `c = 1 - static` (the "clarity"):
  - Three noise reads: one at the first coordinate, one at the second
    (its components swapped) pushed by `-0.02 ×` the first read, one at
    the first coordinate pushed by `-0.02 ×` the second read; `avg` is the
    mean of the last two reads' green channels.
  - The rim strength is, inside the hole, a ramp from 0 at `d = s² - 0.3`
    to 1 at `d = s²`, and outside a fade from 1 at `s²` to 0 at
    `s² + 0.075`; times `max(clamp(2.5 s), 1 - c)`.
  - `burn = 1 - smoothstep(clamp((avg - strength + 0.875) / 1.75))` and,
    inside the hole, `fill = 1 - smoothstep(clamp(c / avg))`.
  - `alpha = clamp(strength × burn + inside × fill) × 1.5`; the gradient is
    read at `alpha × √burn × (|t'_y|^1.5 × 0.8 + 0.2)` (brighter toward the
    bottom of the quad) and multiplied by the colour scale, then fogged.
  - The colour and alpha are multiplied by the light scale clamped to 1,
    so the fire does not brighten with the exposure in dark places.
- So an unlinked portal is filled with the fire (clarity 0 makes the fill
  1 everywhere inside), and once linked the fire shrinks back to the rim
  as the static fades.
