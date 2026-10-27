# Portal 2 audio data

Notes behind `source_formats::{wav, soundscript, soundscape, captions}`. Facts
come from public format knowledge, from looking at the installed data, and from
reading the shipped `engine.dll`, `client.dll` and `soundemittersystem.dll` in a
disassembler. Everything here is a description in our own words; no code was
transcribed. Counts are from one Steam install (base game plus both DLC
folders) and only serve as sanity checks.

## Where things live

| Data | Path | Usually found in |
| --- | --- | --- |
| Sounds | `sound/**.wav` | VPKs (8626 files; 2 loose in `portal2/sound`) |
| Sound scripts | `scripts/game_sounds_manifest.txt` and the files it lists | loose `scripts/` folders |
| Soundscapes | `scripts/soundscapes_manifest.txt`, `scripts/soundscapes_<map>.txt` | loose `scripts/` folders |
| Captions | `resource/closecaption_<lang>.dat`, `resource/subtitles_<lang>.dat` | loose `resource/` folders |

Both DLC folders carry longer copies of the sound script manifest (DLC1 also of
the soundscape manifest). With the search order `update`, `portal2_dlc2`,
`portal2_dlc1`, `portal2`, `platform` the most recent copy is the one used, and
it lists the base game's scripts too.

## Wave files

### What is in the install

| Kind | Files |
| --- | --- |
| RIFF/WAVE, format tag 1 (PCM), 16-bit | 6066 |
| RIFF/WAVE, format tag 1 (PCM), 8-bit | 48 |
| No RIFF header: bare MPEG-1 Layer III, 80 kbit/s, mono, 44.1 kHz, CRC on | 2512 |

No other format tag (no ADPCM, no `WAVE_FORMAT_MPEGLAYER3`) occurs. The MPEG
files are all the character voice-over under `sound/vo/` (GLaDOS, Wheatley,
cores, announcer, Cave Johnson, defective turrets). They have no ID3 or
Xing/LAME tags and end exactly on a frame boundary.

The engine decides how to read a sound by its content, not the extension: it
has separate source types for PCM, Microsoft ADPCM, MPEG audio named `.mp3`,
MPEG audio stored under a `.wav` name, and XMA (consoles). The reader does the
same: `RIFF` means WAVE, an MPEG frame sync (`FF Ex`) or `ID3` means a bare
MPEG stream. MPEG frames are validated (consistent version, layer, rate and
channel count; frame sizes from the standard bitrate tables) but not decoded;
the game itself hands them to the Miles MP3 decoder.

### Chunks the engine uses

* `fmt `: format tag, channels, rate, block align, bits. Bit depths above 16
  draw an "unsupported" warning. For ADPCM the extension carries samples per
  block and the predictor coefficient table.
* `data`: the samples. The sample count is the chunk size divided by the frame
  size; for ADPCM the count is whole blocks times samples per block plus the
  samples held by a short last block (two seed samples plus two per byte per
  channel after the 7-byte per-channel header).
* `cue `: if there is at least one cue point, the first point's sample offset
  becomes the loop start.
* `smpl`: if the chunk lists at least one loop and the first loop's type is 0
  (forward), its start becomes the loop start. The loop end is ignored.
* `VDAT`: sentence (phoneme / word timing) data.

Whichever of `cue ` and `smpl` comes later in the file decides the loop start.
A looping sound always plays to the end of the data and jumps back to the loop
start; the region end from `smpl` or `LIST`/`ltxt` has no effect. 451 files
have a loop start. `LIST` chunks (`INFO` text, `adtl` labels and regions) are
not used by the engine; the reader keeps `adtl` entries for inspection.

Other chunks seen in the data are skipped: `fact` and editor metadata (`bext`,
`CDif`, `FLLR`, `iXML`, `regn`, `minf`, `umid`, `elm1`, `DGDA`, `ovwf`, `filr`,
`ID3 `, `SMED`, `JUNK`, `ytme`, `LGWV`).

### Microsoft ADPCM

