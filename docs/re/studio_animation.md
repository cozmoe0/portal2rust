# Sequence blending

How `source_formats::studio` turns sequences into bone poses. Layouts come
from the MDL format; the blending rules were checked against the bone setup
code in the installed `server.dll`.

## Pose parameters

- A model's parameters are its own followed by those of its included
  animation models that it does not declare itself (merged by name).
  Sequences and layers number parameters within their own model; they are
  mapped to the merged list by name.
- An entity keeps a setting from 0 to 1 per parameter. Setting a value:
  looping parameters (loop range L) first wrap with
  `v -= L * floor((v + L - (start + end + L) / 2) / L)`; the setting is
  `(v - start) / (end - start)`, clamped.
- When a model is set every parameter is set to the value 0, so an
  `aim_yaw` running from -60 to 60 starts at 0.5.

## A sequence's own pose

- Grid position along each axis: with pose keys (`posekeyindex`, one value
  per grid line, axis 0 then axis 1), the value `start + (end - start) *
  setting` is located by walking the spans from the first while the
  fraction is above 1 and another span is left; the fraction is clamped.
  Without keys the fraction is `(setting - s0) / (s1 - s0)` where `s0`, `s1`
  are the sequence's `paramstart` / `paramend` as settings (a reversed range
  counts down), clamped; with more than two lines the index is
  `floor((n - 1) * f)`, at most `n - 2`, and the fraction is the remainder.
- Cycle: `CYCLEPOSE` sequences use the setting of their cycle pose
  parameter; looping cycles wrap into 0..1, others clamp. (`REALTIME`
  sequences follow the clock; not handled.)
- A fraction within 0.001 of a grid line counts as on it. On a line in both
  axes one animation is sampled; on a line in one axis two animations are
  blended along the other. Inside a cell (with `anim_3wayblend 1`, the
  default) the cell is split along a diagonal that alternates with the
  parity of `i0 + i1`:

  | cell | half | corners (offsets) | weights |
  |---|---|---|---|
  | odd | `s0 + s1 > 1` | (1,0) (1,1) (0,1) | `1 - s1`, `s0 + s1 - 1`, rest |
  | odd | otherwise | (0,1) (0,0) (1,0) | `s1`, `1 - s0 - s1`, rest |
  | even | `s0 <= s1` | (1,1) (0,1) (0,0) | `s0`, `s1 - s0`, rest |
  | even | otherwise | (0,0) (1,0) (1,1) | `1 - s0`, `s0 - s1`, rest |

  The second weight snaps to 0 below 0.001. The first two corners are
  blended by `w1 / (w0 + w1)`, then the third by `w2`; with no second
  weight the first and third are blended by `w2 / (w0 + w2)`. Blending two
  poses moves positions linearly and rotations by a normalised linear blend
  in the same hemisphere.

## Merging into the pose

- The weight is clamped to 0..1 and multiplied per bone by the sequence's
  weight list (`weightlistindex`, one float per bone of its model); bones
  the sequence's model lacks get 0 and keep their pose.
- Delta sequences: the rotation is scaled by the bone's weight (the half
  angle scales: the vector part becomes `v * sin(t * asin|v|) / |v|`, `w`
  keeps its sign) and applied after the current rotation for `POST`
  sequences (current times delta) or before it otherwise; positions add the
  delta times the weight.
- Other sequences: rotations slerp and positions lerp towards the
  sequence's by the bone's weight.
- `WORLD` sequences blend in world space (not handled).

## Layers

- Accumulating a sequence: its own pose; for `LOCAL` sequences its
  local-context layers are accumulated onto that pose at weight 1; the
  result is merged; then its other automatic layers follow. There is no
  separate pass before the merge in this build (the layer's `POST` bit is
  not consulted).
