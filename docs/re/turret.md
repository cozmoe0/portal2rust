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

- Bullets (`DMG_BULLET`) only play `Flesh.BulletImpact` from the player's
  damage effects (server.dll `sub_101E7EC0`; crush and drowning get red
  and blue screen fades). The red the screen turns when hurt is a screen
  fade the post-processing shader takes as `$fadeColor`. Measured with
  `hurtme 30` (screenshots 0.25, 0.75 and 1.05 s later): it blends the
  displayed image toward about (45, 0, 0) at 0.39, 0.35 and 0.32, already
  fading before health starts coming back, so it follows the damage and
  time, not the health.
