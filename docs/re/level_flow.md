# Level flow

How a single-player level ends and the next one starts, from the installed
`server.dll` (IDA), the map entity lumps and the installed transition
scripts. Written in our own words; no code is copied.

## Exit elevator

- The exit elevator is a `func_tracktrain` with
  `transitions/sp_elevator_motifs.nut`. Its start trigger closes the doors and
  runs `StartMoving()`, which sets the ride speed from the script's per-map
  table (300 when the map is not listed).
- Each `path_track` of the shaft runs `ReadyForTransition()` on the elevator's
  `trigger_teleport` when passed. For maps in the table without "motifs" the
  level only ends there once `::TransitionReady` is 1, which nothing sets in
  the campaign (the challenge-mode `OnLevelStart` path is the only caller);
  the ride therefore ends at the bottom node, whose `FailSafeTransition()`
  triggers `@transition_from_map`. Maps missing from the table end at the
  first node. The bottom node leads to a node with the teleport flag, so the
  train loops back to the top while the screen fades.
- `@transition_from_map` fades out and, 0.3 s later, `@exit_teleport` puts the
  player into `transition_trigger`, which runs `TransitionFromMap()` in
  `@transition_script` (the map's `logic_script` running
  `transitions/sp_transition_list.nut`).

## TransitionFromMap and portal_stats_controller

- `TryDLC1InstalledOrCatch()` returns true and never throws in the installed
  game, so `TransitionFromMap()` takes its first branch: it fires
  `OnLevelEnd` (parameter 5) at the entity class `portal_stats_controller`
  and does not call `RealTransitionFromMap()` itself.
- The game creates one `portal_stats_controller` per map when none exists.
  Its `OnLevelEnd` records the end time (now, plus the parameter only in
  challenge mode) and schedules a think 0.1 s later. The controller thinks
  every 0.1 s; once `curtime` is past the end time it looks for an entity
  named `@transition_script`, then `script_check_finish_game`, then the first
  `logic_script` whose name contains `transition_script`, runs
  `RealTransitionFromMap()` in that entity's script scope, and removes
  itself.
- `RealTransitionFromMap()` finds the next map in `MapPlayOrder` and fires
  `Changelevel` at `@changelevel` (a `point_changelevel`), or uses the `map`
  command when that entity is missing.
- `OnLevelStart` (fired by `OnPostTransition()` and, three seconds after
  spawn, by the controller itself on `sp_a2_intro` and
  `sp_a4_speed_tb_catch`) only matters for challenge mode.

## Arrival

- The new map's trigger at the landing spot runs `OnPostTransition()`, which
  fires `Teleport` at `@elevator_entry_teleport` and `@arrival_teleport`;
  the latter puts the player into the arrival elevator, whose interior trigger
  sends the train to its bottom node.

## func_tracktrain

- `Spawn` makes the train BSP-solid with spawnflag 128, physics-solid
  otherwise; spawnflag 512 ("unblockable by player") sets an entity flag,
  and spawnflag 8 makes it not solid.
- `MoveToPathNode` sets the destination and starts the train at the speed of
  the node it is at, or its maximum speed when that node has none,
  whatever speed it last moved at. `SetMaxSpeed` only changes the maximum.
  `sp_a1_wakeup`'s breaker room makes a one-unit move at its starting
  maximum of 1, raises the maximum to 52 and later rides up the shaft with
  another `MoveToPathNode`, so it climbs at 52 (155 past `breaker_train_path_2`).
- `orientationtype` 2 (linear blend) and 3 (eased) turn the train from the
  orientation of the node it left to that of the node ahead with the share
  of the segment covered (a slerp, so the short way round); a node's
  orientation is its own angles with its `orientationtype` 2, else the
  direction to the next node. Type 1 faces the way it moves and keeps its
  angles while moving straight up or down. Measured in `sp_a1_wakeup`
  (nodes at yaw 0, 270, 0, 135): the breaker train turns 0 → −108 on its
  one-unit move, back to +8 on the first climb and to 131 at the top (the
  game's think steps overshoot and undershoot the nodes' −90 / 0 / 135),
  while the shaft model's own animation turns the socket a quarter turn
  the same way.

