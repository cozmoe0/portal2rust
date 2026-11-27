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
- Launched objects fly without drag, so they reach the target and chains of
  plates hand them on. In sp_a2_trust_fling a cube from the dropper rides
  three plates into a portal on the first wall pad, comes out of one on the
  second pad, which relaunches it, and two more plates land it at
  `exit_landing_target`, where it slides off onto the lower floor (real
  game: rest at (-1761, 803, -366); ours: (-1669, 723, -366)).
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

## Portal gun view model states

- Holding: picking something up opens the prongs (server.dll
  `sub_103FDE90`, `OpenProngs`): the weapon's effect state becomes 2
  (holding) and the view model plays `ACT_VM_PICKUP` (`idle_to_carrying`),
  idling in `idle_carrying` (`ACT_VM_PICKUP_IDLE`). Letting go closes them:
  state 1 and `ACT_VM_RELEASE` (`carrying_to_idle`). The client starts the
  arcs between the prongs on state 2 (see particles.md, "Effects started
  by code").
- PotatOS (`m_bPotatos`): `logic_playerproxy`'s `AddPotatosToPortalgun` /
  `RemovePotatosFromPortalgun` (`sub_101958A0` → `sub_103FD550`),
  `upgrade_potatogun` (`sub_103FD6C0`) and the script function
  `UpgradePlayerPotatogun` (`sub_103498A0`, which also gives both
  portals) change it. A change holsters the gun (`ACT_VM_HOLSTER`, then
  hidden) and, two seconds later, sets the view model's and world model's
  `potatos_vmodel` body part to it and deploys the gun again (`draw`,
  `sub_103FD230`). The flag stays with the weapon across level changes;
  later maps do not add it again (`sp_a4_finale4` removes it).

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
- `logic_playerproxy`'s `SetDropEnabled 0` stops `+use` letting go (set on
  picking up Wheatley); it is a flag on the player (server.dll writes it in
  `sub_10195940`), set back to allowed only when the player spawns.
- View model holding (server.dll `sub_103CBCC0`, the player's flag at
  +0x1366 says it is in use): the hold point is the eye plus
  `forward × player_held_object_distance_vm` (65)
  `+ flat forward × player_held_object_look_down_adjustment (20) × t`
  `+ up × offset × (1 - t)`, with `t = |pitch| / 75` (clamped to 0..1),
  the eye's unclamped angles, the flat forward the player's up crossed with
  the view's right, and the offset `player_held_object_offset_up_sphere`
  (-15) for cores (`..._cube_vm`, -20, for cubes). A line trace from the eye
  (`MASK_SOLID`, skipping the player and the object) shortens it to where it
  hits, but never below `player_held_object_min_distance` (50). The object
  is put there whatever is in the way (no shadow controller), so it can sit
  inside a solid: measured in `sp_a1_intro7` (Wheatley's centre inside his
  socket's arm and housing) and `sp_a1_wakeup` (inside the breaker train's
  central block, at exactly 50 from the eye, where the socket's trigger
  is). Measured on the walkway outside the breaker room at pitch 10 and 38
  the core sits at the formula's point to 0.1 unit.
- `logic_playerproxy`'s `ForcePhysicsGrabController` /
  `ForceVMGrabController` / `ResetGrabControllerBehavior` store 2 / 1 / 0
  on the player (+0x1370); the controller is chosen with it on the next
  pickup, so the core already held stays a view model clone.
- Personality cores (`npc_personality_core`) are physics props: hanging on a
  parent they ride it, and `ClearParent` lets them fall (Wheatley popping
  off his management rail is the `@sphere ClearParent` the scene fires).
  `EnablePickup` / `DisablePickup` switch carrying on and off,
  `ForcePickup` puts the core in the player's hands (`sp_a1_wakeup` starts
  so) and `PlayAttach` / `PlayLock` play
  `scenes/npc/sp_proto_sphere/sphere_plug_attach.vcd` / `sphere_plug_lock.vcd`.
  Triggers that admit NPCs (spawnflag 2) take them.

## Finding what `+use` uses (`CBasePlayer::FindUseEntity`, server.dll sub_10105820)

- Eight traces from the eye, against MASK_SOLID plus debris and player
  clip (0x0601400B). The first is a line 1024 units along the view. The
  rest are 16-unit hulls, 72 units long, aimed at 45, 30, 20, 15 and 10
  degrees below the view and 10 and 15 degrees above it.
- A hit that cannot be used passes to its move parents until one can.
  It is taken if it is within 100 units. The distance is measured
  horizontally from the trace start, plus how far the end lies above or
  below the player's box. The forward line wins outright. A hit from a
  later trace is kept only as the starting guess for the next step.
