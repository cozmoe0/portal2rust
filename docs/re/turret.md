# Floor turrets (`npc_portal_turret_floor`)

Facts gathered from the installed `portal2/bin/server.dll`; the
descriptions are our own.

## Thinks

The portal turret keeps the base floor turret's think set (search, auto
search, active, suppress, tipped, held, inactive, disabled, burn, break,
die, tractor beam) and adds held and tractor-beam thinks.

- Active think runs every 0.1 s. Without an enemy, or when disabled, it
  goes back to searching with its "last seen" time 4 s ahead and its goal
  angles reset to its own.
- It aims at the enemy's body target from its eye. When a linked portal
  gives a shorter (or the only) clear line to the enemy, it aims through
  the portal instead.
- While the enemy is visible, alive and within range, the turret fires
  when its muzzle direction is within 10° of the direction to the enemy
  (45° when the enemy is closer than 60 units, where only the horizontal
  direction has to line up), and keeps turning toward it.
- Visible but out of range (or blocked): "last seen" 2 s ahead and it
  switches to suppression fire. Not visible at all: "last seen" 4 s ahead
  (1.5 s with spawn flag 128) and back to searching.

## Shots

- One bullet per shot, 16384 units long, from the four muzzle attachments
  in turn, with a line-and-whiz tracer every shot.
- Ammo `PortalTurretBullet`: bullet damage 3 to players and to NPCs,
  carry 150.
- A turret whose up axis is more than about 25° from vertical (up z below
  0.9) gets pushed back by 30 × the shot direction with every shot.
- `NPC_FloorTurret.ShotSounds` plays per shot.

## Settings

- `sv_portal_turret_fire_cone_z_tolerance` 45: the vertical extent of the
  firing cone in degrees.
- `sv_portal_turret_shoot_at_death` 1, `sv_portal_turret_min_burn_time`
  1.0 and `max_burn_time` 1.5, `sv_portal_turret_shoot_through_portals_proximity`
  36864 (only shoot through portals at players within 192 units of the
  portal).

## Search think

- Runs every 0.1 s with the idle activity. When the AI has an enemy, the
  turret measures the distance to it (through a portal when that is
  shorter or the only clear line), schedules its first shot
  `(distance / range)² × 2.5` seconds ahead, deploys, switches to the
  active think and plays `NPC_FloorTurret.Activate` (at most every 3 s).
- Without an enemy, until its "last seen" time it scans, its goal angles
  wobbling up to 20° around its own; after that it retires.

## Voice lines

The turret speaks through `NPC_FloorTurret.Talk*` sounds, one per state:
search, auto search ("are you still there"), active, deploy, retire,
tipped, disabled, collide, pickup, shot at, dissolved, flung, burned.

- A line plays when the turret's state changes and the time for its next
  line has come; afterwards the next line waits 1.75 s (search, auto
  search, deploy), 3.5 s (retire), 1.15 s (tipped), 2.25 s (pickup) or
  10 s (dissolved). Changing to retire, dissolved, pickup or flung (and to
  tipped unless already handled) may speak at once.
- While firing, the active line repeats every 2.5 s.
- Gagged turrets (`EnableGagging`) stay silent.
- Bumping into something plays the collide line and holds off the next
  line for 1 s. A burning turret has a 1 in 4 chance of crying out when it
  catches fire, then burns for `sv_portal_turret_min/max_burn_time`.

## Tipped (server)

- Knocked over (`sub_102E35A0`, the side check), the turret sets its thrash
  time 2 to 2.5 s ahead, thinks every 0.05 s, sets its eye to "sees target"
  and sounds `NPC_FloorTurret.Alarm` (unless gagged).
- Tipped think (`sub_10327F40`): the tipped talk state, no enemy. Until the
  thrash time (and while not carried): every 0.05 s it plays its firing
  activity (`fire` for the first pair of muzzles, `fire2` for the second)
  and shoots from the muzzle (when `sv_portal_turret_shoot_at_death`), and
  its goal angles jump to its own plus a random −60° to 60° in pitch and
  in yaw, the head turning toward them.
- After that it faces ahead (open idle). Once the head is there it plays
  `NPC_FloorTurret.Die` and `NPC_FloorTurret.TalkDisabled`, closes
  (`retract`, `NPC_FloorTurret.Retract`) and takes its death; when the
  close has finished it plays `ACT_FLOOR_TURRET_DIE` (`eye_off`), its eye
  goes dead and it thinks slowly from then on.

## Eye glow (server)

`sub_10328910` keeps `sprites/glow1.vmt` in red (world glow, render fx no
dissipation) on the eye attachment: on its target brightness 255 and scale
0.4 within 0.1 s; searching it blinks each think between 224 at 0.3 and 192
at 0.2; dormant 192 at 0.2 over 0.5 s; dead 0 at 0.1 over 0.25 s; disabled
the same over 1 s.

## Muzzle flash (client)

`fire` and `fire2` carry `AE_NPC_MUZZLEFLASH` events at their first frame
(`COMBINE LFT_Gun1_muzzle` and `RT_Gun1_muzzle`, or the `Gun2` pair). The
`COMBINE` NPC flash (client.dll `sub_103FFC80`) is drawn in the muzzle's
frame with a scale of 1 to 1.5: five sparks of `effects/combinemuzzle1` or
`2` along the barrel (2 × scale apart, 50 to 150 units/s, 0.1 s, 6 to 8
units scaled and smaller further out), three spikes of five across it (up,
and down to each side, 1 × scale apart, a quarter of 50 to 150 units/s,
0.2 s, 2 to 4 units scaled), a glow of `effects/strider_muzzle` 12 to 16
units shrinking to nothing over 0.3 to 0.4 s, all white and fading out, and
an elight of colour (64, 128, 255) × 2⁵ and radius 32 to 128 for 0.05 s.

## Head and laser (client and server)

- The head turns through the `aim_yaw` (±60°) and `aim_pitch` (±15°)
  pose parameters toward the goal angles, at most `MaxYawSpeed` (360) ×
  0.05° per think, applied at two thirds.
- The client draws the laser sight as a beam with `effects/redlaser1.vmt`,
  colour (255, 32, 32) at full brightness, from the turret along its aim
  (client.dll `sub_10271940`): 0.75 units wide, with the server's halo
  `sprites/redlaserglow.vmt` at scale 4 where it ends.

## Model

- The server sets the model itself from `ModelIndex` (`sub_10324810`),
  whatever the map says: 0 `models/npcs/turret/turret.mdl`, 2
  `turret_boxed.mdl`, 3 `turret_backwards.mdl`, 4 `turret_skeleton.mdl`
  (all under `models/npcs/turret/`); 1 is unused. The entity definition's
  `models/props/turret_01.mdl` is a leftover whose material is self-lit.

## Player damage tint

Captured in the game under a turret's fire with `god` (`re/shots/turret_ref.txt`,
`re/tools/capture.py`): the screen still turns red, and holds at about
0.64 × the image + (22, 6.5, 7.3), a fade toward roughly (61, 18, 20) at
0.36.


- Bullets (`DMG_BULLET`) only play `Flesh.BulletImpact` from the player's
  damage effects (server.dll `sub_101E7EC0`; crush and drowning get red
  and blue screen fades). The red the screen turns when hurt is a screen
  fade the post-processing shader takes as `$fadeColor`. Measured with
  `hurtme 30` (screenshots 0.25, 0.75 and 1.05 s later): it blends the
  displayed image toward about (45, 0, 0) at 0.39, 0.35 and 0.32, already
  fading before health starts coming back, so it follows the damage and
  time, not the health.