Each block starts with, per channel, a predictor index (1 byte), the step size
(`i16`), and two seed samples (`i16` each; the second one is the older sample
and is output first). The rest of the block is 4-bit codes, high nibble first,
alternating channels for stereo. Each code is sign-extended, the prediction is
`(s1 * c1 + s2 * c2) >> 8` with the predictor's coefficient pair, the output is
the prediction plus code times step (clamped to 16 bits), and the step is scaled
by a 16-entry adaptation table and never drops below 16. The standard seven
coefficient pairs are used when the format chunk does not list its own.

### `VDAT` sentence text

Only two files carry it (`sound/labs/ding_on.wav`, `ding_off.wav`), both with
empty word lists and the option `voice_duck 1`. The text form is line based:
`VERSION 1.0`, then named blocks in braces: `PLAINTEXT` (the spoken text),
`WORDS` (`WORD <text> <start> <end>` followed by a braced list of
`<code> <phoneme> <start> <end> <volume>` lines), `EMPHASIS` (`<time> <value>`
pairs) and `OPTIONS` (`<key> <value>` lines). Other blocks (old files have a
`CLOSECAPTION` block) are skipped by brace depth.

## Sound scripts

### Manifest

The manifest is one block whose children name script files:

* `precache_file` and `autocache_file`: normal script files.
* `preload_file`: the file's sounds are marked for preloading.
* Anything else draws a "bogus file type" warning and is skipped; the shipped
  manifest has one such key on purpose (`new_sound_scripts_must_go_below_here`).

KeyValues conditionals apply (`[!$GAMECONSOLE]` is true on PC). The base
manifest lists `npc_sounds_energy_ball.txt` twice.

### Entries and duplicates

Every top-level block with at least one child is a sound entry named by its
key (case-insensitive). When a name is already registered, the new definition
is thrown away: **the first definition wins**. The only exception is the map
override file `maps/<map>_level_sounds.txt`, whose entries replace existing
ones. The install has 5862 entries from 53 files and 6 ignored duplicates.

Keys inside an entry (case-insensitive, read in order, later keys overwrite):

| Key | Meaning |
| --- | --- |
| `soundentry_version` | read first; default 1, Portal 2's operator-stack entries use 2 |
| `channel` | see channels below |
| `volume` | `VOL_NORM` (1.0) or a range |
| `pitch` | `PITCH_NORM` 100, `PITCH_LOW` 95, `PITCH_HIGH` 120, or a range (percent) |
| `wave` | adds one wave |
| `rndwave` | block; every child's value adds a wave. Version 2 entries keep at most 63 |
| `soundlevel`, `CompatibilitySoundlevel` | a `SNDLVL_` name or a numeric range |
| `attenuation`, `CompatibilityAttenuation` | an `ATTN_` name or a numeric range, turned into a sound level |
| `play_to_owner_only` | integer flag |
| `delay_msec` | integer, negative values become 0 |
| `operator_stacks` | Portal 2 operator stacks (`start_stack`, `update_stack`, `stop_stack`), kept raw |

Other keys are ignored silently. Defaults: channel auto, volume 1, pitch 100,
sound level 75, no delay, version 1. The `Compatibility*` forms set a flag on the sound level that
asks for the old attenuation behaviour; they require a fixed value.

Value parsing:

* A range is the value split at commas (empty parts skipped), each part read
  like C `atof`: `"95, 105"` is 95 to 105, a single number is fixed. Names such
  as `VOL_NORM` only work as the whole value.
* Sound level names: an exact table (case-insensitive) of `SNDLVL_NONE` 0,
  `SNDLVL_IDLE` 60, `SNDLVL_STATIC` 66, `SNDLVL_NORM` 75, `SNDLVL_TALKING` 80,
  `SNDLVL_GUNFIRE` 140 and `SNDLVL_<n>dB` for n from 20 to 110 in steps of 5,
  120, 130, 140, 150 and 180. Failing that, any `SNDLVL_` prefix
  followed by a number from 1 to 180 is accepted (`SNDLVL_125dB` occurs).
  Anything else gives 75 with a developer message. A value that does not start
  with `SNDLVL_` is read as a numeric range, so `SNDLVL75dB` (missing
  underscore, used in a few soundscapes) reads as 0.
