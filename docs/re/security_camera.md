# Security cameras

`npc_security_camera` as the installed `server.dll` runs it (checked in
IDA); our version is `crates/portal2/src/devices/security_camera.rs`.

## Spawn

- Model `models/props/security_camera.mdl` (no `model` keyvalue in the
  maps); collision bounds of ±16; `aim_yaw` and `aim_pitch` set to 0.
- Spawnflag 32 (Autostart) starts the search think; without it the camera
  holds still until an input starts it. 25 of the 38 single-player cameras
  autostart. The first think comes 0.1 to 0.3 s after spawning.
- The eye is the origin plus 10 units along the camera's forward axis (the
  origin sits on the wall).

## Thinks (every 0.1 s)

- Search: the camera sees all around (field of view -1); when the player is
  alive, passes the team filter and has a clear line from the camera's eye
  to the player's eyes (blocked by solid, line-of-sight blockers and movable
  objects; portals are also tried) it switches to tracking. It also keeps
  picking random goals (pitch -10..30, yaw -80..80 every 10 to 35 thinks)
  but does not turn while searching.
- Track: the target is the player's eyes plus an offset (x and y in -8..8, z
  in 0..32) that is re-rolled every 5 to 30 thinks while the player is
  within 256 units. With the target visible and within 1500 units the goal
  becomes: pitch = angle in degrees between the camera's down axis and the
  direction to the target; yaw from that direction projected onto the plane
  across the up axis, `acos(right·p) - 90` when it points forwards, else
  `-acos(right·p) - 90`, normalised to (-180, 180] (straight up or down
  keeps the previous yaw). Losing the player for 0.5 s returns to search
  without turning back.
- Turning: pitch and yaw each move towards the goal by at most 7 degrees per
  think (8 when following co-op pings), set as `aim_pitch` and `aim_yaw`;
  the client interpolates pose parameters, so the motion looks continuous.
  A movement sound fades to volume 0.7..0.9 while turning and out when
  stopped.
- Disabled or killed cameras turn to pitch 120, yaw 0 and stop thinking.

## Not done here

Damage and death (health, sparks, `NPC_SecurityCamera.Alert`), falling off
the wall when a portal opens on its mount or on `Ragdoll`
(`SecurityCamera.Detachment`), the movement sound, co-op pings and teams,
and the outputs.

## Knocked off by portals

While mounted, the camera tests the line from its origin out to its
radius in front of it and back to its radius behind it against the
portals (with a 1 unit tolerance). When that line meets an open portal
whose partner is open too, the camera detaches: `SecurityCamera.Detachment`
plays, it stops tracking for good and its physics object is set moving,
so it falls and can be carried.
