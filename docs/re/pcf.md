# Particle definitions (`.pcf`)

Portal 2 describes its particle effects in `.pcf` files: binary DMX documents
(the serialisation format of Valve's generic "datamodel") that hold particle
system definitions. `source_formats::dmx` reads the container and
`source_formats::pcf` turns it into `ParticleSystemDefinition`s.

```bash
cargo run -p portal2_tools -- pcfdump                  # every system with its operator names
cargo run -p portal2_tools -- pcfdump portal_close   # one system in full
cargo run -p portal2_tools -- pcfdump --census       # the tables at the end of this page
```

Everything below was checked against the installed game. All 78 files parse
to their last byte, and the defaults come from the attribute tables in the
game's `portal2/bin/client.dll`.

## Where the files are

- `particles/particles_manifest.txt` is a loose file in `portal2/particles/`.
  It is KeyValues: one `particles_manifest` block of `"file" "<path>"`
  entries, read in order. The game warns about any other key.
- A path may start with `!`. The client's config-file loader strips it and
  forces its precache flag on for that file. The stock manifest marks
  `debug.pcf` and the second listing of `portalgun.pcf` this way.
- The stock manifest has 37 entries naming 36 files (`portalgun.pcf` is
  listed twice); a further 12 are commented out.
- All 78 `.pcf` files are in `portal2/pak01_dir.vpk`. The `update` and DLC
  VPKs add none, and only the manifest is loose.
- The client also reads a per-map manifest, `maps/<map>_particles.txt`, in
  the same format. No stock map has one: none of the 117 BSPs (loose or in
  VPKs) packs one or any `.pcf`, and no VPK holds one.

## DMX binary container

Integers are little-endian. A file is:

1. The header text `<!-- dmx encoding binary <E> format pcf <F> -->`, a
   newline and one NUL byte.
2. From encoding 2, a string table: a count, then that many NUL-terminated
   strings. The count is a `u16` in encodings 2 and 3 and an `i32` from 4.
3. The element count (`i32`), then one header per element: type name,
   element name and a 16-byte GUID.
4. One body per element in the same order: the attribute count (`i32`), then
   for each attribute its name, a type byte and the value.

A string is either inline (NUL-terminated) or a reference into the table: a
`u16` index up to encoding 4, an `i32` in encoding 5.

| Encoding | Element type and attribute names | Element names and string values |
| --- | --- | --- |
| 1 | inline | inline |
| 2, 3 | table | inline |
| 4 | table | table |
| 5 | table, 32-bit index | table, 32-bit index |

Strings inside string arrays are always inline.

| Header | Files | Listed in manifest |
| --- | ---: | ---: |
| `binary 2 format pcf 1` | 19 | 2 |
| `binary 3 format pcf 1` | 1 | 0 |
| `binary 3 format pcf 2` | 11 | 3 |
| `binary 5 format pcf 2` | 47 | 31 |

Encodings 2, 3 and 5 are verified on the install. Encoding 3 lays out
everything these files contain exactly as encoding 2 does. Encodings 1 and 4
do not occur; the reader handles them as the table says, which only the unit
tests exercise.

### Attribute types

| Id | Type | Stored as |
| ---: | --- | --- |
| 1 | element | `i32` element index; -1 is null. -2 means an element in another file and is followed by its GUID; PCFs never use it and the reader rejects it |
| 2 | int | `i32` |
| 3 | float | `f32` |
| 4 | bool | 1 byte |
| 5 | string | inline or table reference, see above |
| 6 | binary | `i32` length, then the bytes |
| 7 | time | `i32` ticks of 1/10000 s |
| 8 | color | 4 bytes, RGBA |
| 9 | vector2 | 2 `f32` |
| 10 | vector3 | 3 `f32` |
| 11 | vector4 | 4 `f32` |
| 12 | qangle | 3 `f32`: pitch, yaw, roll in degrees |
| 13 | quaternion | 4 `f32`: x, y, z, w |
| 14 | matrix | 16 `f32`, row by row |
| 15-28 | array of type id - 14 | `i32` count, then the items |

Only types 1, 2, 3, 4, 5, 8, 10, 11 and 15 occur in the install. Type 7 and
the matrix layout are therefore unverified; some older datamodel builds kept
a 16-byte object id under id 7 instead of a time.

## PCF element tree

- Element 0 is the root, of type `DmElement` (68 files) or `DmeElement` (10).
  Its element array `particleSystemDefinitions` lists the file's systems; two
  files also store an `editorType` string there.
- `DmeParticleSystemDefinition`: the element name is the system name. It
  holds the base properties below and seven element arrays: the six ordered
  operator stacks `renderers`, `operators`, `initializers`, `emitters`,
  `forces` and `constraints`, plus `children`.
- `DmeParticleOperator`: `functionName` names the engine operator; its other
  attributes are that operator's parameters. The element name is only an
  editor label; it differs from `functionName` on 3 operators, all in
  `fizzler.pcf`.
- `DmeParticleChild`: `child` points at a definition in the same file;
  `delay` is a float in seconds (default 0) and `end cap effect` a bool
  (default false, true on 2 children). Both defaults are from client.dll.

Shape of the stock data:

- The root array lists every definition in its file, and child links only
  point at listed definitions. One definition can be the target of many
  child links (up to 16); operator elements are never shared. No reference
  is null.
- Files from the older editor (encodings 2 and 3) store every attribute that
  editor knew about. Encoding 5 files mostly leave out attributes that equal
  their defaults (the two replacement-definition strings are written anyway),
  so a reader has to apply the defaults below.
- System names are unique within a file. 120 names occur in more than one
  file, mostly `_dx80` and `_unused` variants the manifest does not list.
  Among manifest files only 2 names repeat (one shared by `fire_01.pcf` and
  `fire_01_portal.pcf`, one by `impact_fx.pcf` and `scene_fx.pcf`). Which
  copy the game keeps is not established yet.
- `functionName` is matched ignoring case. Manifest files spell three
  initializers in lowercase (`remap initial scalar`, `lifetime from
  sequence`, `move particles between 2 control points`) while client.dll only
  contains their capitalised names. Every `functionName` in the install has a
  matching string in client.dll when case is ignored, including the older
  underscore names (`alpha_fade`, `basic_movement`, ...) that occur only in
  files outside the manifest.

## Definition base properties

Attribute names, types and defaults come from client.dll's attribute table
for `DmeParticleSystemDefinition`, listed here in that table's order.
"Stored" counts the definitions (of 1344) that write the attribute
explicitly.

| Attribute | Type | Default | Field | Stored |
| --- | --- | --- | --- | ---: |
| `max_particles` | int | 1000 | `max_particles` | 1154 |
| `initial_particles` | int | 0 | `initial_particles` | 402 |
| `material` | string | `vgui/white` | `material` | 1324 |
| `bounding_box_min` | vector3 | -10 -10 -10 | `bounding_box_min` | 769 |
| `bounding_box_max` | vector3 | 10 10 10 | `bounding_box_max` | 814 |
| `cull_radius` | float | 0 | `cull_radius` | 180 |
| `cull_cost` | float | 1 | `cull_cost` | 110 |
| `cull_control_point` | int | 0 | `cull_control_point` | 104 |
| `cull_replacement_definition` | string | empty | `cull_replacement` | 1117 |
| `fallback replacement definition` | string | empty | `fallback_replacement` | 1106 |
| `fallback max count` | int | -1 | `fallback_max_count` | 163 |
| `radius` | float | 5 | `radius` | 574 |
| `color` | color | 255 255 255 255 | `color` | 486 |
| `rotation` | float | 0 | `rotation` | 331 |
| `rotation_speed` | float | 0 | `rotation_speed` | 331 |
| `normal` | vector3 | 0 0 1 | `normal` | 1 |
| `sequence_number` | int | 0 | `sequence_number` | 339 |
| `sequence_number 1` | int | 0 | `sequence_number_1` | 336 |
| `group id` | int | 0 | `group_id` | 330 |
| `maximum time step` | float | 0.1 | `max_time_step` | 344 |
| `maximum sim tick rate` | float | 0 | `max_sim_tick_rate` | 335 |
| `minimum sim tick rate` | float | 0 | `min_sim_tick_rate` | 348 |
| `minimum rendered frames` | int | 0 | `min_rendered_frames` | 330 |
| `control point to disable rendering if it is the camera` | int | -1 | `camera_disable_control_point` | 337 |
| `control point to only enable rendering if it is the camera` | int | -1 | `camera_only_control_point` | 122 |
| `maximum draw distance` | float | 100000 | `max_draw_distance` | 546 |
| `time to sleep when not drawn` | float | 8 | `sleep_when_not_drawn` | 409 |
| `Sort particles` | bool | true | `sort_particles` | 553 |
| `batch particle systems` | bool | false | `batch_particle_systems` | 140 |
| `view model effect` | bool | false | `view_model_effect` | 115 |
| `screen space effect` | bool | false | `screen_space_effect` | 101 |
| `draw through leafsystem` | bool | true | `draw_through_leafsystem` | 101 |
| `maximum portal recursion depth` | int | 8 | `max_portal_recursion_depth` | 50 |
| `aggregation radius` | float | 0 | `aggregation_radius` | 229 |
| `minimum free particles to aggregate` | int | 0 | `min_free_particles_to_aggregate` | 96 |
| `minimum simulation time step` | float | 0 | `min_simulation_time_step` | 111 |
| `minimum CPU level` | int | 0 | `min_cpu_level` | 106 |
| `minimum GPU level` | int | 0 | `min_gpu_level` | 107 |
| `freeze simulation after time` | float | 1000000000 | `freeze_simulation_after_time` | 1 |

Material paths are relative to `materials/`. 1300 of the 1324 stored ones
use backslashes and end in `.vmt`; most of the other 24 use forward slashes
and no extension, like the default.

Attributes that occur in files but are not in the table:

- `preventNameBasedLookup` (bool) is on every definition and true on one,
  outside the manifest. Kept as `prevent_name_based_lookup`, default false.
- `bounding_box_control_point` (int, 227 definitions) and `fallback_dx80`
  (string, 171) are kept only in `ParticleSystemDefinition::attributes`.

`attributes` keeps every stored attribute except the seven arrays, so a
consumer can tell explicit values from defaults.

## Operator parameters

Besides its own parameters, any operator may store a common set; the older
files write them on every operator: `operator start fadein`, `operator end
fadein`, `operator start fadeout`, `operator end fadeout`, `operator fade
oscillate`, `operator strength random scale min` / `max`, `operator strength
scale seed`, `operator time offset min` / `max` / `seed`, `operator time scale
min` / `max` / `seed` and `operator time strength random scale max`.

Absent parameters take the operator's own defaults, from per-operator
tables in client.dll; `re/tools/particle_defaults.py` lists them and
[particles.md](particles.md) describes how they are found and how the
operators behave.

## Particle field indices

Operators choose particle fields by integer index in parameters such as
`output field`, `input field`, `ramp field`, `oscillation field` and
`Inherited Field`. client.dll names the indices in this order:

| Index | Field | Index | Field |
| ---: | --- | ---: | --- |
| 0 | Position | 12 | Yaw |
| 1 | Life Duration | 13 | Sequence Number 1 |
| 2 | Position Previous | 14 | Hitbox Index |
| 3 | Radius | 15 | Hitbox Offset Position |
| 4 | Roll | 16 | Alpha Alternate |
| 5 | Roll Speed | 17 | Scratch Vector |
| 6 | Color | 18 | Scratch Float |
| 7 | Alpha | 19 | (unnamed) |
| 8 | Creation Time | 20 | Pitch |
| 9 | Sequence Number | 21 | Normal |
| 10 | Trail Length | 22 | Glow RGB |
| 11 | Particle ID | 23 | Glow Alpha |

## functionName census

Uses per stack over every definition, from `pcfdump --census`. "All files"
covers all 78 files, so `_dx80` and `_unused` copies count too; "manifest
files" covers only the 36 files the manifest lists. Spellings that differ
only in case are one row, most common spelling first.

78 files (36 in the manifest), 1344 systems (833 from manifest files)

### `renderers` (5 functions, 1292 uses)

| functionName | all files | manifest files |
| --- | ---: | ---: |
| render_animated_sprites | 1024 | 598 |
| render_sprite_trail | 173 | 125 |
| render_rope | 81 | 67 |
| Render models | 11 | 10 |
| render_blobs | 3 | 3 |

### `operators` (61 functions, 6259 uses)

| functionName | all files | manifest files |
| --- | ---: | ---: |
| Movement Basic | 1202 | 744 |
| Lifespan Decay | 982 | 601 |
| Radius Scale | 868 | 520 |
| Alpha Fade Out Random | 675 | 389 |
| Alpha Fade In Random | 351 | 137 |
| Alpha Fade and Decay | 301 | 180 |
| Rotation Spin Roll | 241 | 174 |
| Movement Lock to Control Point | 232 | 183 |
| Color Fade | 211 | 88 |
| Oscillate Scalar | 209 | 159 |
| Oscillate Vector | 133 | 79 |
| Alpha Fade In Simple | 90 | 89 |
| Alpha Fade Out Simple | 83 | 79 |
| Ramp Scalar Linear Simple | 77 | 74 |
| Set Control Point Positions | 57 | 45 |
| Remap Distance to Control Point to Scalar | 38 | 27 |
| Movement Dampen Relative to Control Point | 38 | 25 |
| Remap Control Point to Vector | 32 | 30 |
| Ramp Scalar Spline Random | 29 | 29 |
| Set Control Point Rotation | 26 | 25 |
| alpha_fade | 24 | 0 |
| basic_movement | 24 | 0 |
| Clamp Scalar | 23 | 23 |
| Lifespan Minimum Velocity Decay | 20 | 20 |
| Ramp Scalar Linear Random | 17 | 17 |
| color_fade | 17 | 0 |
| radius_scale | 17 | 0 |
| Cull when crossing plane | 16 | 11 |
| Movement Maintain Position Along Path | 16 | 9 |
| Movement Lock to Bone | 15 | 15 |
| Remap Direction to CP to Vector | 14 | 14 |
| Lifespan Minimum Alpha Decay | 13 | 11 |
| Inherit Attribute From Parent Particle | 12 | 12 |
| Set child control points from particle positions | 12 | 6 |
| Cull Random | 12 | 1 |
| Lifespan Minimum Radius Decay | 11 | 11 |
| Remap Difference of Sequential Particle Vector to Scalar | 11 | 11 |
| Rotation Spin Yaw | 11 | 10 |
| Movement Max Velocity | 10 | 7 |
| Remap Scalar | 8 | 8 |
| Rotation Basic | 8 | 8 |
| Set Control Point to Impact Point | 8 | 6 |
| Movement Lock to Saved Position Along Path | 7 | 7 |
| Remap Control Point to Scalar | 7 | 7 |
| Remap CP Speed to CP | 7 | 7 |
| Restart Effect after Duration | 5 | 5 |
| Remap Velocity to Vector | 5 | 4 |
| remap dot product to scalar | 5 | 0 |
| rotation_spin | 5 | 0 |
| Remap Percentage Between Two Control Points to Scalar | 4 | 4 |
| Cull when crossing sphere | 3 | 3 |
| Remap Speed to Scalar | 3 | 3 |
| Lerp Initial Scalar | 2 | 2 |
| Normal Lock to Control Point | 2 | 2 |
| Oscillate Scalar Simple | 2 | 2 |
| Remap Distance Between Two Control Points to Scalar | 2 | 0 |
| Set Control Point To Player | 2 | 0 |
| Ramp Scalar Spline Simple | 1 | 1 |
| Set control points from particle positions | 1 | 1 |
| Set per child control point from particle positions | 1 | 1 |
| Noise Vector | 1 | 0 |

### `initializers` (57 functions, 9018 uses)

| functionName | all files | manifest files |
| --- | ---: | ---: |
| Position Within Sphere Random | 1111 | 669 |
| Lifetime Random | 1066 | 691 |
| Radius Random | 1034 | 664 |
| Color Random | 912 | 563 |
| Alpha Random | 905 | 540 |
| Rotation Random | 855 | 515 |
| Sequence Random | 606 | 414 |
| Position Modify Offset Random | 599 | 347 |
| Velocity Noise | 280 | 192 |
| Rotation Yaw Flip Random | 243 | 196 |
| remap initial scalar / Remap Initial Scalar | 180 | 112 |
| Position Modify Warp Random | 143 | 75 |
| Trail Length Random | 140 | 101 |
| lifetime from sequence / Lifetime From Sequence | 95 | 46 |
| Remap Noise to Scalar | 93 | 74 |
| Sequence Two Random | 66 | 59 |
| Remap Initial Distance to Control Point to Scalar | 45 | 38 |
| Remap Control Point to Vector | 45 | 36 |
| Velocity Random | 43 | 43 |
| Rotation Yaw Random | 43 | 35 |
| Remap Particle Count to Scalar | 39 | 39 |
| Position Along Ring | 38 | 38 |
| Position on Model Random | 34 | 33 |
| Position Along Path Sequential | 28 | 20 |
| Cull relative to Ray Trace Environment | 26 | 20 |
| Remap Control Point to Scalar | 24 | 21 |
| Position Within Box Random | 23 | 18 |
| lifetime_random | 23 | 0 |
| position_within_sphere | 23 | 0 |
| Lifetime Pre-Age Noise | 22 | 11 |
| rotation_random | 22 | 0 |
| radius_random | 19 | 0 |
| Velocity Inherit from Control Point | 18 | 14 |
| Velocity Repulse from World | 18 | 6 |
| Initial Velocity Noise | 17 | 0 |
| Normal Align to CP | 15 | 15 |
| alpha_random | 15 | 0 |
| color_random | 15 | 0 |
| Position Modify Place On Ground | 13 | 13 |
| Lifetime from Time to Impact | 12 | 10 |
| Position From Parent Particles | 9 | 7 |
| Position from Parent Cache | 8 | 8 |
| Velocity Set from Control Point | 8 | 5 |
| remap scalar to vector / Remap Scalar to Vector | 7 | 3 |
| Position Along Epitrochoid | 6 | 6 |
| sequence_random | 6 | 0 |
| Initial Scalar Noise | 5 | 0 |
| Color Lit Per Particle | 4 | 4 |
| Position In CP Hierarchy | 4 | 0 |
| move particles between 2 control points / Move Particles Between 2 Control Points | 3 | 2 |
| Normal Modify Offset Random | 2 | 2 |
| Rotation Speed Random | 2 | 2 |
| position_offset_random | 2 | 0 |
| Position Along Path Random | 1 | 1 |
| position_within_box | 1 | 0 |
| Randomly Flip Yaw | 1 | 0 |
| trail_length_random | 1 | 0 |

### `emitters` (4 functions, 1345 uses)

| functionName | all files | manifest files |
| --- | ---: | ---: |
| emit_continuously | 873 | 448 |
| emit_instantaneously | 459 | 381 |
| emit noise | 12 | 12 |
| emit to maintain count | 1 | 1 |

### `forces` (5 functions, 329 uses)

| functionName | all files | manifest files |
| --- | ---: | ---: |
| random force | 134 | 74 |
| twist around axis | 106 | 29 |
| Pull towards control point | 86 | 31 |
| turbulent force | 2 | 1 |
| Force based on distance from plane | 1 | 1 |

### `constraints` (4 functions, 151 uses)

| functionName | all files | manifest files |
| --- | ---: | ---: |
| Collision via traces | 96 | 72 |
| Constrain distance to path between two control points | 21 | 9 |
| Constrain distance to control point | 20 | 12 |
| Prevent passing through a plane | 14 | 4 |
