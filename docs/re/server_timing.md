# Server timing

Facts about how the single-player server schedules work, gathered from the
installed `server.dll` (IDA) and general Source engine behaviour. Written in
our own words; no code is copied.

## Tick

- The single-player server runs 60 ticks per second; `curtime` is
  `tick * (1/60)` and restarts when a map loads.
- Think times are rounded to the nearest tick: something scheduled for time
  `t` runs on tick `floor(0.5 + t * 60)`.
- Within a tick: player movement and entity thinks, then physics, then the
  I/O event queue. An event is delivered once its fire time is `<= curtime`.
  (Ordering from general engine knowledge; still to be confirmed in IDA.)

## logic_auto (verified in server.dll)

- On activation it schedules its think at `curtime + 0.2` seconds.
- The think does nothing while its `globalstate` keyvalue names a global
  state that is not on.
- Otherwise it fires the output for how the map was entered (`OnNewGame`,
  `OnLoadGame`, `OnMapTransition` or `OnBackgroundMap`), then `OnMapSpawn`
  (multiplayer adds `OnMultiNewMap` / `OnMultiNewRound`), and removes itself
  when spawnflag 1 ("remove on fire") is set.
- Datadesc outputs, in order: OnNewGame, OnLoadGame, OnMapTransition,
  OnBackgroundMap, OnMultiNewMap, OnMultiNewRound (plus OnMapSpawn).

## env_global

- Spawnflag 1 creates the named state with `initialstate` (0 off, 1 on,
  2 dead) and `counter` only if the state does not exist yet; states survive
  map changes.
- Inputs: TurnOn, TurnOff, Toggle, Remove (dead), SetCounter, AddToCounter,
  GetCounter (fires OutCounter).
