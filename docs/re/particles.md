# Particle simulation

How the game's client simulates and draws the particle systems described in
`.pcf` files (format: [pcf.md](pcf.md)). Facts come from static analysis of
the installed `portal2/bin/client.dll` (which links the particle library)
and from watching the game; descriptions are our own and the code in
`crates/portal2_particles/src` is written from them.

```bash
python -I re/tools/particle_defaults.py > re/out/particle_defaults.tsv   # every operator's attributes and defaults
cargo run -p portal2_tools -- particlesurvey                    # systems the SP maps start, and what they need
```

## Operator defaults

Every operator (renderer, operator, initializer, emitter, force,
constraint) carries a table of 44-byte attribute descriptors: name, default
string, attribute type, field offset, field size and a few unused words.
The table is filled in by a small initializer function the first time the
operator class is used, and reached through the operator factory, a static
object holding the vtable, the `functionName` and an id.
`re/tools/particle_defaults.py` follows that chain for all 170 factories;
every `functionName` used by a manifest file is among them. Defaults are
stored as strings and parsed like the file values.

Every operator starts with the same common attributes:

| Attribute | Default |
| --- | --- |
| `operator start fadein`, `operator end fadein`, `operator start fadeout`, `operator end fadeout` | 0 |
| `operator fade oscillate` | 0 |
| `operator time offset seed`, `min`, `max` | 0, 0, 0 |
| `operator time scale seed`, `min`, `max` | 0, 1, 1 |
| `operator strength scale seed`, `random scale min`, `random scale max` | 0, 1, 1 |
| `operator end cap state` | -1 |

`operator time strength random scale max` is a second name for the
strength maximum.

## Random numbers

The library draws from a table of 4096 uniform floats in [0, 1): a
collection keeps a counter that advances on every draw and a per-collection
seed added to it. Operators that take a seed index the same table with
`seed + collection seed`.

## Operator strength

Each operator has a strength in [0, 1] from the collection time `t`:

- `operator end cap state` 0 or 1 runs the operator only outside or only
  inside the end-cap phase.
- With a nonzero `time offset seed`, `t` grows by a random value between
  the offset min and max (and is kept non-negative); with a nonzero
  `time scale seed`, the time since `start fadein` is divided by a random
  scale between the scale min and max.
- With `operator fade oscillate` > 0, `t` becomes the fractional part of
  `t / oscillate` (from the unmodified time).
- Strength is 0 before `start fadein` and after `end fadeout` (when that is
  positive), ramps up linearly between `start fadein` and `end fadein`,
  and down between `start fadeout` and `end fadeout` (each end is first
  raised to at least the previous one).
- With a nonzero `strength scale seed` the strength `s` becomes
  `s² × r`, `r` a random value between the strength min and max, kept
  non-negative.
- An operator with strength 0 does not run. Emitters scale their rate by
  it; initializers scale their ranges by it.

## Simulation step

`Simulate(dt)` on a collection:

- Does nothing for a negative `dt`, a frozen collection or one without a
  definition. While the collection time is negative (a child waiting for its
  delay) the time just advances.
- `dt` is split into steps of at most `maximum time step` (0.1 when not
  positive); a `dt` of ten steps or more is cut to one step. With
  `minimum simulation time step` a last short step is rounded up to it.
- Per step: the previous step length and time are kept, the time advances,
  then
  1. operators flagged to run before emission run;
  2. emitters emit and the new particles are initialized;
  3. the remaining operators run (stopping early once no particles are
     left);
  4. renderers get a per-step update.
  Particles killed by an operator are removed after that operator.
- Children are simulated with the same `dt` after the parent.
- Once the time passes `freeze simulation after time` the collection stops
  simulating.

## Emission and initialization

- For each emitter with nonzero strength the number of particles before
  and after `Emit` is compared; new particles are initialized at once.
- Initializers run in order, but one whose written attributes have all
  been written already (by the emitter, which writes the creation time, or
  by an earlier initializer) is skipped. Initializers that modify existing
  values ("Position Modify ...", "Remap ...") run in a second pass
  regardless.
