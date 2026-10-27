# Choreographed scenes: `.vcd` text and `scenes.image`

Portal 2's dialogue and scripted animation (GLaDOS, Wheatley, Cave Johnson,
the announcer, the co-op robots) runs through choreographed scenes. A scene
names one or more actors; each actor has channels; each channel holds timed
events such as "play this line", "blend this gesture", "look at that target"
or "fire trigger 2". Scripts start them with `CreateSceneEntity`.

The facts below come from reading the installed data files and from watching
how `scenefilecache.dll` and `server.dll` consume them. They are written in
our own words; nothing here is transcribed from the binaries. The readers are
`source_formats::vcd` (text) and `source_formats::scenes_image` (compiled),
and `cargo run -p source_formats --example vcddump -- <scene>` prints one.

## Where scenes come from

- **Text** `.vcd` files under `scenes/` in the VPKs: 4662 in
  `portal2/pak01_dir.vpk`, 116 in `portal2_dlc1`, 82 in `portal2_dlc2`, none in
  `update`.
- **Compiled** `scenes/scenes.image`, a loose file in `portal2/`,
  `portal2_dlc1/` and `portal2_dlc2/`. The game opens only the first one on
  its `GAME` search path, which is the highest DLC directory, so every image
  repeats the scenes of the ones below it. The server loads scenes only
  through the image (a missing entry is a warning, with no fallback to the
  text file); the text files are the sources it was compiled from. Scripts can
  also pass a scene as text (`LoadSceneFromString`).
- Scenes are looked up by path. The key is built by lowercasing, turning `/`
  into `\`, and adding `.vcd` when the file name has no extension, giving
  `scenes\npc\glados\foo.vcd`. The image stores the CRC-32 (zlib variant) of
  that key and binary-searches its entry table, which is sorted by CRC.
- Some entries have no text source in any VPK: one in the base image (no
  sounds, 3.35 s long) and 31 more in the DLC images (DLC1 co-op ending lines).
  Their paths cannot be recovered from the CRC; they are only reachable by
  name through the image.

## Text format

A token stream: quoted strings, bare words, `{`, `}` and `//` comments to end
of line. Files start with the comment `// Choreo version 1`. Keyword matching
ignores case. A few trailing values are optional and are only taken when they
sit on the keyword's own line, so line breaks matter in exactly those places
(noted as "same line" below). Unknown words inside an event are skipped one at
a time by the game; our reader keeps them in `unknown` lists.

### Top level

| Statement | Meaning |
| --- | --- |
| `actor "<name>" { ... }` | An actor (see below). |
| `event <type> "<name>" { ... }` | Scene-level event. In Portal 2 these are all stop points and sections. |
| `mapname "<map>"` | Map the scene was authored for. Unused by Portal 2 data. |
| `fps <n>` | Editor frame rate; the game clamps it to 10..240. |
| `snap on\|off` | Editor snapping. |
| `ignorePhonemes on\|off` | Skip phoneme-driven lip sync. |
| `scene_ramp <curve>` | Intensity curve over the whole scene. |
| `scalesettings { "<tool>" "<percent>" ... }` | Editor zoom per tool window; values of 0 or less mean 100. |

Anything else at the top level makes the game stop reading the file.

Actor blocks contain `channel "<name>" { ... }`, `faceposermodel "<path>"`
(editor preview model) and `active "<0|1>"`. Channel blocks contain events and
`active "<0|1>"`. Both default to active.

### Events

`event <type> "<name>" { ... }` where the type is one of the names in the
table further down (an unknown name becomes "unspecified" in the game; our
reader keeps it as `EventKind::Other`). The body is a list of these, in any
order:

