# Portal 2 sound runtime

How the shipped engine turns a started sound into speaker volumes, as
implemented in `crates/portal2_audio/src`. Facts come from reading the
installed `engine.dll` (and `server.dll`/`client.dll` strings) in a
disassembler and from the installed script data; everything is described in
our own words. The file formats themselves are in `audio_formats.md`.

## Pipeline

1. A sound starts from a sound entry (or a bare wave path). The entry picks
   a wave (random among `rndwave`), a volume, pitch and sound level within
   its ranges, and a channel slot.
2. The sound joins up to eight mix groups (below).
3. The entry's start stack runs once; it may delay the sound, block it, stop
   other sounds, start other entries or change mix layers.
4. Every frame the update stack runs and produces six speaker volumes
   (left front, right front, left rear, right rear, centre, LFE), a pitch, a
   DSP amount and housekeeping outputs. Entries without stacks use the
   library's `update_default`.
5. A stop request runs the stop stack once; the sound keeps playing while an
   update stack's `stop_hold` output stays above zero (fade-outs), then ends.

A new sound on the same entity and channel slot replaces the old one, except
on `CHAN_AUTO` (0) and `CHAN_STATIC` (6).

## Operator stacks

`scripts/sound_operator_stacks.txt` has `start_stacks`, `update_stacks`,
`stop_stacks` (and `prestart_stacks`, unused by Portal 2). There are no
built-in default stack names in the engine. An entry's `operator_stacks`
block holds `start_stack` / `update_stack` / `stop_stack` blocks; inside one,
`import_stack <name>` copies a library stack's operators, a block with an
existing operator name overrides that operator's parameters, and a new name
appends an operator.

Each operator has typed inputs (`input_*`, literal or `@op.output_name`),
outputs, `execute_once` and `input_execute` (the operator is skipped while
it is not above zero). `input2[*]` broadcasts a scalar into every speaker;
speaker names (`left_front`, ...) set individual speaker components.

### Operators checked in the binary

* `calc_falloff` (inputs `input_distance`, `input_level`, both default 1;
  output default 1): the distance multiplier is `1000 / 10^(level/20) /
  snd_refdist` (0 for level 0); the gain is the engine's distance gain with
  unit input gain.
* Distance gain: `relative = distance · mult · 10^(distance/1200 ·
  snd_foliage_db_loss / 20)`. Gain is `1 / relative`, or 10 when `relative`
  ≤ 0.1. Above 0.5 the gain is compressed: with `p = 2.5`, reduced by
  `(level − 90) · 1.7 · 0.02` when the level derived back from the
  multiplier exceeds 90 dB, `gain = (1 − 1 / (gain^p · y)) · snd_gain_max`
  where `y = −1 / (0.5^p · −0.5)`. Below `snd_gain_min` the gain becomes
  `snd_gain_min · (2 − relative · snd_gain_min)`, at least 0.001.
  Defaults: `snd_refdist` 36, `snd_refdb` 60 (the 1000 above is
  10^(60/20)), `snd_foliage_db_loss` 4, `snd_gain_max` 1, `snd_gain_min`
  0.01, `snd_gain` 1.
* `calc_spatialize_speakers` (`input_radius` 0, `input_distance` 0,
  `input_rear_stereo_scale` 0.75, `input_position`): a mono amount is 1
  inside half the radius and falls linearly to 0 at the radius. The device
  then computes speaker gains from the direction to the source:
  * Angles are measured from the listener's right vector, so 90° is ahead;
    elevation above 45° (folded into 0..90) adds `(elev − 45) / 45` to the
    mono amount.
  * Headphones (`snd_surround_speakers` 0): each ear (0° and 180°) gets
    `1 − (angle/180)^1.5`, blended towards 0.9 by the mono amount.
  * Two speakers: four virtual speakers at 45° (front right), 135° (front
    left), 225° (rear left) and 315° (rear right) get `1 − (angle/90)^1.5`
    within 90°. Front ones blend towards 0.9 by the mono amount, rear ones
    are scaled by `1 − mono`; each rear is added to its front side times the
    rear stereo scale, clamped to 0..1.