- Attributes nobody wrote get defaults: the creation time is the current
  time, the particle id is a running counter plus the collection seed
  (mod 4096), and everything else takes the definition's base values
  (`radius`, `color` rgb and alpha / 255, `rotation`, `rotation_speed`,
  `sequence_number`, `sequence_number 1`, `normal`).

### `emit_continuously`

- Starts at `emission_start_time` after the emitter's start and runs for
  `emission_duration` seconds (0: forever).
- Rate: `emission_rate` × strength, times the value of
  `emission count scale control point` (component
  `... control point field`, at least 0) when one is set, times
  `scale emission to used control points` × the number of control points
  in use when that is nonzero (or × the parent's particle count with
  `use parent particles for emission scaling`).
- Each step adds rate × (step time inside the active window) to a running
  total; the emitted count is the total's integer part minus what was
  emitted before, so fractions carry over. The new particles' creation
  times are spread evenly over the window. Particles beyond
  `max_particles` are dropped (and still counted as emitted).

### `emit_instantaneously`

- At start the emitter picks its count: `num_to_emit`, or a random integer
  between `num_to_emit_minimum` and `num_to_emit` when the minimum is not
  negative (the two are swapped if needed), and its start time:
  `emission_start_time`, or a random time up to `emission_start_time max`
  when that is positive.
- With `emission count scale control point` the count is multiplied by
  that control point's component when the emitter fires.
- From the start time on it emits up to `maximum emission per frame`
  (negative: `max_particles`, at most 5000) per step until the count is
  used up. All of them get the start time as their creation time.

## Initializers

- Random scalars (`Lifetime Random`, `Radius Random`, `Alpha Random`,
  `Trail Length Random`): `min + rand^exponent × (max - min)` with the
  range scaled by the strength. Alpha bounds are 0..255 integers / 255.
- `Rotation Random` and `Rotation Yaw Random`: degrees converted to
  radians; `initial + min + rand^exponent × (max - min)`, and with
  `randomly_flip_direction` the sign flips with probability ½.
- `Color Random` without `tint_perc`: one random `t` blends `color1` to
  `color2` (bytes / 255, no gamma conversion) for all three channels.