* Attenuation names: `ATTN_NONE` 0, `ATTN_NORM` 0.8, `ATTN_IDLE` 2, `ATTN_STATIC`
  1.25, `ATTN_RICOCHET` 1.5, `ATTN_GUNFIRE` 0.27; unknown names give 0.8. An
  attenuation `a` becomes the sound level `50 + 20 / a` dB (truncated), or 0
  when `a` is 0.

### Channels

A value that does not start with `chan_` (case-insensitive) is read as an
integer. Otherwise it must be one of `CHAN_AUTO` 0, `CHAN_WEAPON` 1,
`CHAN_VOICE` 2, `CHAN_ITEM` 3, `CHAN_BODY` 4, `CHAN_STREAM` 5, `CHAN_STATIC` 6;
unknown names give 0 with a warning (there is no `CHAN_VOICE2` in this build's
table). Install totals: voice 3315, auto 1670, static 699, body 103, item 51,
weapon 24.

### Sound characters

All engine DLLs share the same prefix scanner: any number of the twelve
characters `* ? ! # @ ( > < ^ ) } $` at the start of a wave name are skipped
before the path, and a character "is set" if it occurs anywhere in that leading
run.

| Char | Meaning |
| --- | --- |
| `*` | stream from disk |
| `?` | voice chat data, not a file |
| `!` | sentence name, not a file |
| `#` | dry: no DSP room effect |
| `}` | fast, non-interpolated pitch shift |
| `@` | omnidirectional |
| `(` | directional stereo |
| `)` | spatialised stereo |
| `^` | distance variant: left channel near, right far |
| `<` | directional: left channel faces the listener, right faces away |
| `>` | doppler: left channel approaching, right receding |
| `$` | the client emits the sound's close caption even when it normally would not |

Only one spatial mode applies. When several are present the engine checks them
in the order `>`, `<`, `^`, `@`, `)`, `(` and keeps the last one found, so `(`
beats `)` beats `@` and so on. Install usage: `)` 1532, `*` 303, `#` 21, `^`
11, `@` 7, `$` 1.

Wave paths are relative to `sound/`. Of 9628 wave references in the scripts,
708 name files that exist in no VPK and no loose folder of the install (Portal
1 GLaDOS lines under `vo/aperture_ai`, Half-Life 2 weapons and physics sounds,
cut Cave Johnson and co-op lines, a few `.mp3` music tracks). They are dangling
in the shipped data, not a reader problem.

## Soundscapes

### Files and lookup

`scripts/soundscapes_manifest.txt` is one block whose children must be `file`
keys (others warn). After the listed files the client also reads
`scripts/soundscapes_<map>.txt` for the current map if it exists and is not in
the manifest. Every top-level block with at least one child is appended to one
list without checking names; lookup walks that list from the end, so **the
last definition of a name wins** (the opposite of sound scripts). The install
has 422 definitions of 413 names in 18 files.

### Keys

Soundscape level (only the active, outermost soundscape applies the DSP and
mixer keys):

| Key | Meaning |
| --- | --- |
| `dsp` | room DSP preset index |
| `dsp_player` | second (player) DSP preset index |
| `dsp_volume` | DSP mix volume |
| `fadetime` | seconds to crossfade into this soundscape |
| `soundmixer` | sound mixer name |
| `playlooping`, `playrandom`, `playsoundscape` | sound rules, below |

`playlooping`: `wave`, `volume`, `pitch`, `soundlevel`, `attenuation`,
`position`, `origin`, `radius`, `suppress_on_restore`. Ranges are resolved to a
random value once, when the soundscape starts. Without `volume` the volume is 0
and the sound is not started; pitch defaults to 100 and the sound level to 74
dB (one below `SNDLVL_NORM`; it is the client's built-in `ATTN_NORM`
conversion). The volume is multiplied by the enclosing `playsoundscape` volume.

`playrandom`: `time` (seconds between plays), `volume`, `pitch`,
`soundlevel`, `attenuation`, `rndwave` (the only way to name waves here),
`position`, `origin`, `suppress_on_restore`. The parser starts from all zeros.