* `get_soundmixer` (`mixgroup` parameter, else the sound's first group):
  outputs the group's volume, level and DSP factors under the current mixer
  (below).
* `get_source_info` (`source` `emitter` or `entity`): position, angles,
  volume, level (dB), pitch (ratio), radius, source count, entity index.
* `sys_output` `output`: `dsp`, `position`, `speakers`, `facing` (stored as
  `2x − 1`), `distvar`, `pitch` (ratio), `delay` (start offset; negative
  starts part-way in), `stop_hold`, `mixlayer_trigger`, `save_restore`,
  `block_start`. `volume` is accepted but has no effect.
* `math_float` `apply`: set, add, sub, mult, div (0 unless the divisor is
  positive), mod (same), max, min, invert (`1 − a`), greater_than,
  less_than, greater_than_or_equal, less_than_or_equal, equals, not_equal,
  invert_scale (`1 − (1 − a) · b`).
* `math_remap_float`: inputs `input` 0, `input_min` 0, `input_max` 0.1,
  `input_map_min` 0, `input_map_max` 1; `clamp_range` and `default_to_max`
  default true. A reversed output range swaps both ranges. With an empty
  input range the result is the low or high end depending on which side the
  input is, and `default_to_max` decides the case where it equals the range.
  Inputs at or beyond either end of a proper range map to that end;
  otherwise linear. The result is clamped to the output range when
  `clamp_range` is set.
* `get_entry_time`: entry elapsed, sound elapsed, stop elapsed, sound
  duration.

Operators whose exact formulas were not traced yet are implemented from
their parameters and the way the stacks use them: `calc_source_distance`
(distance to the listener), `calc_occlusion` (no occlusion yet: 1),
`calc_distant_dsp`, `math_func1`, `math_random`, `math_delta`,
`math_speakers` (per-speaker `math_float`), `iterate_merge_speakers` (one
source), `sys_stop_entries` (other playing sounds matching entry name or
substring, entity and channel; the oldest beyond `input_max_entries` stop;
`match_entry` `NONE` matches nothing), `sys_start_entry`,
`sys_block_entries`, `sys_mixlayer`, `game_view_info`,
`calc_angles_facing`, `math_logic_switch`, opvars, `get_map_name`,
`sys_platform`.

## Mix groups, mixers and layers (`scripts/soundmixers.txt`)

* `MixGroups` rules: name, wave directory substring (lowercase), emitting
  class substring (`localPlayer` matches the local player), channel name,
  minimum and maximum sound level, then priority (default 50), is-ducked,
  causes-ducking, duck target (percent) and total trigger volume (percent).
  A sound takes, in rule order, every group whose rule matches, up to eight.
  Rules with the same name share one group; the last group, `All`, matches
  everything.
* `SoundMixers` give each group volume, level, DSP, solo and mute. A group a
  mixer does not list is left at 1/1/1. Up to 33 mixers, 145 rules, 17
  layers. `snd_soundmixer` defaults to `Default_Mix`; soundscapes switch it.
* `MixLayers` use the same rows ("first three fields are multipliers, last
  two additive"). Each layer has an amount (0..1). For every group listed in
  layers with a positive total amount `A`, a layer with amount `a` adds
  `(value − 1) · a²/A` to the group's volume, level and DSP factors (from 1)
  and `value · a²/A` to solo and mute (from 0), each clamped to 0..1.
* Group values: level = mixer level × layer level × `snd_mixer_master_level`,
  DSP likewise with `snd_mixer_master_dsp`, volume = mixer volume × layer
  volume × (1 − max(mixer mute, layer mute)). With `S` the largest solo over
  all groups (mixer and layers; skipped when `snd_disable_mixer_solo` is
  set), a group with solo `s` is further scaled by `1 − (1 − s/S) · S`.
* `LayerTriggers` (layer, triggering group, threshold, factor, attack,
  release): every 0.1 s of host time, the `mixlayer_trigger` outputs of all
  playing sounds are summed per group. A layer's target amount is the
  largest factor among its triggers whose group total exceeds the
  threshold (0 when none). The amount moves towards the target by `0.1 /
  time · largest factor` per update (attack time when rising, release when
  falling; 0 jumps). This is how dialogue ducks music, ambience and effects.

## ambient_generic

Spawnflags 1 play everywhere (level 0), 16 start silent, 32 not looped.
`radius` (default 1250) becomes the level `40 + 20·log10(radius / 36)` dB
(truncated); `health` is the volume (0–10), `pitch` the pitch.
`SourceEntityName` emits from that entity. `PlaySound` starts a stopped
looping ambient or restarts a one-shot, `StopSound`, `ToggleSound`, `Volume`
(0–10), `Pitch`, `FadeIn` and `FadeOut` (seconds). These follow the Source
SDK's behaviour; this build's details were not traced.

## Code-side sound names

From `server.dll`/`client.dll` strings: `Weapon_Portalgun.fire_blue`,
`fire_red`, `HoldSound`, `powerup`; `Portal.open_blue`, `open_red`,
`close_blue`, `close_red`, `fizzle_moved`, `fizzle_invalid_surface`,
`ambient_loop`, `button_down`, `button_up`, `button_locked`,
`elevator_ding`; `PortalPlayer.EnterPortal`, `ExitPortal`, `ObjectUse`,
`UseDeny`, `ObjectUseNoGun`, `UseDenyNoGun`, `FallRecover`; `Prop.Fizzled`;
`Laser.BeamLoop`.

## Footsteps

Step sounds use the surface property's `stepleft`/`stepright` entries on
`CHAN_BODY`, alternating, every 400 ms below 220 units/s and 300 ms above
(+100 ms ducked), volume 0.2 walking / 0.5 running, ×0.65 ducked; none
below 10 units/s or in the air (Source SDK behaviour, not traced in this
build).

## Not covered yet

* Room DSP (`dsp_presets.txt`, soundscape `dsp`) and the `dsp` outputs.
* Occlusion traces, doppler, `distvar` wave pairs, surround output.
* Choreographed scenes (most voice-over plays through them).
