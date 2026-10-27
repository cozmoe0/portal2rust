# Saved games

How the original saves and reloads during single-player, from the installed
`engine.dll` / `server.dll` (checked in IDA) and the console variable dump,
and how `crates/portal2/src/save` does it.

## When the game saves

- `sv_autosave 1`: entering a level through a transition autosaves.
- `logic_autosave` (81 in the single-player maps): `Save` autosaves;
  `SaveDangerous <seconds>` makes a dangerous autosave when the player has at
  least `MinimumHitPoints`, which becomes the autosave only if the player is
  still alive with `MinHitPointsToCommit` when the seconds are up;
  `SetMinHitpointsThreshold` sets `MinimumHitPoints`.
- `trigger_autosave` (57): when the player touches it, it removes itself and
  autosaves, or makes a dangerous autosave with its `DangerousTimer` (skipped
  below `MinimumHitPoints`).
- A pending dangerous autosave is committed early when another autosave
  request comes while it is still pending and the player is alive.
- Commands: `save <name>`, `load <name>`, `autosave`, `autosavedangerous`,
  `autosavedangerousissafe` (turns the dangerous autosave into the
  autosave), `reload`.
- `save_history_count 1`: one older copy of autosaves and quick saves is
  kept (`autosave01`, `quick01`).
- Saving is refused while the player is dead ("Can't savegame with a dead
  player").

## Reloading

- Death in single-player: the server issues `reload` after
  `sk_auto_reload_time` (3) seconds.
- `player_loadsaved` (9): `Reload` fades the screen (`rendercolor`,
  `renderamt`, `duration`, `holdtime`) and reloads after `loadtime` seconds.
- `reload` loads the most recent save written or loaded; with none it
  restarts the map.
- On load `logic_auto` entities still in the world fire again (`OnLoadGame`,
  then `OnMapSpawn`) a fifth of a second in; fire-once ones removed
  themselves when they first fired.
- The original saves the script VM; script objects of native classes lose
  their native data (the shipped `turret_vo_manager.nut` recreates its
  `regexp` after loads for that reason).

## Our format

- One gzip-compressed CBOR file per save in our own folder
  (`%LOCALAPPDATA%\portal2rust\save`, or `P2R_SAVE_DIR`), never the game's
  `SAVE` folder.
- Contents: map, game clock, every map entity (its save id: the map entity
  index, or a counter from 2^20 for entities made while playing; keyvalues,
  outputs, local transform, parent entity or model bone, visibility and the
  state of its components), resources (I/O event queue, global states,
  portal gun, portals, held object, fade, flags), the script VM and the
  player (movement state, view angles, health).
- The script VM is stored slot for slot (see `squirrel::snapshot`), so table
  orders and object identities come back exactly; entity handles are stored
  as save ids. Regexp objects keep their pattern.
- Loading reloads the map as a "load game", spawns the saved entities from
  their saved keyvalues (spawn code builds models, collision and other
  runtime parts) without running their scripts, then puts the saved state on
  top; components only the running game needs (render entities, loaded
  scene data, sound channels) are kept from the spawn code. Looping sounds
  that were playing start again, lines of dialogue being spoken carry on
  from where they were (with their captions), the player's portals are
  placed again and a held object stays held.
- `--save-test <maps>` checks that a save, loaded and saved again at once,
  is identical (all 63 maps pass); `--save-info <name>` summarises a save.

## Not done yet

One-shot sounds that were playing are not restarted; `logic_active_autosave`
(used by no single-player map) is not implemented; there is no save
screenshot, "Saving..." notice or menu yet (M7).