- Next, a usable ground entity flagged use-on-ground (0x10), then a sphere
  query of 100 units for entities flagged use-in-radius (0x20). A
  candidate's nearest point to the eye must be at least 0.8 (cosine) along
  the view. The candidate nearest the view line wins, if a line trace to
  that point is clear or ends on it. Last, a 100-unit trace for player
  allies.
- Usable means ObjectCaps (vtable +156) has a use type in its low four
  bits, plus the required flags (vtable +1764 on the player).
  - `CBaseButton` and `CRotButton`: 0x21 with spawnflag 1024, locked or
    not. `CMomentaryRotButton` also sets 0x2.
  - `prop_button` (`CPropButton`): 0x1 only, so it is never found by
    radius.
  - `CPhysicsProp`: 0x1 when it can be picked up or has spawnflag 0x100,
    and 0x20 with spawnflag 0x4000. `CPropWeightedCube` adds 0x1, and 0x20
    while an inner object's check (+0x714, vfunc 16) is false.
  - `prop_dynamic`: nothing.
- `sp_a3_01`'s push bar door behind the vault depends on the radius
  pass. Its `func_button` is two 4-unit grate strips at the door's edges,
  so aiming at the door's middle hits only the door model.
- Ours keeps its 80-unit box trace and catching. A hit that cannot be
  used goes to a usable parent, and then to the radius pass over use
  buttons. Props and the hull traces are left out of the radius pass.

## Weighted cubes (`CPropWeightedCube`)

From server.dll (`sub_103EEEE0` sets the model, `sub_103EE8E0` the skin,
`sub_103EE870` reads old skins, `sub_103ECB40` the carry angles).

- The server sets the model itself, whatever the map says, from the cube
  type: 0 standard and 1 companion `models/props/metal_box.mdl`,
  2 reflective `models/props/reflection_cube.mdl`, 3 sphere
  `models/props_gameplay/mp_ball.mdl`, 4 antique
  `models/props_underground/underground_weighted_cube.mdl`.
- Without `NewSkins` the type comes from the old `skin` keyvalue: skins
  above 1 are one more than the type (so 3 is reflective).
- The skin follows the type, `SkinType` (rusted), the paint power (0
  bounce, 2 speed) and whether the cube is activated:
  - standard: rusted 3 (5 activated); bounce 6 (10); speed 7 (11);
    otherwise 0 (2);
  - companion: bounce 8, speed 9, otherwise 1 (4 activated);
  - reflective: bounce 2, speed 3, otherwise 1 rusted, 0 clean;
  - sphere: bounce 2, speed 3, otherwise 0 (1 activated);
  - antique: bounce 1, speed 2, otherwise 0.
- A floor button's touch sets a cube it takes activated as it starts
  touching and clears it as it stops (`sub_103DE610` / `sub_103DE670`).
- Reflective cubes have preferred carry angles of zero relative to the
  player, so they are held facing the view; a laser that hits one goes on
  from its centre plus 22 units along its facing, along its facing.

## Portal placement checks (server)

The placement check runs in this order and stops at the first failure:

1. A trace through the portal centre along its normal must hit a surface
   behind it; moving surfaces are refused; a no-portal material refuses
   the shot, a pass-through material lets it go on.
   - Moving (`sub_10379A50`): the entity hit has a velocity (its physics
     object's, when it has a moveable one, above 0.1 squared), or it or
     anything up its parent chain has an angular velocity. Entities moved
     by movers count; a brush carried by an animation (a panel on an arm's
     attachment) does not. Brush entities standing still take portals like
     the world (`sp_a2_intro`'s arm panels).
   - No-portal (`sub_10354E00`): the surface flag `SURF_NOPORTAL` (0x20,
     set by `%noportal` materials), a surface property of game material
     `Y` (glass), or a model's surface (`**studio**`). Nothing else is
     refused: tool faces take portals (`sp_a2_intro`'s panel wall is a
     solid detail brush faced with `tools/toolsplayerclip`, flags 0x480,
     while `tools/toolsnodraw` carries 0x20).
   - Pass-through: sky faces (`SURF_SKY`) and `lights/light_orange001`.
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
   and no-portal volumes, all corners on surface) must pass. The corner
   check (`sub_10356BA0`) traces at the four corners of the opening's
   rectangle, 2.2 units in, and at the centre: each needs a surface behind
   it that is not a `func_door`, a no-portal or a pass-through material.

Ours fits the portal by sampling a grid over the oval opening and those
four corners and pushing it away from failing samples, which lands near
the game's fit but not on it (`sp_a2_intro`, a wall whose top corner
overhangs: the game moves the portal 5.4 units sideways, ours 3.4 sideways
and 7 down).

