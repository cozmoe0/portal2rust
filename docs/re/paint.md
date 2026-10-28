# Paint (gels)

How the installed game stores, applies and draws paint, and what the gels
do to the player. Gathered from the installed `engine.dll`,
`materialsystem.dll`, `server.dll` and `stdshader_dx9.dll` (IDA), the
console-variable defaults (`re/tools/convar_dump.py`), the shipped
materials under `materials/paint`, and the compiled shader
`platform/shaders/fxc/lightmappedpaint_ps20b.vcs` (unpacked with
`re/tools/vcs_dump.py`, disassembled with `fxc /dumpbin`). Written in our
own words; no code is copied.

## Paint types and colours

| type | power  | colour (cvar, sRGB)                      |
|------|--------|------------------------------------------|
| 0    | bounce | `bounce_paint_color` 0 165 255           |
| 1    | (unused "reflect"; drawn with the portal colour) |
| 2    | speed  | `speed_paint_color` 255 106 0            |
| 3    | portal | `portal_paint_color` 128 128 128         |
| 4    | erase  | `erase_color` 0 0 0 0                    |

Only maps whose worldspawn sets `paintinmap 1` have paint maps;
`maxblobcount` there caps the live blobs.

## Paint maps

- One byte per lightmap luxel, laid out exactly like the lightmap pages
  (pages are 512 x 256 on PC). The top three bits are the paint type, the
  low five bits the amount (0..31, read as amount / 31).
- On upload each byte becomes an RGBA texel: the type's colour in RGB and
  the amount (times 255) in alpha. The texture is not sRGB; the shader
  converts the colour itself.