- `Position Within Sphere Random`: a point evenly spread through the unit
  ball (direction from two random numbers, radius the cube root of a
  third), each axis multiplied by `distance_bias` (with
  `distance_bias_absolute_value` taking the absolute value of an axis
  first), normalised, then placed `distance_min + radius × (distance_max -
  distance_min)` from the control point at the particle's creation time
  (turned into the control point's frame with `bias in local system`). The
  velocity is that direction × `speed_min..max` (with
  `speed_random_exponent`) plus a random local velocity between the two
  `speed_in_local_coordinate_system` vectors along the control point's
  forward, right and up (right, not left, for y here). Velocities are
  stored as the previous position, `position - velocity × previous step`.
- `Position Modify Offset Random`: adds a random offset per axis between
  `offset min` and `max` (times the radius with `offset proportional to
  radius`, turned by the control point with `offset in local space`) to
  both the position and the previous position.
- `Sequence Random`: a uniform integer from `sequence_min` to
  `sequence_max`; `shuffle` and `linear` deal from a deck instead.

## Control points

A control point holds a position and forward, up and right vectors. Its
transform takes local x along forward, y along left (minus right) and z
along up. Effects on entities take control point 0 from the entity's
origin and angles.

## Operators

- `Movement Basic`: drag `d` (at least 0) gives a velocity factor
  `(1 - d)^(30 dt) × dt / previous dt`; each step moves `position +=
  (position - previous) × factor + acceleration × dt²` where the
  acceleration is `gravity` plus every force. Constraints then run `max
  constraint passes` times.
- `Lifespan Decay` kills particles whose age reaches their lifetime (or
  whose lifetime is not positive). The lifetime defaults to 1 second.
- `Alpha Fade and Decay`: within `start_fade_in_time..end_fade_in_time` of
  the life the alpha eases (`3t² - 2t³`) from `start_alpha` × the initial
  alpha up to the initial alpha; within the fade-out window down to
  `end_alpha` × the initial alpha; the particle dies at the end of its life.
- `Alpha Fade In Random` / `Alpha Fade In Simple`: the alpha eases up from
  0 over the first `fade in time` of the life (a share of it when
  proportional, seconds otherwise; a per-particle random time between min
  and max).
- `Alpha Fade Out Random`: over the last `fade out time` the alpha goes to
  0, eased with `ease in and out`, otherwise along Schlick's bias curve
  with `fade bias` (0.5 is linear). A random range whose minimum is 0 is
  replaced by a fixed 1.19e-7 (so no fade). `Alpha Fade Out Simple` eases.
- `Radius Scale`: between `start_time` and `end_time` (shares of the life)
  the radius is the initial radius × a lerp from `radius_start_scale` to
  `radius_end_scale`, the fraction eased (`ease_in_and_out`) or biased
  (`scale_bias`); outside the window it is left alone.
- `Color Fade`: from the initial colour to `color_fade` between
  `fade_start_time` and `fade_end_time`, eased by default.
- `Rotation Spin Roll` / `Yaw`: each step adds `spin_rate_degrees` × dt
  (wrapped within a turn when there is no stop time), scaled down to 0
  over `spin_stop_time` × the life, but never below `spin_rate_min` × dt.
  The minimum is applied as written, so a negative rate yields the
  minimum rate instead.
- `Oscillate Scalar` / `Vector`: while the age (a share of the life with
  `start/end proportional`) is between a random start and end time, the
  field changes by `s(x) × rate × dt × strength`, `s` the library's sine
  stand-in `4f(1 - f)` (f the fractional part of x, the sign flipping
  every whole number, about `sin(π x)`), `x = age share × frequency ×
  multiplier + phase` (proportional) or `frequency × (multiplier × time +
  phase)`. Rate and frequency are per-particle randoms between their
  bounds. Alpha stays within 0..1.
- `Remap Control Point to Vector` (also an initializer): each step the
  control point's position is remapped per axis, clamped, from `input
  minimum..maximum` to `output minimum..maximum` (turned into `local space
  CP`'s axes when it is not -1) and written to `output field` for every
  particle, times its initial value with `output is scalar of initial
  random range`. Attributes: `input control point number` (0), `input
  minimum` / `maximum` and `output minimum` / `maximum` (0 0 0), `output
  field` (0), `offset position` and `accelerate position` (0), `local space
  CP` (-1). This is how `portal_success`, `portal_projectile_ball` and the
  tractor beam effects take their colour from control point 2 (0..255 to
  0.25..1); without it they stay white.

## Renderers

- Sheets: VTF resource `0x10`. Version (0 or 1), sequence count, then per
  sequence its number, flags (1 holds the last frame instead of looping;
  2 and 4 mark colour-only and alpha-only sequences), frame count and
  total time, and per frame its display time and one (version 0) or four
  rectangles `u0 v0 u1 v1`. The library cuts each sequence into 512
  samples over its length; a particle's sample index is its age × the
  animation rate × 512, truncated, held at 511 for clamped sequences and
  wrapped otherwise. `use animation rate as FPS` divides the rate by the
  sequence's length in frames; single-frame sequences always show that
  frame. Each sample gives the frame, the next frame and how far between
  them, which the shader blends.
- `render_animated_sprites` sets the material's `$orientation` to
  `orientation_type` (0..3); with 2 the sprites lie in the orientation
  control point's frame. The second sequence (`sequence_number 1`) is
  sampled with `second sequence animation rate`. Particles are quads
  `radius` from centre to edge, turned by their roll.
- `render_sprite_trail`: the trail runs from the particle back toward its
  previous position, `|position - previous| / dt × trail length` long
  (without the division with `ignore delta time`), grown in over `length
  fade in time` of age, at most `max length` and at least `min length`;
  with `constrain radius to length` the radius is at most the length. The
  head takes the particle colour, the tail the colour × `tail color and
  alpha scale factor`.
- Visibility proxies: with `Visibility Proxy Input Control Point Number`
  the visible share of a sphere of `Visibility Proxy Radius` at that
  control point is mapped from the input range to 0..1 and picks the
  alpha and radius scales between their minimum and maximum. We estimate
  the share with five traces from the view.

## Effects started by code

- Portal shots: the server sends a `PortalBlast` effect with no delay; the
  client entity it makes flies from the gun to the landing point,
  starting `cl_portal_projectile_delay_sp` (0.1) after the shot and ending
  at least 0.1 s after that, at `(1 - t) start + t ((1 - t) aim + t end)`
  where `aim` is the start pushed along the shot direction by the shot's
  length. Shots from the player's gun use `portal_projectile_stream` with
  the portal colour on control point 2 (0..255); pedestal launchers use
  `portal_1/2_projectile_stream_pedestal`.
- Portal colours on effects: blue (0, 60, 255), orange (233, 78, 2).
- Placement results make `portal_success` (opened), `portal_badsurface`
  (a surface that takes no portals) or `portal_close` (closed), at the
  portal (or hit point) oriented by it, with the colour on control point 2.
- `env_portal_laser` turning on starts `laser_start_glow` on its laser
  attachment (`reflector_start_glow` on the cube for beams a reflection
  cube sends on). Powered catchers and relays start `laser_relay_powered`
  on their `particle_emitter` attachment after stopping their effects.
- `trigger_portal_cleanser` with `UseScanline` (and
  `cl_portal_cleanser_scanline` 1; the server never initialises the flag,
  so fields whose map entry lacks the key have no scan line, whatever the
  entity definitions' default of 1 says), when enabled and at least 32 units
  tall, runs `cleanser_scanline` with control point 0 at the top corner
  of its bounds' maxima, 1 at (min x, min y, max z), 2 at (max x, max y,
  min z), and 4 and 5 at objects near the field (512 units below the
  bounds otherwise). The effect is made again whenever it has ended.

## AR2 explosion dust (`env_ar2explosion`)

`Explode` makes a client cloud (`C_AR2Explosion`) of 70 puffs of the
entity's material (`particle/particle_noisesphere` by default):

- Each puff first tries three times to land on the world: the segment
  between two random points 240 units from the centre is tested against
  nearby world surfaces and a random hit is used. The puff flies along its
  direction from the centre mirrored in the surface, at
  10000 × (1 − d / 240)² (d the distance, at most 240), and lives 4 to
  10 s.
- The others start 60 units out in a random direction; their distance
  counts a further 0 to 50 units; they fly outward at
  10000 × (1 − d / 120)² and live 2 to 5 s with five times the spin.
- A puff appears `d × 0.0000767 + 0.1` s after the explosion. Each frame
  (time step at most 0.05 s) it moves, grows by the distance moved (from 8)
  and slows by `100 / (moved² + 100)`; its spin (random ±0.4 rad/s) falls
  to 0.3 of itself per second while above 0.2.
- Drawn screen-facing, colour from (0.35, 0.345, 0.33) to (0.75, 0.75, 0.7)
  over its life `(age − 0.25) / life`; alpha rises over the first 0.25 s to
  0.5, then falls with `(1 − t) × 0.5`, scaled to 0 within 50 units of
  the view and to 1 at 150.

## Not traced yet

- How the SpriteCard shader combines dual sequences (`$sequence_blend_mode`)
  and `$maxlumframeblend`, soft particles (`$depthblend`) and
  `$startfadesize`/`$endfadesize`.
- The library's noise function (we use our own gradient noise), the exact
  trail width (we draw trails a radius wide in all, which matches the
  look of sparks in the game) and the rope texture scale.
- Tractor beams, paint and code effects beyond the ones above.

## Place on ground and modulate sprites

- `Position Modify Place On Ground` traces down `max trace length` (with
  `include water`, water surfaces stop it too); a particle the trace does
  not stop still goes to the trace's end unless `kill on no collision`.
  GLaDOS's drag ripples (`glados_head_drag_mod`) are placed so: as her head
  rises they land far below it, and their initial distance to it fades
  them out.
- SpriteCard `$mod2x` multiplies what is behind by twice the colour, eased
  to grey (no change) by the alpha; `$addself` does not make such a sprite
  additive (`particle/wake/ring_wake_mod` has both).