### Resting surface check (`CPortal_Base2D::TestRestingSurfaceThink`)

0.1 s after a portal is placed or activated it traces from each corner of
its rectangle (2.2 units in) to 1 unit behind its plane (mask
`SOLID | WINDOW | GRATE | MOVEABLE`, ignoring portals). A corner with
nothing behind it ("Surface removed from behind portal") closes the
portal. The check repeats every 0.1 s while any corner rests on something
other than the world (a brush entity that may move away, be disabled or
be killed) and stops once all four rest on the world.

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

## Fizzling (`trigger_portal_cleanser`, weighted cubes' `Dissolve`)

Both a field touching a prop and a `prop_weighted_cube`'s `Dissolve` input
run `CTriggerPortalCleanser::FizzleBaseAnimating` (server.dll
`sub_10397480`; the cube's `InputDissolve`, `sub_103E4E20`, calls it with
no field). Other props have no `Dissolve` input at all (only
`env_entity_dissolver`, the cube, `prop_exploding_futbol` and
`prop_hot_potato` declare one).

- A field fizzles only what passes its trigger filters
  (PassesTriggerFilters, via the touch at server.dll `sub_10397930`):
  - objects need spawnflag 8 (physics) or 64 (everything), or 2 for NPCs;
  - the field's filter entity must pass.

  Most fields are 9 or 65. The 4097 fields (players only, 17 in the
  campaign) take the player's portals but let props through:
  `sp_a3_bomb_flings` flings paint bombs through three of them to
  `catapult_target`. Checked in the game (2026-11-26): the bomb comes out of
  the 50° panel's portal at 883 u/s, the unnamed catapult re-aims it ("needed
  between 781.3 and 1128.5"), and it flies past the field.
- Classes in a fixed list never fizzle: `func_door`,
  `func_door_rotating`, `prop_door_rotating`, `func_tracktrain`,
  `env_ghostanimating`, `physicsshadowclone`, `prop_energy_ball`,
  `weapon_portalgun`, `weapon_paintgun`. A personality core only runs its
  `OnFizzled()` and stays.
- From a field, a prop named `box` fires the field's `OnDissolveBox`
  (activator the prop).
- A carried prop is dropped first and the copy moves with the player's
  velocity plus 4 units along the view across the ground
  (`EyeDirection2D`, x and y only) and 32 down; otherwise it keeps the
  prop's velocity and angular velocity.
- `OnFizzled()` fires the prop's `OnFizzled` (activator and caller the
  prop itself), game stats note the fizzle, and the prop is replaced by a
  copy (`simple_physics_prop`, or `simple_physics_brush` for brush models)
  with its model, skin, render colour and placement, and the original is
  removed at once (so the map can no longer address it). The copy is not
  solid, ignores gravity, moves at half the velocity and gets 20°/s more
  angular velocity about its local z (the physics object's angular
  velocity is local, in degrees).
- A Schrödinger cube's linked cube fizzles with it (cube type 5).
- With the dev-only cheat `swap_model_for_fizzler_fx` (default 0) the copy
  would use `<model>_fx_fizzler.mdl` (`metal_box_fx_fizzler.mdl`, a
  107-bone shard model with `fizzler_anim_a`) and an `env_fizzler_effects`;
  shipped, the models match and the copy `Dissolve`s instead
  (`CBaseAnimating::Dissolve`, `sub_100C5A60`, normal type), which plays
  `Prop.Fizzled` on it. From a field, the field's `OnDissolve` fires last
  (activator the prop).
- The dissolver (`env_entity_dissolver`, `CEntityDissolve::Spawn`
  `sub_101397F0`) sets fade-in start 0 and length 1, model fade-out start
  1.9 and length 0.1, fade-out start 2 and length 0. Its think
  (`sub_10139470`) removes the target and itself once 2 s have passed.
- Client (`C_EntityDissolve`): on creation, with `cl_portal_use_new_dissolve`
  (default 1), it starts the particle system `dissolve` on the target
  (`PATTACH_ABSORIGIN_FOLLOW`; `particles/spark_fx.pcf`: sparks, glow,
  black ash and ground flecks placed on and locked to the model's
  hitboxes) and draws none of the old sprite sparks (`DrawModel` returns
  at once). Its `ClientThink` (`sub_1009AD00`) gives the target render mode
  `kRenderTransColor` and render colour grey `(1 - fade in) × 255`, alpha
  `model fade out × 255` (1 until 1.9 s, then down to 0 at 2.0 s). The
  fade-in is 0 before its start, `(t - start) / length` strictly inside
  the window and 1 otherwise (so exactly at the start too). Models take
  the render colour into linear space (gamma 2.2), so the model blackens
  quickly.