## linked_portal_door

- A fixed portal opening (`width` x `height`, facing along its angles),
  open from the start with `startactive`, toggled with `Open`/`Close`, and
  linked to the door `partnername` or `SetPartner` names. `sp_a1_wakeup`
  links and opens `incinerator_entry_portal_1` (a floor opening in the
  incinerator) to `incinerator_entry_portal_2` (a ceiling opening above the
  shaft whose trigger ends the level); `sp_a4_finale2` has two more pairs.

## Parenting to the player

- `SetParent` looks its parameter up the way the engine's entity search
  does, so `!player`, `!activator` and `!caller` work. The child keeps its
  world placement, and its offset is taken in the player's frame at that
  moment.
- The player's own angles are the view's yaw alone (pitch is zeroed after
  each move), and what hangs from the player turns with that yaw.
- `sp_a3_00` parents `player_looktarget` to `!player` on map spawn. The
  potato rig (`potatos_tank`, a `func_tank`) aims at that target, so the
  potato stays on the target's side of the player as they turn.
  `sp_a2_core` teleports the player to its exit landmark at yaw 90 before
  the change, so after a real playthrough the target (and the potato) sits
  90 degrees to the player's right. Checked in the game: at yaw 45 the
  target was at (89.8, -4208.8), at yaw 135 at (113.9, -4005.2).
  The potato is only glimpsed while turning, as the tank's turn lags. Loading
  the map directly (yaw 0) puts it in front.
- The broken elevator above (`elevator_escape.mdl` on `lift_rotator_2` /
  `lift_rotator_1` / `lift_mover`) is meant to be there. It rises at
  5 units/s, spins and tilts, and starts about 416 units above the player.

## point_teleport

- On activation it saves its own absolute origin and angles; with spawnflag 1
  ("teleport home") it saves its target's instead, and removes itself (with a
  warning) when that target does not exist.
- `Teleport` moves the first entity named by `target` to the saved
  placement, `TeleportEntity` does the same for the entity named in the
  parameter, and `TeleportToCurrentPos` uses the teleport's current
  placement. Velocity is left alone. A target with a parent is refused with
  a warning, and a prop held by a player is dropped first.

## trigger_teleport

- Without a landmark (and without spawnflag 32, "preserve angles") the
  toucher takes the destination's angles and stops; with a landmark both
  view and velocity carry over.

## Collision of studio models

- A `.phy` solid's convex pieces ("ledges") index one point array shared by
  the whole solid; each piece is the hull of the points its own triangles
  use.
- Props that are physics-solid (`solid` 6) collide through their `.phy`
  only: without one they do not block anything. Only box-solid props
  (`solid` 2) use the model's hull box.

## Triggers and the player's fog

- A player in noclip still touches triggers: measured in `sp_a1_wakeup`,
  teleported in noclip into the trigger past the hall (which enables
  `hub_catwalk_door_reopen_trigger`) and then into that one, the training
  door opens as when walking.
- `SetFogController` on the player with a name no entity has leaves the fog
  as it was (`sp_a1_wakeup`'s `relay_hub_fog_and_tonemap` names a
  `fog_hub` the map lacks).
- Switching to another controller does not snap: the view blends from the
  fog it showed to the new controller's colours, distances, maximum density
  and HDR scale over the new controller's `foglerptime` (the Source SDK's
  `C_BasePlayer::FogControllerChanged` / `UpdateFogBlend`; not traced in this
  build). In `sp_a2_core` the real game goes red over 5 s at the core
  transfer, white over 2 s when Wheatley takes the body, and back to dark
  over 15 s later; measured in screenshots of both games, the brightness
  ramps over about 2 s and 4 s.