- An automatic layer with `start == end` plays at the parent's cycle and
  weight. Otherwise the ramp index is the parent's cycle, or for `POSE`
  layers the value of the layer's pose parameter; outside `[start, end)`
  the layer is skipped; the ramp is `(i - start) / (peak - start)` before
  the peak, `(end - i) / (end - tail)` after the tail and 1 between;
  `SPLINE` smooths it to `3s² - 2s³`; past the tail `XFADE` layers weigh
  `s * w / (1 - w + s * w)`, `NOBLEND` layers `s`, others `s * w`. The
  layer's cycle is `(cycle - start) / (end - start)`, or the parent's cycle
  for `POSE` layers. Local layers always ramp over the cycle.
- Every `AUTOPLAY` sequence accumulates at weight 1 with cycle
  `frac(time * cycles per second)`, after the main sequence and the overlay
  (gesture) layers.

## Layout notes

- Sequence descriptor: automatic layers at 148 (count) and 152 (offset),
  each 24 bytes (sequence and pose parameter as 16-bit indices, flags,
  start, peak, tail, end); weight list at 156; pose keys at 160 (`groupsize`
  0 plus 1 floats); keyvalues at 172 (offset) and 176 (size); cycle pose
  parameter at 180.
- Sequence flags used here: delta 0x4, autoplay 0x8, post 0x10, cyclepose
  0x80, realtime 0x100, local 0x200, world 0x4000. Layer flags: spline
  0x40, xfade 0x80, noblend 0x200, local 0x1000, pose 0x4000.

## Whole-frame animations

Animation descriptors with flag 0x40 store every frame rather than
run-length channels (the container ride and the intro's `ghostAnim`
models). Long ones are split into sections (`section frames` per
section); a section's data sits in the `.mdl` (block 0, offset from the
descriptor) or in the `.ani` block file, and frames count from the
section's start.

- Header: constants offset, frame data offset, frame length (all from the
  header's start), 12 unused bytes, then one flag byte per bone.
- Each bone reads its rotation, then its position. Constants and frame
  data are two streams; frame `n`'s data starts at
  `frame offset + n * frame length`.
- Flag bits: 0x01 position as three half floats in the constants; 0x02
  rotation as a Quaternion48 (16/16/15 bits and a w sign) in the
  constants; 0x04 / 0x08 the same two in the frame data; 0x10 position as
  three floats in the frame data; 0x20 the same in the constants; 0x40
  rotation as a 48-bit smallest-three quaternion in the constants; 0x80
  the same in the frame data.
- Smallest-three: three 15-bit components (offset 16384, scale 1/23168);
  the top bits of the first two words give the index of the first stored
  component, the next two follow it wrapping round, and the missing one is
  rebuilt from unit length, negated when the third word's top bit is set.
- Checked against the real game: the intro wake-up view path recorded
  with `getpos` (0x40/0x80/0x10/0x20 bones), and Wheatley's entry rail
  (`wheatley_entry.mdl`, 0x02/0x01/0x08/0x04 bones) whose attachment the
  game reports at the same place and angles.

## Sequence events

- Event 1100 sends the input named in its options to the model's own
  entity (activator and caller itself). The container ride's master model
  fires `FireUser1`-`4` this way for its impacts (`anim2`, 135 s); the
  game's IO trace shows them at the cycles the events give.
- Named events: `AE_CL_PLAYSOUND` plays the sound in the options from the
  model; `AE_CL_CREATE_PARTICLE_EFFECT` starts the effect (options:
  effect, attach type, attachment), e.g. the vault glass's
  `impact_glass_dust_lrg` at `glass_D2_attach`.

## In the installed models

- Security cameras and floor turrets aim with autoplay delta grids over
  `aim_yaw` / `aim_pitch` (`auto_leftright`, `auto_updown`). The camera's
  yaw deltas are relative to its -180 pose: at yaw 0 the swivel bone turns
  180 degrees, so without the autoplay layers a camera faces its wall.
- 211 sequences carry keyvalues (faceposer gesture types), 146 have
  automatic layers, 63 have pose keys.
- Chell's `player.mdl` takes `head_yaw`, `head_pitch`, `body_yaw`,
  `body_pitch`, `move_x` and `move_y` from `player_animations.mdl` (11 blend
  grids, 19 sequences with automatic layers).
