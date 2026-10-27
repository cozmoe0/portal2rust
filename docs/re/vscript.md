# VScript (map scripting)

How the original game hosts Squirrel scripts, learned from the installed
`server.dll` and `vscript.dll` (IDA, string tables, embedded helper scripts).
Written in our own words; our implementation is in `crates/squirrel` (the
language) and `crates/portal2/src/script` (the game bindings).

## Language

- `vscript.dll` embeds "Squirrel 2.2.3 stable".
- Notable 2.2 traits: closures copy enclosing locals when created; `parent`
  (delegate of `this`) and `expr.parent`; `delegate a : b`; `vargc`/`vargv`;
  default parameter values; `const`/`enum`; no `base` keyword; plain calls
  pass the caller's `this`; unknown names resolve through `this` (with its
  delegates and default methods) and then the root table.

## VM lifetime

- A new VM is created for every map at level init; `scripts/vscripts/
  mapspawn.nut` runs first (in the root table).
- Helper functions provided by the hosting layer (script side in the
  original): `printl(text)` prints with a newline, `Msg(text)` prints,
  `Assert(b, msg)` throws "Assertion failed[: msg]", `UniqueString(s)`,
  `EntFire(target, action, value = null, delay = 0.0, activator = null)`,
  `IncludeScript(name, scope = null)` (scope defaults to the caller's
  `this`), plus documentation helpers.
- `EntFire`: a null/false value becomes ""; when the calling scope has a
  `self`, it is the caller and the default activator; target, action and
  value are converted with `tostring()`; it queues the input like any
  output (`DoEntFire`), never runs it immediately.

## Entity scopes

- A scope is a table stored in the root table under the entity's script id,
  with `__vname` (that id) and `__vrefs` (reference count) slots and the root
  table as its delegate. Releasing the last reference deletes it from the
  root and clears the delegate.
- Entities with a `vscripts` keyvalue run their scripts when spawned, before
  the entity's own spawn code: the value is split on spaces and each file
  runs in the entity's scope (worldspawn's run in the root table).
- Before the files run, call chains for `OnPostSpawn` and `Precache` are set
  up; after each file, a function named exactly `OnPostSpawn` / `Precache`
  in the scope is appended to its chain (appended again by every later file
  too, as the original's duplicate check never matches). `Dispatch<Name>`
  calls the chain from the last entry to the first with the scope as `this`
  and stops if one returns a non-null false value.
- Spawn order per entity: run scripts, dispatch `Precache`, entity spawn,
  then `CallScriptFunction DispatchOnPostSpawn` is queued with no delay
  (activator and caller = the entity), so it runs when the queue is next
  serviced.
- `thinkfunction`: the named function is called every
  `sv_script_think_interval` (0.1 s) starting one interval after the scripts
  ran; a numeric return value replaces the interval for the next think; a
  failed call stops thinking.

## Inputs and outputs

- When any input reaches an entity that has a scope, `activator` and
  `caller` are set in the scope for the duration of the input; if the scope
  defines a function `Input<InputName>` it is called first and the input is
  only handled when it returns true (or nothing).
- `RunScriptCode` / `RunScriptFile` / `CallScriptFunction` run in the
  entity's scope.
- `ConnectOutput(output, function)` adds an ordinary output connection to
  `!self` with input `CallScriptFunction`, the function name as parameter,
  no delay and unlimited fires (skipped if an identical one exists).
  `DisconnectOutput` removes it.

## Native functions (server.dll registration names)

Globals include: `Time`, `FrameTime`, `DoEntFire`, `EntFireByHandle`,
`DoUniqueString`, `CreateSceneEntity`, `DoIncludeScript`, `TraceLine`,
`SendToConsole`, `GetMapName`, `ShowMessage`, `GetDeveloperLevel`,
`SetDucking`, `RecordAchievementEvent`, `CreateProp`, `DebugDrawLine`,
`DebugDrawBox`, `LoopSinglePlayerMaps`, `GetMapIndexInPlayOrder`,
`GetNumMapsPlayed`, `SetMapAsPlayed`, `RequestMapRating`, `GetPlayer`,
`AddGladosSpokenFlags`, ... Classes: `CBaseEntity`, `CBaseAnimating`,
`CBaseFlex`, `CBasePlayer`, `CPortal_Player`, `CSceneEntity`,
`CTriggerCamera`, `CPropLinkedPortalDoor`, `CEntities` (the `Entities`
instance), `CScriptKeyValues`. The full member list with signatures comes
from the game's `script_help` output (see `re/tools`).
