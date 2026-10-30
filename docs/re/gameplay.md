# Gameplay

Facts about Portal 2's gameplay code gathered from the installed
`portal2/bin/server.dll` and `client.dll`; the descriptions are our own and
the code in `crates/portal2` is written from them.

## Faith plates (`trigger_catapult`)

- On touch an object is launched once; an object the plate turns down is
  ignored for 0.1 s before it is considered again.
- Toward `launchTarget`: the flight time is the straight-line distance from
  the object's origin to the target over the speed (`playerSpeed` for
  players, `physicsSpeed` for objects), `t = |target - origin| / speed`,
  and the velocity is `(target - origin) / t` with `sv_gravity × t / 2`
  added upward. For players the target is taken 32 units lower, so their
  middle rather than their feet passes through it. The plate's debug
  overlay draws the same numbers ("Adjusted Player velocity").
- `useExactVelocity`: the launch has exactly the speed and follows one of
  the two ballistic arcs through the (lowered) target, from the usual
  `atan((v² ± √(v⁴ - g(g x² + 2 y v²))) / (g x))`. `exactVelocityChoiceType`
  1 takes the first (steep) arc, 2 the second (flat) one, 0 the one closest
  in direction to the object's current velocity. When the target cannot be
  reached at that speed (or lies straight above), the plain launch is used.
- `useThresholdCheck`: toward a target, the object must be heading within
  `EntryAngleTolerance` (a dot product) of the direction to the target and
  moving faster than `(1 - lowerThreshold)` and slower than
  `(1 + upperThreshold)` times the launch speed; otherwise it is turned
  down. Without a target the object's velocity along `launchDirection` must
  lie within the same bounds around the plate's speed. When it passes,
  `onlyVelocityCheck` fires `OnCatapulted` without launching.
- Along `launchDirection` (no target): players get the direction × the
  speed, with the vertical part replaced by 1.5 × the speed when the
  direction is straight up; objects get the horizontal part × the speed
  and always the full speed upward, plus a random spin of up to 50°/s per
  axis. Toward a target objects spin up to 150°/s per axis when
  `applyAngularImpulse` is set.
- Launched players leave the ground (and ignore it for 0.5 s) and lose air
  control for `AirCtrlSupressionTime` seconds, 0.25 when that is not
  positive; launches along a direction only do so with
  `DirectionSuppressAirControl`.
- Defaults (entity definitions): speeds 450, `lowerThreshold` 0.15,
  `upperThreshold` 0.30, `EntryAngleTolerance` 0, `applyAngularImpulse` 1.

## Air movement

- The air wish velocity uses the full view direction: its forward axis is
  flattened onto the ground plane and only rescaled to unit length when
  the view's vertical component is under 0.5 (looking less than 30° up or
  down), so forward steering weakens when looking steeply. The side axis
  is always flattened and rescaled.
- Above 300 u/s of horizontal speed, a wish component is dropped when it
  pushes against an axis the player moves along faster than 150 u/s, so
  a fling cannot be braked by steering.
- While air control is suppressed (after a faith plate launch) the wish is
  zeroed, but the floor-portal pull below still applies.
