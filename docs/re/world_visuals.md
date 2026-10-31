# World visuals: overlays, sky, ropes, lights, sprites, water, screens

Facts gathered from the installed binaries (`engine.dll`, `client.dll`,
`server.dll`, `materialsystem.dll`, `shaderapidx9.dll`, `stdshader_dx9.dll`)
and from comparing captures of the running game (`re/shots/m8.txt`). All
descriptions are our own; the code in `crates/portal2` is written from them.

## Material conditionals

- A material key `cond?$param` is applied only when `cond` holds. The
  material system knows `lowfill`, `hdr`, `ldr`, `srgb`, `srgb_pc`,
  `srgb_gameconsole`, `GPU>=n` / `GPU<n` (n = 1..3), `360`, `SonyPS3` and
  `gameconsole`, each optionally negated with `!`. Anything else prints a
  warning and the key is skipped.
- `srgb` (and `srgb_pc`) is true when the hardware config reports
  sRGB-correct blending: `r_shader_srgb` is 0 (its default) and the caps
  flag set for DX10-class cards (NVIDIA always, AMD with large vertex
  texture support, Intel by device id). On any current card it is true, so
  for example frosted glass uses `"srgb?$alpha" "0.5"`.

## Decals and overlays

- Decal-like geometry draws with a slope-scaled depth bias:
  `mat_slopescaledepthbias_decal` -2 and `mat_depthbias_decal` -0.0000038
  (negative is toward the viewer in the engine's depth convention). Without
  the slope term overlays lose to the surface below at grazing angles.
- Overlays that list no faces are drawn nowhere.

## WorldVertexTransition

- With `$blendmodulatetexture` the vertex blend is sharpened per texel:
  green is the midpoint and red the half-width of a smoothstep over the
  vertex alpha. `$bumpmap2` is blended with the same factor.

## 2D sky

- The engine loads `skybox/<sv_skyname><side>` for the sides rt, bk, lf,
  ft, up, dn (in that order), falling back to `sky_urb01`. Each side is
  drawn as a quad around the view; the engine's tables map side 0..5 to
  +X, -X, +Y, -Y, +Z, -Z through `rt, lf, bk, ft, up, dn` and place the
  corners (s, t) = (±1, ±1) as (3,-1,2), (-3,1,2), (1,3,2), (-1,-3,2),
  (-2,-1,3), (2,-1,-3) (axis index + 1, sign = direction) of (s, t, 1).
  Texture coordinates are ((s + 1) / 2, 1 - (t + 1) / 2).
- The sky materials are UnlitGeneric; `$nofog` keeps fog off them.

## 3D sky

- `sky_camera` (`scale`, default 16) is drawn first from
  `origin + eye / scale` with its own fog (`fogenable`, `fogcolor`,
  `fogstart`, `fogend`, `fogmaxdensity`, `fogblend` with `fogcolor2` and
  `fogdir`); its fog distances apply at the sky box's own scale. The main
  view then draws over it, with sky faces left out. The sky box geometry
  is the BSP area containing the camera.

## Ropes (`move_rope`, `keyframe_rope`)

- Each keyframe with `NextKey` makes a rope to that keyframe with its own
  settings. `Type` 0, 1, 2 give 10, 4, 2 nodes. The server sends the
  distance between the ends (truncated to an integer) and `Slack`.
- Spring length is (distance + slack - 100) / (nodes - 1), in integer
  arithmetic. Nodes start on the straight line between the ends.
- Simulation steps are 0.02 s: verlet integration keeping 98% of the
  velocity, gravity -1500 units/s² (off with rope flag 0x100); then three
  relaxation passes over the springs, each moving both ends of a spring
  that is too long by half the excess (springs never push), with the
  locked end points reset after each pass and, with `Collide`, a world
  trace pushing nodes 2.2 units off surfaces. Ropes with the initial-hang
  flag settle for five seconds before they are first drawn; we settle
  every static rope that way (and do not simulate ropes on moving parents).
- `Dangling` frees the far end. `rope_min_pixel_diameter` (2) keeps ropes
  at least two pixels wide; `rope_smooth_enlarge` is 1.4. Every rope in the
  single-player maps uses `cable/cable` (a black texture).

## Projected textures (`env_projectedtexture`)

- Spawn flag 1 starts the light on, flag 2 makes it update every frame.
- Colour: `lightcolor` r, g, b / 255 times alpha / 255 times the light
  style, approached at `colortransitiontime` × 255 per second per channel
  (with 0 the colour never changes after the first update).
- The shader scales the colour by `brightnessscale` and the flashlight
  scale `r_flashlightbrightness` (0.25), times 2.5 when blending is
  sRGB-correct (2 without HDR).
- Attenuation: constant 0, linear 100, quadratic 0 (so 100 / distance,
  clamped to 1), faded out between 60% and 100% of `farz`. Field of view
  `lightfov` both ways, near `nearz`, far `farz`.
- Shadows (`enableshadows`, shadow maps of `r_flashlightdepthres` 1024):
  shadowed light keeps `r_flashlightshadowatten` (0.35) of the light,
  blended back toward full shadow where the attenuation is strong.
- The light points at `target` when one is set.
- Only one shines at a time: `TurnOn` first sends `TurnOff` to every other
  `env_projectedtexture`, and one that is on when it activates does the
  same (warning "forced off"). In `sp_a1_intro1` the container's track
  light turning on at the first impact switches off the room, door and
  Wheatley lights, and the map raises `r_flashlightbrightness` to 3 (from
  0.25) through a `point_clientcommand` for the ride.

## Sprites (`env_sprite`)

- Named sprites start hidden unless spawn flag 1 is set; unnamed sprites
  always show (Source's usual rule, not traced in this build). `.spr`
  models mean the material of the same name.
- Size: the texture's size × `scale`, around `$spriteorigin`;
  `$spriteorientation` picks view-plane, upright or oriented quads.
- Render mode 5 (additive) is depth tested. Modes 3 (glow) and 9 (world
  glow) skip the depth test, scale by the visible share of a
  `GlowProxySize` area and fade with 1200² / distance² past 1200 units;
  mode 3 also grows with distance / 200 to keep its size on screen.
- Colour: `rendercolor` × `renderamt` / 255 × `HDRColorScale`.

## Spotlights (`point_spotlight`, `beam_spotlight`)

- Beam material `sprites/glow_test02` (a cone picture with its origin at the
  top, faded by the `LampBeam` proxy as the view lines up with the beam),
  halo `sprites/light_glow03`. Defaults `SpotlightLength` 500,
  `SpotlightWidth` 50 (at most 102.3), `HDRColorScale` 1 (0.7 for
  `beam_spotlight`), `maxspeed` 100.
- `beam_spotlight` spawn flags: 1 on, 2 no dynamic light, 4 rotation on,
  8 reverse, 16 / 32 rotate about X / Y instead of Z.
- How bright the beam and halo are and the exact halo fade were not traced;
  ours are matched by eye.

## Water and goo

- Portal 2's water materials use flow maps: `$flowmap` gives the
  direction, `$color_flow_*` scroll the goo's `$basetexture` in two phases;
  `$fogcolor` with `$lightmapwaterfog` gives the murky colour, lit by the
  lightmap; `$reflecttexture` with `$reflectamount` and `$reflecttint` adds
  a reflection. We draw the fog colour, the flowing base texture (its alpha
  marks the floating scum) and the map's cube map as the reflection; the
  real-time reflection is not drawn.
- `{r g b}` colours are 0-255 and `[r g b]` 0-1 (the fog, refraction and
  reflection tints); scaling the former twice made every water's fog black.
- Water one sees into (`$refracttexture`; consoles get `$pseudotranslucent`
  instead): the scene behind, shifted by the flowing `$normalmap` by
  `$refractamount` before the perspective divide and times `$refracttint`,
  goes to `$fogcolor` (lit by the lightmap with `$lightmapwaterfog`) with
  the length of the view's path under the surface between `$fogstart` and
  `$fogend`; the reflection (`$reflecttint`) goes over it by a Fresnel term
  (`$reflectamount` is how far the reflection is shifted, not its
  strength). The game reads the depth behind from its refraction texture;
  we bake the depth of the ground under each water surface at map load. In
  `sp_a1_wakeup` the chamber's puddles are one water plane at z 396 over
  displacement ground whose lightmap under the water is almost black, so
  the puddles show the lit fog colour, brown, as in the game.

## Emancipation fields (`SolidEnergy` with `$flowmap`)

- Fizzler brushes use `effects/fizzler*`: `SolidEnergy` with a flow map,
  `$flow_color` [0.025 0.08 0.1], `$outputintensity` 2.3 on sRGB-correct
  PCs, `$flowbounds` masking the edges and `$basetexture`
  `fizzler_ripples_dim`, whose green channel holds the ripples and whose
  alpha a dim copy. Drawing the texture itself shows them green; we add
  the alpha ripples (scrolled along the flow map in two phases like the
  goo) × the flow colour × the intensity, which matches the game's
  brightness at the same exposure. The `FizzlerVortex` swirl around
  objects is not drawn.

## Movie screens (`vgui_movie_display`)

- A screen's rectangle has its lower left corner at the origin and runs
  along the entity's +Y (width) and +Z (height), facing +X.
- `forcedslave` screens show their group master's movie, in step. The
  elevator scripts (`videos/video_splitter.nut`) set the master's movie,
  enable it and give each panel a part of the frame with `SetUseCustomUVs`
  and `SetUMin` / `SetUMax` / `SetVMin` / `SetVMax`.

- Movies are drawn by the `Bik` shader, which turns the frame's YUV into
  RGB and writes it as it is (fogged when fog is on): no lighting, no
  tone mapping scale and no conversion, so a frame shows at its own
  brightness whatever the exposure.

## Progress signs (`vgui_screen` with `sp_progress_sign`)

- The panel (`CVGUI_SP_ProgressSignScreen`, 400 × 808 pixels from
  `scripts/vgui_screens.txt`) reads the map's entry under `Maps` in
  `scripts/vscripts/transitions/sp_lightboard_icons.txt`: `level_number`,
  `total_levels`, `startup` (a sequence in `lightboard_startup_sequences.txt`),
  `dirt` (0-2, an `elevator_video_overlay1..3` over everything) and up to ten
  `icons` (name and 1 for lit). Icon names index the five-by-five sheet
  `vgui/screens/P2_lightboard_vgui` in this order: cube_drop, cube_button,
  cube_bonk, drink_water, goop, crushers, laser_cube, turret, turret_burn,
  portal_fling, plate_fling, bridges, bridge_block, grinders, tbeams,
  paint_bounce, paint_speed, handoff, button_stand, laser_power,
  portal_fling_2, tbeam_polarity, danger_field; an unknown name leaves its
  slot empty.
- Painting, in order: the board `vgui_coop_progress_board` over the whole
  panel in grey 140; the chamber number's two digits from
  `vgui_coop_progress_board_numbers` (a sixth of the sheet wide and half
  tall, 0-5 on the top row and 6-9 below), each a third of 40% of the
  panel's width wide and that 40% tall, from x 70 and y a quarter of the
  40%, white; the bar `vgui_coop_progress_board_bar` from (81, 461) to
  (81 + (width - 162) × level / total, 493) repeating every 32 pixels
  (full when the level is 0), white; the icons, 53 pixels square every 63
  from (81, 577), five to a row, black with alpha 255 when lit and 140
  when not; the dirt, white; then the `LevelProgressLabel` "%02d/%02d" in
  black (`CoopLevelProgressFont_Small`, UniversLTStd-BoldCn 28) at
  (81, 386) in a box 100 tall.
- When the screen becomes active the startup sequence plays: the board
  grey drops to a quarter after `flicker_quick_min` seconds and every
  `flicker_rate_min..max` seconds (random) switches between a quarter and a
  half of 140 (the dirt's alpha between 63 and 127) until
  `bg_flicker_length`; digits, bar and label have alpha 2 until
  `level_number_delay`; the icons are drawn at a tenth until
  `progress_delay`. The quick dimming time is a random number between the
  minimum and the minimum again.

## Area portal windows (`func_areaportalwindow`)

- The window takes over the brush model of the entity its `target` names
  (which stops drawing itself) and draws it blended by distance: `d` is the
  distance from the view to the brush's bounds (times the zoom factor);
  with equal fade distances the blend is `TranslucencyLimit` while `d` is
  below them and 1 from there on, otherwise
  `lerp(TranslucencyLimit, 1, clamp((d - FadeStartDist) / (FadeDist -
  FadeStartDist)))`. A blend of 0 draws nothing. Inputs
  `SetFadeStartDistance` and `SetFadeEndDistance`.

## Light styles

- A face stores one lightmap per light style it is lit by (four for each
  style on bump-mapped faces, flat first), in the order of its style
  slots; the lightmap is their sum, each scaled by its style's brightness.
  A style's pattern is stepped through at ten letters a second; a letter's
  brightness is `(letter - 'a') × 22 / 264`, so `m` is 1 and `a` is 0.
  World lights lighting models are scaled the same way.

## Laser beams (`env_portal_laser`)

- The client draws each stretch of the beam (one per portal crossing) as
  an engine beam with `sprites/purplelaser1` 32 units wide in white
  (lethal lasers: `sprites/laserbeam`, 2 units, colour 100 255 100). A
  beam is a strip facing the view, its full width across U and its
  texture repeating every 100 units of length along V (at least half a
  repeat), drawn additive. `purplelaser1` is UnlitTwoTexture: the base
  (a narrow hot core in a wide dark red falloff) times
  `effects/laserplane_atmosphere` under a texture transform (scale 0.4,
  rotation 27.5°, offset by a LinearRamp of 0.02 per second) times
  `$color`, a UniformNoise between 0.9 and 1.05, times the tone mapping
  scale; it is not fogged.
- The server's beam trace starts inside the emitter's housing; as with any
  engine trace a solid it starts inside and leaves does not stop it.

## Dynamic tone mapping (auto exposure)

- `client.dll` measures the 3D view into a luminance histogram: with the
  newer algorithm (`mat_tonemap_algorithm` 1) sixteen buckets with edges
  (i / 16)^2.5 in linear luminance plus one for pixels at 1 and above
  (left out of the totals). One bucket's occlusion query is reissued per
  frame.
- The location of `p` percent bright pixels walks the buckets from the
  top, interpolating inside the bucket where the share is reached; it
  snaps to a second value when that falls in the same bucket.
- Target: the scale that puts the `mat_tonemap_percent_bright_pixels` (2)
  location at `mat_tonemap_percent_target` (60%), raised if needed so the
  median reaches `mat_tonemap_min_avglum` (3%), times the current scale.
- The target is clamped to the auto-exposure range (`mat_autoexposure_min`
  0.5, `mat_autoexposure_max` 2, or what `env_tonemap_controller` set; test
  chambers use 0.5 to 3), kept in a ten-entry history averaged with weights
  |i - 5| from the oldest, clamped again. The scale moves toward it by
  `mat_hdr_manual_tonemap_rate` (1, or the controller's `SetTonemapRate`)
  × 2 × frame time, at most 1/64 per frame; when darkening the rate rises
  toward `mat_accelerate_adjust_exposure_down` (3) times the rate in
  proportion to the gap. `mat_force_tonemap_scale` overrides it.
- Our scene's linear values match the game's, so the scale is used as the
  view's exposure directly; the histogram comes from a compute pass over
  the finished view, read back a frame or two later.

## Additive blending in Bevy

- Bevy 0.20 draws `AlphaMode::Add` with premultiplied-alpha blending, so
  additive shaders must write zero alpha with the colour already scaled.