| Keyword | Arguments | Meaning | Compiled? |
| --- | --- | --- | --- |
| `time` | start, end (end: same line) | Seconds from scene start. End -1 marks an event without duration. If the end is missing the game sets it to 1.0. | yes |
| `param`, `param2`, `param3` | string | Type-specific: sound name and sound-level override for speak; sequence for gesture and sequence; target for look-at, face and move-to (move-to's `param2` is the movement activity); expression file and name for expression; trigger number for firetrigger. | yes |
| `event_ramp` | curve | Intensity over the event, times relative to its start. | yes, without interpolation types |
| `ramp` | a, b, c (b and c: same line) | Old shorthand that the game expands into three ramp samples when the event has no `event_ramp` and a value is not 1. | no |
| `fixedlength`, `resumecondition`, `lockbodyfacing`, `forceshortmovement`, `playoverscript` | - | Flags. | yes |
| `synctofollowinggesture` | - | Flag. | no |
| `active` | 0 or 1 | Event enabled (default 1). | yes |
| `distancetotarget` | float | Move-to stopping distance. | yes |
| `pitch`, `yaw` | integer | Look-at offsets in degrees. | no |
| `loopcount` | integer | Loop event repeat count (default -1). | yes, as a signed byte |
| `sequenceduration` | float | Length of the gesture's sequence, used to place its tags. | gesture only |
| `tags { "<name>" <fraction> ... }` | | Named points in the event as fractions of its duration. | yes, byte precision |
| `absolutetags <playback_time\|shifted_time> { "<name>" <fraction> ... }` | | Gesture tags before and after time shifting. | yes, 1/4096 steps |
| `flextimingtags { "<name>" <fraction> <locked> ... }` | | Tags for flex animation timing. | yes, lock flag dropped |
| `relativetag "<tag>" "<sound>"` | | Start the event at a tag of another (speak) event. | yes |
| `flexanimations ...` | see below | Flex controller curves. | yes |
| `cctype` | `cc_master`, `cc_slave`, `cc_disabled` | Caption role; unknown names mean master. | speak only |
| `cctoken` | string | Caption token to use instead of the sound's own. | speak only |
| `cc_usingcombinedfile`, `cc_combinedusesgender`, `cc_noattenuate` | - | Caption flags. | speak only |

### Curves

`event_ramp` and `scene_ramp` are followed by optional
`leftedge <interp> <value>` and `rightedge <interp> <value>` (value and
interpolation before the first and after the last sample), then a block of
samples, one per line: `<time> <value> [<interp>]`, with the interpolation
name on the same line. An interpolation name has the form
`curve_<in>_to_curve_<out>`; each half is one of:

| # | Name | # | Name |
| --- | --- | --- | --- |
| 0 | `default` | 8 | `kochanek_early` |
| 1 | `catmullrom_normalize_x` | 9 | `kochanek_late` |
| 2 | `easein` | 10 | `simple_cubic` |
| 3 | `easeout` | 11 | `catmullrom` |
| 4 | `easeinout` | 12 | `catmullrom_normalize` |
| 5 | `bspline` | 13 | `catmullrom_tangent` |
| 6 | `linear_interp` | 14 | `exponential_decay` |
| 7 | `kochanek` | 15 | `hold` |

Names that do not start with `curve_` mean default. Compiled data packs a pair
as `in << 8 | out` in a 16-bit value.

### Flex animations

```
flexanimations [samples_use_time] [defaultcurvetype=<interp>]
{
  "<controller>" [disabled] [combo] [range <min> <max>] [leftedge ..] [rightedge ..]
  {
    <time> <value> [<interp>]
    ...
  }
  [{ second sample list for combo tracks }]
  ...
}
```

`defaultcurvetype` may have spaces around the `=`. Sample times are fractions
of the event's duration unless `samples_use_time` is given; the game converts
them to seconds from the event start while reading. A combo track drives a
left/right controller pair: the first list is the amount, the second the
balance. Ranges default to 0..1. Disabled tracks with no samples are dropped.

## Event types

The compiled format numbers types in this order. Counts are events in the
4860 text files (base game plus both DLCs).

| # | Type | Count | # | Type | Count |
| --- | --- | --- | --- | --- | --- |
| 0 | `unspecified` | 0 | 10 | `flexanimation` | 0 |
| 1 | `section` | 2 | 11 | `subscene` | 0 |
| 2 | `expression` | 65 | 12 | `loop` | 0 |
| 3 | `lookat` | 57 | 13 | `interrupt` | 0 |
| 4 | `moveto` | 30 | 14 | `stoppoint` | 767 |
| 5 | `speak` | 5056 | 15 | `permitresponses` | 0 |
| 6 | `gesture` | 6208 | 16 | `generic` | 0 |
| 7 | `sequence` | 708 | 17 | `camera` | 0 |
| 8 | `face` | 52 | 18 | `script` | 0 |
| 9 | `firetrigger` | 59 | | | |

## `scenes.image`

Little endian throughout.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 4 | `VSIF` |
| 4 | 4 | version, must be 3 |
| 8 | 4 | number of scenes |
| 12 | 4 | number of strings |
| 16 | 4 | offset of the entry table |
| 20 | 4 per string | offset of each string |

The strings are NUL-terminated and shared by every scene. The pool keeps one
spelling per string ignoring case (the base image has `audio` but no `Audio`),
so names read from compiled scenes can differ in case from their sources.

Each entry is 16 bytes: CRC of the key, offset and length of the scene data,
and offset of a summary. The summary holds the scene's duration in
milliseconds (the latest start or end of any event, rounded), the end of the
speak event listed last (not necessarily the one that ends last, in ms), the
number of sounds the scene plays and the string index of each, so a scene's
length and sounds are known without decoding it.

Scene data is either a raw binary scene (starts `bvcd`) or Valve's LZMA
wrapper: `LZMA`, decompressed size, compressed size, the 5 LZMA property
bytes, then the raw stream. The 417 raw entries in the base image are all 93
bytes or smaller.

## Binary scene (`bvcd`, version 4)

Header: `bvcd`, a version byte (4), and the CRC-32 of the source `.vcd` file's
bytes (the game ignores it; we use it to pair texts with compiled scenes).
Then:

1. a byte count of scene-level events, each an event record;
2. a byte count of actors; each actor is a string index, a byte count of
   channels, the channels, and an "active" byte (1 = active). A channel is a
   string index, a byte count of events, the event records and an "active"
   byte;
3. the scene ramp as a curve record;
4. a byte, non-zero when phonemes are ignored.

Strings are 16-bit indices into the image's pool. A curve record is a byte
count, then per sample a `f32` time and a byte value (value / 255); it has no
interpolation types or edges.

An event record:

| Field | Encoding |
| --- | --- |
| type | byte, numbered as in the table above |
| name | string |
| start, end | `f32` each |
| param, param2, param3 | strings; the game trims trailing spaces from the last two |
| ramp | curve record |
| flags | byte: 1 resumecondition, 2 lockbodyfacing, 4 fixedlength, 8 active, 16 forceshortmovement, 32 playoverscript |
| distance to target | `f32` |
| tags | byte count, then string + byte (value / 255) each |
| flex timing tags | the same layout |
| playback-time tags, shifted-time tags | for each kind: byte count, then string + `u16` (value / 4096) each |
| sequence duration | `f32`, gesture events only; -1 when unknown |
| relative tag | byte; when it is 1, two strings (tag, sound) follow |
| flex tracks | byte count; each: string, byte flags (1 active, 2 combo), `f32` min, `f32` max, `i16` sample count, samples; combo tracks add a `u16` count and the balance samples. A flex sample is `f32` time (seconds from event start), byte value (/ 255) and `u16` interpolation pair |
| loop count | signed byte, loop events only |
| caption | speak events only: byte type (0 master, 1 slave, 2 disabled), string token, byte flags (1 using combined file, 2 combined uses gender, 4 no attenuation) |

Not stored: map name, fps, snapping, scale settings, the editor model, curve
edges, ramp interpolation types, look-at pitch and yaw, `synctofollowinggesture`,
timing-tag locks and the old `ramp` shorthand.

## Census of the installed data

Images (as installed):

| Directory | Size | Scenes | Strings | LZMA-wrapped |
| --- | --- | --- | --- | --- |
| `portal2` | 815,576 | 4663 | 4689 | 4246 |
| `portal2_dlc1` | 836,612 | 4809 | 4834 | 4395 |
| `portal2_dlc2` | 849,830 | 4891 | 4916 | 4476 |

Every image entry decodes and consumes its data exactly, and every summary
matches the decoded scene to the millisecond. For every `.vcd` in a VPK the
image of the same directory has an entry, and its compiled source CRC matches
the text (no stale compiles). Comparing the 4662 + 4778 + 4860 text/compiled
pairs, actors, channels, event names, types, start and end times (bit-exact),
parameters, flags, ramps, tags and caption settings all agree once case and
quantisation are allowed for.

Text files (all 4860):

- 4849 scenes have one actor, 11 have two. 47 distinct actor names; the most
  common are `@sphere` (1814 scenes), `@glados` (1412), `player` (834, the
  co-op robots), `!Target1` (305) and the cores.
- Channel names are editor conventions, compared ignoring case: `audio`,
  `gesture`, `animation`, `lookat`, `faceto`, `moveto` and `trigger` appear in
  almost every scene; 26 distinct names in all.
- By folder: `npc/sphere03` 1473, `npc/glados` 1191, `player/ballbot` 688,
  `npc/cavejohnson` 235, `npc/turret` 224, `npc/sphere01` 210,
  `npc/announcer` 170, `player/eggbot` 146, the three cores 269, `templates` 63.
- Every speak event is `fixedlength` with `cctype "cc_master"` and an empty
  `cctoken`; 63 override the sound level in `param2` (`75dB` or `VOL_NORM`).
- `event_ramp`: 4718 gestures, 608 sequences, every expression and face
  event, 54 look-ats and one speak. 3894 ramp samples name an
  interpolation, mostly `curve_easein_to_curve_easeout`; 10 distinct pairs.
- `sequenceduration` on 5542 of 6208 gestures; 61 gestures carry both kinds
  of absolute tags; one gesture has relative `tags`.
- `fixedlength` also on 465 sequences; `lockbodyfacing` on 23 face events;
  `distancetotarget` 29, `forceshortmovement` 27 and `resumecondition` 2 on
  move-to events; 106 actors name a `faceposermodel`; one channel is
  `active "0"`. `ignorePhonemes` appears in 1143 files, always `off`.
- One event name ends in a space (`"Look at "`); the compiled copy keeps it.
- Never used: `mapname`, `scene_ramp`, curve edges, `flexanimations`,
  `flextimingtags`, `relativetag`, `loopcount`, `pitch`, `yaw`,
  `playoverscript`, `synctofollowinggesture`, the caption flags and the old
  `ramp` shorthand, and the event types with a count of 0 above.

## Playback (`logic_choreographed_scene`)

What the engine's scene entity exposes, from `server.dll`'s strings, and how
`crates/portal2/src/entities/scenes.rs` plays scenes:

- Keys: `SceneFile`, `ResumeSceneFile`, `target1`..`target8`, `busyactor`,
  `onplayerdeath`. `CreateSceneEntity(file)` in scripts makes the same
  entity.
- Inputs: `Start`, `Pause`, `Resume`, `Cancel`, `CancelAtNextInterrupt`,
  `PitchShift`, `InterjectResponse`, `StopWaitingForActor`, `Trigger`,
  `SetTarget1`..`SetTarget4`, `ScriptPlayerDeath`, `Reload`.
- Outputs: `OnStart`, `OnCompletion`, `OnCanceled`, `OnTrigger1`..`OnTrigger16`.
- Actor names: `!target1`..`!target8` (the keys above), `!self`, `!player`,
  `!activator`, `!caller`, `!speechtarget`, `!friend`, `!enemy`; anything
  else is an entity name. In the campaign maps `@glados` is a
  `generic_actor` and `@sphere` an `npc_personality_core`.
- Our playback: scene time follows the game clock times the `PitchShift`
  factor. Events fire when the time passes their start: `speak` plays its
  sound on the actor (pitch from `PitchShift`, `param2` as a sound level
  override), `firetrigger` fires `OnTrigger<param>`, `sequence` starts the
  named sequence on the actor. The scene completes at its end time (the
  latest event start or end, the same value the image summary stores).
  `Cancel` stops the scene's speech and fires `OnCanceled`. Scripts can ask
  `EstimateLength`, `IsPlayingBack` and `IsPaused`.
- Gestures: every gesture event spanning the scene time layers its
  sequence on the actor, played over the event's length. The weight is the
  event's ramp (linear between samples) or, without one, the sequence's
  fade-in and fade-out times at the edges. The layer is accumulated like any
  sequence (per-bone weights, delta and `post` handling, its own automatic
  layers; see `studio_animation.md`).
- Personality cores (`npc_personality_core`) use
  `models/npcs/personality_sphere/personality_sphere_skins.mdl` (the server's
  model string; `ModelSkin` is the skin). Its gesture sequences (`g_*`) are
  delta sequences with the `post` flag. Through its animation include it has
  `aim_yaw` and `aim_pitch` (-1..1) and three 3x3 aim matrices
  (`sphere_aimmatrix_neutral`, `_eyeball`, `sphere_plug_aimmatrix_neutral`),
  but neither server nor client code names them or sets those parameters:
  Wheatley's looks are the hand-made gestures in his scenes (`g_angle_*`,
  `g_eye_look_*`, ...).
- Look-at and face: the server drives `head_yaw`, `head_pitch`,
  `head_roll`, `body_yaw`, `body_pitch` (and `aim_*`) pose parameters. No
  compiled scene with a look-at or face event is named by a single-player
  map (`SceneFile`) or script: they belong to co-op and old test scenes
  (`!target1` with `mbot`/`obot` expressions, `glados01`, `jack`). They are
  not played.
- Flex animation and lip sync: no compiled scene has a `flexanimation`
  event, and none of the 2709 voice waves under `sound/vo` has a `VDAT`
  (phoneme) chunk; no single-player model has a mouth. The only flexed
  single-player models are the toxin tubes and tanks (`props_destruction`),
  whose flexes nothing drives; Chell's model has two eyeballs (drawn by the
  eye shader) and no vertex flexes.
- Event census over `scenes.image`: gestures 5764 on `@sphere`, 319 on
  `@core02`, 29 on `glados01`, 25 on `!target1`; sequences 624 on `@sphere`;
  look-at 53 and face 52 on `!target1`, look-at 3 on `glados01`.
