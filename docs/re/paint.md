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
  paint along its length). An entity that is not paintable takes the
  paint's colour as its render colour (CPaintDatabase's `PaintEntity`,
  server.dll sub_103A2960).
- Blob collision (server.dll sub_1039D5C0 and its callers) traces with
  mask 0x46004001: solid, moveable, monster, debris and hitbox, but not
  window or grate. Blobs fall through grates and glass brushes:
  `sp_a3_crazy_box`'s hanging box has a grate lid and floor, so the gel
  falls in onto the cube and out onto the floor beneath.
- Splashes where blobs land (client.dll sub_1039B5E0 / sub_1039A780):
  - The effect is `paint_splat_bounce_01`, `_stick_01` or `_speed_01`;
    portal paint and erasers use `paint_splat_erase_01`. It faces away
    from the surface (VectorAngles of the negated normal).
  - Each sprayer keeps its recent splashes, each counting for
    `paint_impact_particles_duration` (0.2 s).
  - A landing blob gets a splash while there are fewer than
    `paint_min_impact_particles` (20) recent ones, never at
    `paint_max_impact_particles` (50). In between it needs to be at least
    `paint_impact_particles_distance_threshold` (20) from each recent one.
  - `cl_group_paint_impact_effects` 0 turns the grouping off, and
    `draw_paint_splat_particles` 0 turns off the effect.
  - The white spray where sp_a3_crazy_box's cleansing gel lands is these
    splashes.
- Any brush entity's `RemovePaint` input (CBaseEntity, server.dll
  sub_100DEEA0) clears all paint on its model: the engine's
  `RemoveAllPaint` for the model, then a `RemovePaint` user message to
  clients. sp_a3_crazy_box's floor under the cleansing gel is a
  `func_brush` (`erase_cleanser_brush`). It gets `RemovePaint` 1.6 s after
  the gel comes back on, so blue gel put down while the gel was off is
  washed away.

## Drawing blobs (client.dll's Blobulator)

- Blobs are drawn as one metaball surface, so streams pour and nearby
  blobs flow together.
- Each blob goes in as {world centre, radius `r`, field scale `1/r²`},
  with radius clamped to (0.001, 1].
- Sizes are in units of `paintblob_isosurface_box_width` (8), times
  `r_paintblob_blr_scale` (1):
  - a lone blob shows `r_paintblob_blr_render_radius` (1.3) of them
    across its radius;
  - its field reaches `r_paintblob_blr_cutoff_radius` (5.5);
  - the main view samples every `r_paintblob_highres_cube` (0.8); other
    views use 2.5 times that.
- A streaking blob draws at `paintblob_radius_while_streaking` (0.3).
- The falloff inside the Blobulator was not traced. Ours
  (`paint/surface.rs`):
  - metaball `(1 - d²/R²)²` with the game's radii;
  - surface nets over 8³ bricks;
  - normals from the field gradient;
  - each sprayer's group built in parallel.
- Paint bombs (`C_PropPaintBomb`, `cl_draw_paint_bomb_with_blobs` 1):
  - `paintbomb_draw_num_paint_blobs` (20) blobs with radii
    `paintbomb_draw_min_blob_radius`..`max` (0.6..0.8);
  - each starts at a random direction × `paintbomb_draw_sphere_radius`
    (25) from the bomb, moving inward at `paintbomb_draw_blob_speed_min`
    ..`max` (50..80);
  - they ride along with the bomb and reverse when outside the sphere
    while moving outward, stepped 60 times a second;
  - the result is one wobbling ball about the pipe's width.

## Blob material (stdshader_dx9.dll `paintblob`, sub_100210F0)

- Client.dll picks the material by paint type:
  `paintblobs/blob_surface_bounce`, `_stick`, `_speed`, `_portal` or
  `_erase`. A sprayer's blobs are lit as a model at its
  `light_position_name` entity (`m_vLightPosition`).
- Pixel shader static combo:
  `20 × (BACK_SURFACE + 2 LIGHT_WARP + 4 FRESNEL_WARP + 8
  HIGH_PRECISION_DEPTH + 16 INTERIOR_LAYER + 32 OPACITY_TEXTURE + 64
  shadow filter + 256 CONTACT_SHADOW)`. Dynamic combo: lights 0..4.
- Parameter defaults the shader falls back on:
  - `$bumpstrength` 1, `$fresnelbumpstrength` 1;
  - `$translucentfresnelminmaxexp` (0, 0.5, 2);
  - `$interiorambientscale` and `$interiorbacklightscale` 0.3;
  - `$interiorrefractstrength` 0.015;
  - `$phongexponent` 1, `$phongboost` 1, `$phongexponent2` 1,
    `$phongboost2` 1;
  - `$rimlightexponent` 10, `$rimlightboost` 1;
  - `$uvscale` 0.02.