## Stuck players

- `CGameMovement::CheckStuck`: a player whose box starts inside something
  tries one small nudge from a fixed table (an eighth of a unit, then 1 or
  6 up, 2 sideways) and otherwise does not move that tick, keeping their
  ground entity, so a pusher they stand on keeps carrying them. In
  `sp_a1_wakeup` Wheatley, riding his socket on the shaft model's animation
  while the breaker train turns a little differently, sweeps through where
  a player can stand; the player stays on the train.

## Physics-solid brush entities

- Brush entities the game makes physics-solid (doors unless under a
  BSP-solid parent, trains without spawnflag 128, `func_brush` without
  `solidbsp`, `func_movelinear`...) are traced through the map's physics
  hulls (`LUMP_PHYSCOLLIDE`), which vbsp builds half a unit inside the
  brushes. Measured in `sp_a1_wakeup`: the player stands on the breaker
  train at −476.47, its brush top being −476 (hull top −476.5, less the
  trace's 1/32 keep-off). The inset is what lets the player step from the
  catwalk onto the train: the train's hull ends 0.54 units short of the
  doorway lintel, so the player is already under the lintel when the step
  up is tried and rises only to it. The game's walk ends at
  (8976, 1115.53, −476.47), as here.
- In `sp_a1_wakeup` the carried Wheatley is held inside the breaker
  train's central block, where the socket's trigger sits: he is held as a
  view model clone, which no solid stops (carrying, above), not because of
  the train's "unblockable" flag.

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
- Opening plays `prop_portal_door.open` and closing `prop_portal_door.close`
  on the door (server.dll `sub_103EB260` / `sub_103EA9F0`), after the
  `OnOpen` / `OnClose` output.

## Device sounds

From server.dll.

- Lasers: each laser (the emitter's and each one a reflection cube sends
  on) loops `Laser.BeamLoop` (`LaserGreen.BeamLoop` for the green kind)
  from a sound entity the laser keeps for the player, on the beam
  (`sub_103B19A0`).
- Laser catchers and relays powering up play `prop_laser_catcher.poweron`,
  loop `prop_laser_catcher.powerloop`, start `laser_relay_powered` at their
  `particle_emitter` attachment and take their lit skin; powering down
  plays `prop_laser_catcher.poweroff` and stops the loop
  (`sub_103C61F0` / `sub_103C62C0`). Catcher skins: clean 0 / 1, rusted
  (`SkinType`) 2 / 3.
- What a beam powers is a box, not the model. Catchers and relays make an
  internal `point_laser_target` at their model's `laser_target`
  attachment (`sub_103C59F0`; a relay's is not a terminal point), and each
  stretch of each beam is tested against every target's box (a swept box
  and a ray-box test, `sub_103B40E0`); the beam's own trace skips
  `point_laser_target`. The boxes are world-aligned (`sub_103C5B10`):
  not terminal, 10 / 10 / 17 half extents (so a beam through the hollow
  middle of `laser_receptacle.mdl`, which collides only at its feet and
  cap, powers the relay and runs on); terminal, 11 each way, except a
  target owned by `laser_catcher_center.mdl`, where they are
  |forward × 15 + (right + up) × 20|.
- Fizzlers: `VFX.FizzlerStart` when one comes on, `VFX.FizzlerDestroy` when
  it goes off; all of them share one `VFX.FizzlerLp` through a
  `fizzler_multiorigin_sound_player` (`sub_103970D0`).
- Tractor beams loop `VFX.TbeamEmitterSpinLp` at the emitter and
  `VFX.TBeamPosPolarity` / `VFX.TBeamNegPolarity` by their direction
  (`sub_103EC0E0`); light bridges loop `VFX.BridgeGlow` (`sub_103F0BE0`).

## Switchable lights (`light`, `light_spot`)

- Named lights carry their own light style (`style`, 32 and up). At spawn
  a light starting dark (spawn flag 1) sets its style to `a`, otherwise to
  its `pattern` or `m`. `TurnOn` sets the pattern (or `m`) and clears the
  flag, `TurnOff` sets `a`, `Toggle` swaps, `SetPattern` takes a new
  pattern, `FadeToPattern` steps the style one letter every 0.1 s from the
  current pattern's first letter to the new one's, then sets the pattern.