`playsoundscape`: `name`, `volume` (multiplies the nested sounds), `position`
(added to the nested soundscape's position indices), `positionoverride` (puts
every sound of the nested soundscape at that position unless an outer override
is active), `ambientpositionoverride` and `ambientoriginoverride` (place the
nested soundscape's unpositioned sounds). `soundlevel` is reported as not
supported (one shipped soundscape has it). Nesting stops after 8 levels.

`position` is either `random` or an index into the `env_soundscape`'s
`position0`..`position7` targets; a sound with neither position nor `origin`
plays without spatialisation. In soundscapes a `soundlevel` starting with
`SNDLVL_` goes through the sound level names, anything else is a numeric range.
`origin` (and `ambientoriginoverride`) is split at commas and each part read
like `atof`, so `"1.5, 2, 3;"` works and a stray space-separated number inside a
part is ignored: one shipped origin, `"672.29284,7 -63.278221, 896.031250;"`,
reads as (672.29, 7, 896.03).

## Close captions

### Which files

The client builds the caption dictionary from `resource/closecaption_<lang>.dat`
followed by `resource/subtitles_<lang>.dat`. For each name it tries every
directory on the `GAME` search path and adds each copy it finds; if no copy of
a name exists for the language it falls back to English for that name. With
English the install yields four dictionaries: `portal2` close captions (266
sound effect captions), and subtitles from `portal2_dlc2` (81), `portal2_dlc1`
(206) and `portal2` (2750). No token appears in more than one of them, so the
lookup order (first dictionary with the hash) never matters in practice.
`portal2_dlc2/resource/portal2dlc2_english.dat` is also a `VCCD` file but the
client never opens it.

### `VCCD` layout

All values little-endian.

| Offset | Type | Field |
| --- | --- | --- |
| 0 | 4 bytes | `VCCD` |
| 4 | `i32` | version, must be 1 |
| 8 | `i32` | number of data blocks |
| 12 | `i32` | block size (8192) |
| 16 | `i32` | number of directory entries (at most 65536) |
| 20 | `i32` | file offset of the first block (512-aligned) |
| 24 | 12 bytes each | directory: `u32` hash, `u32` block, `u16` byte offset in the block, `u16` byte length including the terminator |

Blocks follow at the data offset, each exactly one block size long; the file
ends after the last block. Text is UTF-16LE and NUL-terminated, and an entry
never crosses a block boundary. The directory is not sorted on disk; the client
sorts it after loading.

The hash is the standard CRC-32 (the zlib/PNG polynomial) of the token with
ASCII letters lowercased; this matched every token of the shipped `.txt`
sources. Tokens are sound entry names (`announcer.<line>`, `glados.<line>`;
Wheatley's lines use the `sphere03.` prefix).

### Text and tags

Tags are `<name>` or `<name:args>`; the client compares names exactly, with
`B` and `I` upper case and the rest lower case.

| Tag | Effect |
| --- | --- |
| `<clr:r,g,b>` | push a text colour; `<clr>` with no values pops it. When the tag is followed by `Name:` the client looks the speaker up in `resource/captioning_colors.txt`, which can replace the colour |
| `<playerclr:r,g,b:r,g,b>` | first colour if the local player emitted the sound, second otherwise |
| `<B>`, `<I>` | toggle bold / italic |
| `<cr>` | line break |
| `<sfx>` | sound effect caption: hidden when `cc_subtitles` is set, shown without the `cc_predisplay_time` delay |
| `<low>` | low-priority caption |
| `<len:n>` | display for n seconds |
| `<delay:n>` | split point: the text after the tag becomes a separate caption that appears n seconds after the caption starts |
| `<norepeat:n>` | ignore repeats of this caption within n seconds |

Two text prefixes are special: text starting with `!!!` is marked invalid on
the caption item, and text starting with `-->` is the placeholder for a missing
caption, shown only when `cc_captiontrace` is 2 (1 prints "Missing caption" to
the console). Shipped data uses `<clr>` (2791 times), `<low>`, `<sfx>`,
`<norepeat>` and `<len>`.

## Not covered yet

* Decoding the MPEG voice-over to PCM (the reader validates and keeps frames).
* `scripts/sound_operator_stacks.txt` (the stack library entries import with
  `import_stack`), `scripts/soundmixers.txt`, `scripts/dsp_presets.txt` and
  `resource/captioning_colors.txt`.