- Air acceleration builds speed along the wish direction up to 60 u/s (not
  the usual 30), `sv_paintairacceleration × wish speed × dt` per tick
  (5, not `sv_airaccelerate`'s 10), the wish speed at most the player's
  maximum speed (raised by speed gel, see paint.md).

## Funneling into floor portals (`sv_player_funnel_into_portals`)

Checked each air tick while the horizontal speed is at most 300 u/s, over
every open portal whose partner is open too:

- The player qualifies only while not steering (both horizontal wish
  components within ±64) and either rising faster than 165 u/s or falling
  faster than 165 u/s while looking down (view forward z below -0.7).
- Falling: floor portals (forward z above 0.7) 0 to 1024 units below the
  player's centre (origin plus half the hull height). Rising: ceiling
  portals (forward z below -0.7) up to 1024 units above and no higher than
  the top of the jump, `vz² / (2 sv_gravity)`.
- The player's centre must lie over the opening grown by a factor from 1.5
  (256 units away or closer) to 3 (1024 or further): within `32 × f`
  along the portal's width axis and `56 × f` along its height axis. The
  nearest such portal is used.
- The time `t` until the player's centre is `sv_player_funnel_height_adjust`
  (128) above the portal comes from `h + vz t - g t² / 2 = 0` (the positive
  root, the smaller one when both are positive); no real root, no pull.
- Per horizontal axis with offset `d` to the portal centre and velocity
  `v`: if `v` is 0 or `t <= d / v` (the player would not reach the centre
  in time) the wish gains `6 d k - v`, with `k = 1 + (speed_bonus - 1) ×
  clamp(vz / 1065, 0, 1)`; otherwise (would overshoot, or moving away) the
  velocity on that axis is zeroed when within `snap_threshold` (10) and
  else halved every `0.01 + 0.14 × clamp((|dz| - 128) / 896, 0, 1)`
  seconds.
- Leaving through a portal that faces straight up or down (within 0.001),
  a velocity within `sv_player_funnel_gimme_dot` (0.9) of vertical keeps
  only its vertical part.
- `sv_player_funnel_well_above` is registered but not read by this code;
  walking funnels (`speed_funnelling_enabled`) only apply on speed gel.

## Prop funneling (`sv_props_funnel_into_portals`)

Each physics tick, for props with `allowfunnel` (1 unless the keyvalue or
`DisablePortalFunnel` turns it off), moving vertically faster than 1 u/s
and horizontally slower than 64 u/s:

- The target is the nearest open floor portal (forward z above 0.8) 0 to
  1024 units below the prop's origin, with the origin within 1.5 × the
  half width and half height of its centre along its axes.
- Falling: the horizontal velocity moves toward the offset to the centre
  (as a velocity) by `slow × high` of the difference per tick, `slow =
  clamp(1 - speed² / 128², 0, 1)` and `high` rising from 0 at 128 units
  above the portal to 1 at 256.
- Rising: when the horizontal velocity points away from the centre, the
  velocity gains the horizontal offset vector times the outward speed over
  the offset length, at most `deceleration` (2) × the tick length.

## Portal gun bob (client)

- The bob's time base advances by the elapsed time × `speed / max speed`,
  where `speed` is the player's full 3D speed clamped to their maximum
  speed (175). `cl_bob`, `cl_bobcycle` and `cl_bobup` are registered but
  not read.
- Vertical bob: the fraction of the time base through a 0.45 s cycle,
  mapped to an angle (`π f / 0.5` in the first half, `π + π (f - 0.5) / 0.5`
  in the second), gives `b × 0.3 + b × 0.7 × sin(angle)` with `b = speed ×
  0.005`, clamped to -7..4. The sideways bob is the same over a 0.9 s
  cycle.
- The gun moves `vertical × 0.1` along the view direction and along the
  player's up axis and `sideways × 0.8` along the view's right axis, and
  turns by `vertical × 0.5`° of roll, `-vertical × 0.4`° of pitch and
  `-sideways × 0.3`° of yaw (as turns about its own axes).

## Carrying objects (`CGrabController`)

- Single player uses the physics grab controller for everything except
  personality cores, which are held as view model clones
  (`player_held_object_use_view_model` -1 leaves the choice to this rule).
- The view used for holding has its pitch clamped to ±75° (-90° upward
  when the object may be held overhead).
- Hold distance along that view from the eye: the object's radius plus the
  player's horizontal hull radius (22.6) plus `player_held_object_distance`
  (15) plus a per-object offset. With `player_hold_object_in_column` (1)
  the distance is instead how far along the view a vertical plane that
  distance in front of the player lies, clamped between
  `player_held_object_distance` and `player_hold_column_max_size` (96).
- Weighted cubes are held `player_held_object_offset_up_cube` (-10) along
  the view's up axis, scaled by `1 - |pitch| / 75`; other objects get no
  offset.
- A trace from the eye toward the hold point shortens the distance to
  where it hits, but never below the two radii.
- Dropping: the error is an average of the distance between the object and
  its target, `error = (1 - t) error + t d` with `t` the time since the
  last update (at most 1) and `d` halved when it implies more than the
  controller's top speed. While the line from the eye to the object is
  blocked the error triples every update (also ×2.5 when held through a
  portal that is no longer looked through). The object drops when the
  error exceeds 40 (12 with `player_held_object_collide_with_player`), or
  when the player stands on it.

## Portal placement checks (server)

The placement check runs in this order and stops at the first failure:

1. A trace through the portal centre along its normal must hit a surface
   behind it; moving surfaces are refused; a no-portal material refuses
   the shot, a pass-through material lets it go on.
2. Unless `sv_portal_placement_never_bump`, the portal is fitted onto the
   surface with its half width and height reduced by 2: traces from the
   centre toward each corner, just off the surface, find obstructions and
   the edge of the portalable area (refined by bisection), and the portal
   is moved away from them, recursing at most 6 times. A fit that ends
   further than `√(((2 × half height)² + (2 × half width)²) / 2)` (about
   91 units) from where the shot landed fails.
3. A wall portal (up axis above 0.7 vertical) whose centre is within half
   its height + 1.5 units of a floor below, but further than half its
   height - 0.1, drops so its bottom edge rests on that floor (traces are
   against brushes only).
4. It must not overlap the other portal, and the remaining checks (fizzler
   and no-portal volumes, all corners on surface) must pass.

## Screen shake (`env_shake`)

- Server: the amplitude is clamped to 16. For each player, a starting
  shake is skipped while they are airborne unless the shake has the
  "in air" flag (other commands always go through). With a radius the
  player's amplitude is `(1 - d / radius) × amplitude` for a hull-centre
  distance `d` within it, and none outside; without one (or with the
  global flag) the full amplitude. Commands are start, stop (clears every
  running shake), and new amplitude or frequency (applied to the newest
  running shake).
- Client: at most 32 shakes run at once. Every `1 / frequency` seconds a
  shake picks a new random offset (each axis within ±amplitude) and roll
  (within ±amplitude / 4). Each frame it contributes that offset and roll
  times `sin(now × frequency / f) × f²`, where `f` is the remaining share
  of its duration (the wobble speeds up as it fades), and its amplitude
  decays by `amplitude × frame time / (duration × frequency)`. Expired
  shakes and those with no amplitude, frequency or duration are dropped.
  The summed offset moves the view and the summed roll tilts it.

## Health

- With the game rules' `player_regenerates_health` set, a hurt player
  gains one point of health every think (each 60 Hz tick) once
  `sv_regeneration_wait_time` (1.0 s) has passed since the last damage.

## Fizzling (`trigger_portal_cleanser`)

- A prop touching an active field is replaced by a non-solid copy of its
  model and the original is removed at once (so the map can no longer
  address it). A carried prop is dropped first and the copy moves with
  the player's velocity plus 4 units along the view and 32 down;
  otherwise it keeps the prop's velocity. The copy ignores gravity, moves
  at half that velocity and spins 20°/s faster about the vertical axis.
- The copy dissolves with the standard dissolve timing: the effect fades
  in over 1 s, the model fades out from 1.9 to 2.0 s and the copy is
  removed at 2.0 s. `Prop.Fizzled` plays.
- Personality cores are handled separately, and a list of classes never
  fizzles.

## Bone followers

- Dynamic props (`CDynamicProp`), floor buttons (`CPropFloorButton`) and
  test chamber doors (`CPropTestChamberDoor`) build their collision the same
  way: one `phys_bone_follower` per bone listed in the model's
  `bone_followers` keyvalues, or when the model lists none and its `.phy`
  holds more than one solid, one per solid on the bone it names. The
  followers move with the animated bones, so the exit elevator's
  `spinnydoor_left` / `spinnydoor_right` solids open with `dooropen` and a
  floor button's `portal_button_top` plate goes down with `down`.

## Test chamber doors and their area portal window

- A door's `AreaPortalWindow` names a `func_areaportalwindow`. When the door
  spawns it sets the window's fade distances to 0 and 0; when it starts to
  open (as it plays `prop_portal_door.open`) to 10000 and 10000, or with
  `UseAreaPortalFade` to its `AreaPortalFadeStart` / `AreaPortalFadeEnd`;
  once a closing animation finishes, back to 0 and 0.

## Switchable lights (`light`, `light_spot`)

- Named lights carry their own light style (`style`, 32 and up). At spawn
  a light starting dark (spawn flag 1) sets its style to `a`, otherwise to
  its `pattern` or `m`. `TurnOn` sets the pattern (or `m`) and clears the
  flag, `TurnOff` sets `a`, `Toggle` swaps, `SetPattern` takes a new
  pattern, `FadeToPattern` steps the style one letter every 0.1 s from the
  current pattern's first letter to the new one's, then sets the pattern.
