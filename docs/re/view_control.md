# Scripted views and attachment rigs

Facts behind `entities::viewcontrol` and the first map's container ride,
from `server.dll` and the real game (`getpos`, `script` printouts and
screenshots through the network console).

## point_viewcontrol

- `Enable` puts the player's view on the entity (the view entity replaces
  any other); spawnflag 4 holds the player still. `Disable` gives the
  player their own view back.
- `TeleportToView` moves the player so their eyes are at the view, facing
  its angles (pitch and yaw), then disables it. It acts whenever the camera
  had been enabled for a player, even if another view has replaced it
  since: in `sp_a1_intro1` the intro camera's `TeleportToView` 12 s in
  would pull the player back if the bed were pressed before then (the game
  does not allow that in play).
- The first map's wake-up camera rides `attach_1` of `ghostAnim`
  (`sp_a1_intro`, a whole-frame animation); its path matches the one the
  game reports to within the sampling step, ending at
  (-8709.2, 1690, 92), angles (-4.2, 64.8, 0).

## point_viewproxy

- The view sits at the proxy entity (`proxy`), offset by where the player
  stands relative to the view proxy's own spawn point when `offsettype` is
  2 (measured from 64 units above the feet), easing from the player's eyes
  onto the proxy over a second when it is 1, and at the proxy when 0.
- Its orientation is the proxy's (or the `proxyattachment` attachment on
  the proxy's parent): yaw whole, pitch and roll scaled by `tiltfraction`,
  with the player's own look on top. The entity's own origin follows the
  view, so other entities can aim at it (Wheatley's swivels target
  `camera_1`).
- `TeleportPlayerToProxy` moves the player to the view and disables it.
- `usefakeacceleration` (offset 944): each think the proxy's velocity is
  measured from its movement since the last think and its change taken as
  an acceleration; one whose square is below 20 or above 10000 counts as
  none. With `skewaccelerationforward` it is scaled by
  `0.75 − forward · direction × 0.35` (forward of the proxy entity). The
  player's base velocity then loses `100 × frametime` of its length and a
  `0.25 × frametime` share, and the acceleration × 3 × `accelerationscalar`
  is taken off it. The base velocity carries the player around the hidden
  box (`FL_BASEVELOCITY` keeps it for the next move), so with offset type 2
  the view sways with the container's jolts.
- `easeanglestocamera` (offset 952): the player's own pitch and yaw move
  15% of the way to the proxy's each think and the view takes only the
  proxy's roll.
- Second wake-up in `sp_a1_intro1`: `camera_1` (offsettype 2, tilt 0)
  through `camera_proxy` on `Actor_container_master`'s `vstAttachment`;
  the game reports the view at (-5758, 1938, 281), as here.

## Parenting to attachments

- `SetParentAttachment` / `SetParentAttachmentMaintainOffset` parent to the
  attachment frame (the first snaps onto it) and then set the entity's
  move type to none: its own movement stops and it only rides the
  attachment. A parent given as `name,attachment` at spawn does not do
  this.
- `func_tank` refreshes its parent matrix every think from the parent and
  its attachment, converts the target into it and turns its local angles
  (10x the remaining angle, capped at the rates; centres are the local
  angles at activation). Turning needs the tank's own movement, so a tank
  moved onto an attachment by input stays put.
- Wheatley's entry rig: `top_swivel_1` (yaw only) is moved onto
  `Actor_railing_1`'s `vstAttachment` (a bone frame rolled 90 degrees) and
  never turns; `bottom_swivel_1` under it does all the aiming. Measured
  10 s after the door opens: attachment angles (0, 91.2, 90), top swivel
  (0, 91.2, 0), bottom swivel (22.3, -19.5, 0) aiming at `camera_1`, and
  `@sphere` (0, -109.5, -22.3); the sphere model faces +Y.

## npc_personality_core

- The model is `models/npcs/personality_sphere/personality_sphere.mdl`
  unless `altmodel` is set (then `personality_sphere_skins.mdl`).
  `ModelSkin` picks the skin. Without `altmodel` the core also makes an
  `env_projectedtexture` on its `eyes` attachment (85 degree field of view,
  `effects/flashlight001_improved`, shadows) and turns it off.