- Faces never painted: translucent ones, displacements, and faces whose
  texinfo has the no-decals flag (0x2000, which Portal 2 also uses as "no
  paint"). Only lightmapped faces have paint.

## Painting a sphere (one blob impact)

The server paints with a sphere of `sv_paint_surface_sphere_radius` (60)
units, or `sv_erase_surface_sphere_radius` (80) for erase, and amount
`sv_paint_alpha_coat` (0.4). The centre is the impact point pushed 0.1
units off the surface along its normal. Every brush model whose bounds
touch the sphere is painted, the world included.

Choosing the faces of a model: walk the BSP nodes within the radius and
collect each leaf face that can take paint, whose plane distance to the
centre is between 0 and the radius (front side only), and whose polygon
comes within the radius of the centre (closest point over its triangle
fan). The closest such face's plane is remembered. A collected face is
painted if it lies on that closest plane, or if a point on it (its first
vertex plus a quarter of the two first edges, pushed 0.1 along its
normal) is on the front side of the closest plane.

Per face, with `d` the centre's plane distance and `R` the sphere radius:

- The circle on the plane has radius `sqrt(R² - d²)`, in luxels
  `round(max(|s axis|, |t axis|) × that)` (the lightmap axes' lengths).
- The centre in luxel space is its lightmap coordinate minus the face's
  lightmap mins, plus 0.5; luxel `(i, j)`'s distance is measured from the
  integer pair `(i, j)` to that centre.
- The luxels visited are the spans of a midpoint circle of that radius
  around the centre (rows `cy ± y` covering `cx ± x`, and rows `cy ± x`
  covering `cx ± y`, ends rounded outward by half a luxel), clipped to the
  face's luxels `[0, size]`.
- Each visited luxel gets amount `coat × (1 - Bias(clamp(dist / r + d / R),
  paintsplat_bias = 0.1))` with `Bias(x, b) = x^(ln b / ln 0.5)`, plus
  uniform noise in ±`paintsplat_max_alpha_noise` (0.1) clamped to [0, 1]
  (`paintsplat_noise_enabled` 1). It is added to the luxel's amount,
  clamped to 1, or to `paint_max_surface_border_alpha` (0.7) on the face's
  first and last luxel rows and columns, and the luxel takes the new type.
- With `paint_alpha_offset_enabled` (1) the eight neighbours (inside the
  face) of each span's first and last luxel also take the new type,
  keeping their amount, so colours filter cleanly at the splat's edge.
- Erasing sets a luxel's amount to 0 and keeps its type.

## Drawing (LightmappedGeneric's paint pass)

A second, alpha-blended pass over painted faces, used when `gpu_level` is
above 1 (the "thick" variant with bubbles; PC default). Textures:
`$paintsplatnormalmap paint/splatnormal_default`,
`$paintsplatbubblelayout paint/bubblelayout`,
`$paintsplatbubble paint/bubble`, `$paintenvmap paint/paint_envmap_hdr`.
With `uv` the face's flat lightmap page coordinate and `v` the view vector
in tangent space:

- `p` = paint map at `uv`; texels with amount below 0.01 are discarded.
- Splat normal map at `uv × (40, 20)` (one tile per 12.8 luxels); its
  alpha is a height `hs`, and a second read is offset by `v.xy × hs × 0.01`
  (parallax). The paint "thickness" is `h = hs + p.a`.
- Bands of `h` (smoothstep `s(x) = x²(3 - 2x)` of clamped ramps):
  `A = s((0.6 - h/2) / 0.3)` (thin), `B = s((h/2 - 0.34) / 0.29)`
  (covered), `C = s(h - 1)`, `D = s((h/2 - 0.4) / 0.35)`,
  `E = s((h/2 - 0.75) × 20)`, `F = s((h/2 - 0.4) × 20)` (alpha).
- Surface normal: the parallax splat normal with its x and y scaled by
  `A × B × 3.9 + 0.1` (steep at the paint's edge, flat inside).
- Bubbles: the layout texture is read twice, at `uv × (260, 130)` (about
  half a tile per luxel) plus `v.xy × 0.0314`, and at 1.76 times that
  plus `v.xy × 0.00316`; its rg pick a spot inside a bubble (turned to face
  the viewer) and b says whether a bubble is there. Bubble normal maps are
  read there, the first pushed by the screen-space projection of the
  surface normal times `h × C × 0.2` (0.05 for the second); the first
  counts where `b × D`, the second where `b × E`, combined by the second's
  alpha (the first's alpha weighs 0.4). A third, unturned read at half
  size adds bumps (xy negated, doubled) to the surface normal with weight
  `0.4 × (b × E)^3.5`.
- Colour: the paint map is read again 0.003 × (1 - h) further along both
  axes and used where that read's amount is above 0.9 (`s((a - 0.9) ×
  10)`); the colour is converted from sRGB, then pulled toward half its
  luminance where the paint is thin: `lum/2 + B × (rgb - lum/2)`.
- Lighting: the three bumped lightmaps weighted by the squared clamped dot
  of the surface normal with the bump basis, divided by the weights' sum,
  times the tint and lightmap scale (`D`).
- Reflections from `paint_envmap_hdr` (scaled by 1/16 in effect): the
  bubble reflection `2 × env × colour × bubble mask × (N·V)`, and the
  surface reflection `0.67 × env × (1.5 - N·V)`; both times `(D + 1)`.
- Result `colour × (D × 0.82 + 0.04) × (0.8, 0.85, 1) + reflections`,
  times the tone-map scale, fogged; alpha
  `0.85 × (F - 1.25 × (0.2 × bubble₁ + 0.3 × bubble₂))`.

## Blobs and sprayers

- `info_paint_sprayer` emits a blob every `1 / blobs_per_second` while
  active and under `maxblobcount`. Spread: a random point on a disc of
  `blob_spread_radius` at the nozzle, aimed from a cone apex behind it
  (`blob_spread_angle`, clamped 0..89). Speed uniform in
  [`min_speed`, `max_speed`]. A blob streaks when the sprayer's
  `blob_streak_percentage` passes a roll; streak time and dampening are
  uniform in their min/max keyvalues. Radius grows from a random
  start radius to a random end radius over a random grow time.
- Motion, 60 steps per second (`paintblob_update_per_second`):
  `v' = v - g dt` with `g = sv_gravity × paintblob_gravity_scale`; the
  position moves by the mean of `v` and `v'` times `dt`, then
  `v = v' × (1 - paintblob_air_drag × dt)` (drag 0.1). Blobs live until
  they hit something (`paintblob_limited_range` is 0).
- On impact: a blob with streak time left that hits a wall (normal z
  within ±0.5) or comes in within `paintblob_streak_angle_threshold` (45°)
  of the surface keeps sliding along it; otherwise it paints and dies.
  Blobs that hit props paint the prop instead (a light bridge takes the
  paint along its length).

## Bounce gel (player)

- `sv_press_jump_to_bounce` is 3: in the air the player bounces when
  moving into a bounce-painted surface faster than
  `trampoline_bounce_min_impact_speed` (4); on the ground only when jump
  is held (`jump_button_can_activate_trampoline_bounce` 1) or when running
  faster than `bounce_auto_trigger_min_speed` (500) while looking within
  `acos(0.7)` of the direction of travel (auto long jump). Walls touched
  while on the ground need jump too
  (`trampoline_bounce_off_walls_while_on_ground` 0). Crouching
  suppresses bouncing.
- Velocity: the part along the surface normal is removed, then the part
  along the player's up axis. An impulse is built: on floors the normal
  times `bounce_paint_min_speed` (500); on walls the up axis times
  `bounce_paint_wall_jump_upward_speed` (275) plus the normal times 500 ×
  `sv_wall_bounce_trade` (0.73); in between it blends by `1 - up·n`. The
  result is the remaining tangential velocity plus the impulse.
- If the speed into the surface (times `bounce_reflect_restitution`, 1)
  is at least 500 the velocity is reflected instead (the wall impulse
  still applies on vertical walls, with at least 10 u/s upward).
- Wall jump help (`sv_wall_jump_help` 1): a horizontal direction within 5°
  of the wall normal is turned onto it, one 5–9° off is turned by 5°.
- Floor bounces (normal z ≈ 1) add `sv_bounce_paint_forward_velocity_bonus`
  (0.375) of the horizontal velocity, capped at 1.375 × 175.
- The bounce plays `Player.JumpPowerUse` and leaves the ground.

## Speed gel (player)

- On speed paint the player's maximum speed (normally `sv_speed_normal`
  175) rises each tick by `sv_speed_paint_acceleration` (500 per second;
  `sv_speed_paint_ramp_acceleration` 1000 going uphill on a ramp) times
  the alignment of the velocity with the wish direction, times 0.7 when
  strafing; it falls by `|speed - 175| × alignment` when the wish points
  backwards; it is clamped to [175, `sv_speed_paint_max` 800]. Turning
  sharply (alignment under 0.3) pulls it toward the speed along the wish
  direction. No input resets it to 175. Only on the ground.
- The movement code scales the forward and side moves by max speed / 175
  when the max speed is raised, the one less aligned with the velocity by
  `sv_speed_paint_side_move_factor` (0.5), and clamps the wish to the max
  speed. Ground acceleration and friction are the usual ones, except that
  below `sv_stopspeed` friction works as if at twice the stop speed.
- Off speed paint, on the ground, the max speed falls by 500 per second
  (after `sv_speed_paint_on_bounce_deceleration_delay` 0.2 s when on
  bounce paint); in the air it follows the horizontal speed.

## Air control

The Portal 2 air move accelerates with `sv_paintairacceleration` (5), not
`sv_airaccelerate` (10).