- Shared by every blob material:
  - triplanar `paintblobs/blob_surface_normal` at `$uvscale` 0.006
    (projection weights `max((|n| - 0.2) × 7, 0)`, normalized);
  - Phong 40 × 2 and 400 × 7;
  - `paint/paint_envmap_hdr`.
  - None sets a spec mask; the shader binds white there.
- Per light, with `nr` the bumped normal (`$bumpstrength` scales the
  bump's tangential part), `R = reflect(-V, nr)` and `F = 1 - N·V` from
  the plain normal (`$fresnelbumpstrength` 0):
  - diffuse `halfLambert² × light`;
  - `spec1 = (R·L)^phongexp × sat(N·L) × light`;
  - `spec2 = (R·L)^phongexp2 × sat(N·L) × light`;
  - `rim = (R·L)^rimexp × sat(N·L) × light × F⁴ × rimboost`.
- Opaque paint (`$interior 0`, static 0), with `amb` the ambient cube at
  the vertex normal:

  ```
  base × ((diffuse × 0.82 + 0.04) × (0.8, 0.85, 1) + amb)
    + spec1 × phongboost + spec2 × phongboost2 + rim
    + env(R) × F × 0.67 × c30.z / 16 × (1 + diffuse + amb)
  ```

  c30.z is the cube map scale. Cube maps come out at their stored value
  times c30.z, as on models (see `refract.md`), so the reflection is a
  sixteenth of what `paint_envmap_hdr` holds (values up to 1). Taken at
  full strength it turned the blue blobs silvery white; the game's are a
  flat, matte light blue.

- Erase paint (`$interior 1`, `$opacitytexture`, `$fresnelwarptexture`,
  static 52):
  - Base colour (191, 198, 211); opacity 21/255.
  - With FRESNEL_WARP on, the translucency is the opacity alone; the warp
    texture is never sampled.
  - The interior light is
    `max(average ambient × 0.1, ambient(-normalize(V + N)) × 0.1)`.
  - The frame is looked up at the screen position plus
    `(2 - F) × 0.1 × 80 / distance × bump.xy` (the blended raw
    normal-map xy).
  - The output is
    `lerp(lerp(interior, frame, sqrt(N·V)) × base + env × F × 0.67 × (1 +
    diffuse + amb) + spec1 × 2, opaque layer, 0.082)`, with the same
    sixteenth on the reflection. In the opaque layer
    the rim boost is 300 and the exponent 8.
- Opaque paint draws with the opaque pass; only blobs with erase paint
  among them draw after the frame copy (which glass and water show), so
  blue blobs show through `sp_a3_crazy_box`'s glass.
- Everything, the copied frame included, is multiplied by `cLightScale.x`
  (the tonemap scale). That is why the water reads brighter than what is
  behind it.
- `erase_color` is "0 0 0 0" (for painted surfaces). Drawing erase blobs
  with it left only the reflection, which looked like chrome.

## Paint bombs (`prop_paint_bomb`, server.dll sub_103AB060)

- Burst: sound `PaintBomb.Splat`. Traces over the
  `paintbomb_*_angle_split` directions out to
  `paintbomb_explosion_radius` paint where they land, and players within
  that radius are painted.
- The splash is the particle effect `paint_bomb_bounce` / `_stick` /
  `_speed` / `_erase` (portal paint uses erase). It faces away from the
  last surface a burst trace reached.
- The burst paints brush entities as well as the world. Ours painted only
  the world until 2026-11-26, so the func_brush floor under
  sp_a3_crazy_box's cleansing gel stayed bare.
- Bombs are physics props and cross portals like any other. Ours does
  not burst a bomb heading into an open portal's opening, and reads the
  body's own position, so a crossing in the same tick counts (2026-11-25).
  In `sp_a3_bomb_flings` the floor portal under the dropper sends the
  bomb out the wall portal.
- Bombs burst on contact. Ours probes a tick ahead, and also bursts on a
  sudden drop in speed, but only while touching something. In
  `sp_a3_bomb_flings` a `trigger_catapult` in front of the second ledge's
  wall re-aims bombs coming out of a portal there toward
  `catapult_target2` (352, 288, 168) at 500 u/s; that speed change alone
  burst them in mid-air before.

## Bounce gel on props (`PropPaintPowerUser`)

- A prop's paint power is updated from its physics contacts on every
  physics tick it is awake (`CPhysicsProp::VPhysicsUpdate` override
  sub_103AC230 then sub_103AC4B0: the friction snapshot, one entry per
  contact with an entity, normal and point; `PaintPowerUser::
  UpdatePaintPowers` sub_103ABD90). A painted prop touching anything
  takes the first contact's normal with its own paint. Each power slot
  has a state: 0 activate, 1 in use, 2 deactivate, 3 off. A slot with a
  contact goes to "in use" only if it was in use with the same power,
  the same field at +32 and a normal within 10 ulps of the last one;
  otherwise it is activated again. A slot without a contact is
  deactivated. The handler's return value is the new state.
- Bounce (activate, server.dll sub_10329810): the velocity loses its part
  along the surface normal; a push is built like the player's (up ×
  `bounce_paint_wall_jump_upward_speed` and normal ×
  `bounce_paint_min_speed`, blended by `1 - up·n` and
  `sv_wall_bounce_trade`); a push with no horizontal part gets a random
  ±80 on x and y; the result is the tangential velocity, less its part
  along the push, plus the push. Activate and use (sub_104D9E10) both
  return 2, so bounce never stays in use: every tick the prop touches
  something it is thrown off again. A painted cube at rest leaps 500
  straight away and can never settle against a floor, wall or corner.
  The contact is a real one: the impact happens (and pushes what it
  hits) before the bounce, so contacts the physics keeps for something
  still a little way off do not count.
- Speed (sub_103AC430 / sub_103AC480) swaps the physics material for the
  slippery one and back.
- A weighted cube's paint power switch (sub_103ECB90) gives a bounce-
  painted cube the `WeightedCube_Bounce` physics surface, so its impacts
  play `WeightedCube.JumpPowerActivateLong` / `Short`.
- `prop_weighted_cube`'s `PaintPower` keyvalue (4: none) paints it at
  spawn. `sp_a3_crazy_box`'s cube has 3 (portal), which does nothing
  visible.

## `sp_a3_crazy_box`'s glass box

- The box is a `func_physbox` (`@box_chamber_base`, 874.8 × `massscale`
  0.5 from the physics lump's key data) hung from a
  `phys_ragdollconstraint` at z 2040: up to 90° of tilt, none about the
  vertical (`zmin`/`zmax` 0). Its motion is enabled when the player enters
  the second chamber.
- Eight glass panels ride it: an intact `prop_dynamic`, a cracked one and
  a `lowend` copy at `renderamt` 0 (rendermode 2), and a shattered edge.
  Each has a `trigger_multiple` (physics objects, filter
  `prop_weighted_cube`) adding 1 to a `math_counter` and playing
  `Glass.Break` (at most two at once). A `logic_case` cracks the panel at
  2 (kill the intact one, `Alpha 255` on the cracked ones) and breaks it at
  4 (`Break` the cracked one, play
  `ScriptedSequence.CrazyBoxGlasstastrophe01`, push with an
  `env_physexplosion`; `OnBreak` shows the edge).
- The panels are shadow objects in the game, not joined to the box. Here
  they are kinematic bodies following it, and a quarter of each prop's
  push on them is handed to the box at the contact: the box then swings
  about ±5° while the cube rattles round inside, as measured in the game
  (±4.7°, at most 7°, period about 4.5 s, no turn about the vertical).
  Taken whole (the panels as part of the box) it swung ±25°; not at all,
  ±0.5°.
- The painted cube never stops bouncing by itself in the game either:
  with both sprayer timers running and nobody touching it, it was still
  hopping round the room 196 s on (about 250 units high each time) and
  never wandered into the clear gel stream's alcove. The bounce rule
  above rules out settling. Only paint removal stops it: the timed clear
  gel stream (`paint_sprayer_erase_02`, 2 s on, 2 s off), a
  `trigger_paint_cleanser` or a fizzler. Catching it with the gun holds
  it; carried through the clear gel it is washed and can go on the
  button.
- `func_clip_vphysics` blocks physics objects only. The chamber is lidded
  with one at z 2056 and has more along its -y side and round the
  sprayer nozzles.
- Measured in the game after the gel first reaches the cube: first panel
  hit, first crack 0.6 s later, first break 3.0 s, the cube leaves the box
  4.1 s (`trigger_once` stopping `ambient_sp_a3_crazy_box_x1`) and lands
  on the floor trigger at 7.2 s. Ours, from a fresh map with the same
  portals: cracks after 1–1.6 s, breaks 3.4–4.6 s, out 4–5.4 s, floor
  6.1–8.4 s.

## Bounce gel (player)

- The gel under a touched surface is the paint within
  `sv_paint_detection_sphere_radius` (16) of the contact point, on the
  world and on brush entities alike (a painted `func_brush` floor bounces
  like a painted world floor). Ours looks 2 units off the surface: on a
  brush entity's collision the hull can rest half a unit inside the drawn
  face, and the sphere only sees faces it is in front of.
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
