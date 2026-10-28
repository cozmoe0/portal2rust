# portal2rust

An engine reimplementation that plays **your own copy of Portal 2**, written from
scratch in Rust on top of [Bevy](https://bevyengine.org/).

## What this is (and is not)

This project is a new engine, in the spirit of OpenMW or OpenRCT2. It reads the
original game's data files (maps, models, textures, materials, sounds, scripts)
from a local Portal 2 installation at runtime and aims to reproduce the game's
behaviour as closely as possible.

- **A Portal 2 installation is required.** Nothing from the game ships with this
  repository: no maps, models, textures, sounds, scripts, text or code.
- **All code here is original.** File-format readers are written from public
  format knowledge. Reverse engineering of the installed binaries (IDA, REA) is
  used only to learn facts such as file layouts, console-variable defaults and
  gameplay constants; decompiled code is not translated into this codebase.
- **No game content is recreated.** Characters, levels, dialogue, art and music
  come only from your installation. Anything missing is drawn as a generic
  placeholder (a magenta/black checkerboard or a plain box).

Because of this, the result cannot become a standalone copy of Portal 2; it is
a different engine for the game you already own.

## Running

```bash
cargo run --release -p portal2
```

Without a map the game starts like the original: the Valve intro video, then
the main menu over its background movie, with Single Player (Continue, New
Game with chapter select, Load Game), the pause menu in a game (`Esc`), the
quit confirmation and the loading screens. Give a map to go straight into it:

```bash
cargo run --release -p portal2 -- sp_a2_laser_intro
```

The installation is found in the default Steam library, any library listed in
Steam's `libraryfolders.vdf`, or the folder given by the `PORTAL2_DIR`
environment variable (the folder that contains `portal2.exe`).

Useful options:

| Option | Meaning |
| --- | --- |
| `<map>` / `--map <map>` | Map to load, e.g. `sp_a1_intro3` (none: start at the main menu) |
| `-novid` / `-nomenuvid` | Skip the intro video / show the menu background still instead of the movie |
| `--pos x,y,z` / `--ang p,y,r` | Override the start view (Source units / degrees) |
| `--exposure <scale>` | Fixed exposure multiplier |
| `--fullbright` | Ignore lighting (debugging) |
| `--screenshot <file> --frames <n>` | Capture a screenshot after `n` frames and exit |
| `--noclip` | Start in free flight (toggle in game with `V`) |
| `--autopilot f2,j,w1` | Scripted input for movement tests (`f/b/l/r` move, `j` jump, `d` crouch, `t` turn, `w` wait) |
| `--spawn class@x,y,z` | Spawn an entity after the map loads (repeatable), e.g. `prop_weighted_cube@0,0,64` |
| `--io-debug` | Log every entity output as it fires |
| `--probe` | Log what the view ray hits (distance and owning entity) |
| `--no-static-props` / `--no-entity-models` | Skip prop rendering (debugging) |
| `+command args` | Console command run once the first map has loaded, e.g. `+sv_cheats 1 +setpos 0 0 64` |
| `--shots <file>` | Visit each view in a shot list and save `re/out/ours/<name>.png` (parity checks) |
| `--smoke sp\|<maps>` | Run each map briefly and write `re/out/smoke.txt` (script errors, unhandled inputs) |
| `--chain <map>` | Take every level's exit from `<map>` on through the campaign at `--chain-speed` (default 8) times real time, for `--chain-count` maps; writes `re/out/chain.txt` (ride times, next maps, arrival placement) |
| `--nosound` | No audio output (the batch modes above never open the device) |
| `--snd-record <file.wav> [--snd-seconds n]` | Also write the first `n` seconds (default 10) of the mixed output to a WAV file |

Controls come from your own `portal2/cfg/config.cfg` key bindings, as in the
original game (`+forward`, `+jump`, `+attack`, `+use`, ...). `Esc` opens the
pause menu, the console key (`` ` ``) the developer console, `V` toggles
noclip.

The interface is drawn like the game's: its schemes, layout files, fonts
(rasterized with GDI the same way, effects included) and localized text,
blended the way the game blends it. The HUD shows the portal crosshair,
close captions (`closecaption 1`) and chapter titles; `cl_drawhud 0` hides
it.

## Console and scripts

The engine has the original's console: console variables with the game's
defaults, commands (`map`, `changelevel`, `setpos`, `ent_fire`, `noclip`,
`give_portalgun`, ...), aliases, `bind`, `exec`, `wait`, and the startup
sequence (`config.cfg`, then `valve.rc`). Map scripts run on
`crates/squirrel`, an interpreter for Squirrel 2.2 (the version the game
embeds), with the game's VScript bindings in `crates/portal2/src/script`.

The portal gun is available from the start unless the map has one to pick
up; override with `--portalgun none|blue|both`.

## Sound

Sounds play from the installed WAV and MP3 files through the game's own
sound scripts, sound operator stacks (`scripts/sound_operator_stacks.txt`)
and mixers (`scripts/soundmixers.txt`, including the mix layers that duck
everything else under dialogue), on a software mixer in `crates/portal2/src/audio`.
`ambient_generic`, soundscapes, footsteps, the portal gun and portals,
buttons and script `EmitSound` calls are wired up; room DSP and occlusion
are not yet.

Dialogue plays through choreographed scenes (`logic_choreographed_scene` and
script `CreateSceneEntity`), read from the game's compiled
`scenes/scenes.image`: speak events play the lines on their actors, trigger
events fire the scene's outputs and scenes complete at their real length.
Gestures layer onto the actors (Wheatley and the other cores now appear
with their model). Models blend their sequences by pose parameter, with
per-bone weights and automatic and autoplay layers; security cameras use
this to turn and watch the player. The single-player scenes have no look-at,
flex or lip-sync data, so nothing of that kind is played.
`closecaption 1` shows captions in the game's own panel layout and font. `snd_list` lists the playing channels, `play <sound>` plays one.
The volume comes from your `config.cfg` (`volume`). Portal openings are drawn
with a simple coloured ring around the linked view.

Games save and load like the original: `save <name>` / `load <name>`, the
quick save and load keys from your binds, autosaves from the maps' autosave
entities and on level transitions, and a reload of the latest save a few
seconds after dying. Saves go to `%LOCALAPPDATA%\portal2rust\save` (set
`P2R_SAVE_DIR` to change it), never to the game's own save folder; a save
holds the map's entities, the script state and the player, and loading
carries on from there. `--save-test <maps>` checks that loading a save and
saving again gives the same save.

## Layout

- `crates/source_formats` – engine-agnostic readers: KeyValues, VPK, VTF, VMT,
  BSP, MDL/VVD/VTX, map pakfiles and the layered game filesystem, plus sounds,
  sound scripts, soundscapes, captions, choreographed scenes (VCD and
  `scenes.image`) and particle definitions (DMX/PCF).
- `crates/squirrel` – the Squirrel 2.2 interpreter (tables keep the original's
  slot layout, so iteration order matches).
- `crates/bink` – a Bink 1 video and audio decoder.
- `crates/portal2` – the Bevy game: rendering, world loading, entities and
  I/O, scripting, console, cameras, the interface (`vgui`), HUD, menus
  (`gameui`) and video playback.
- `re/tools` – analysis and parity helpers: `convar_dump.py` (console
  variables and defaults from a game DLL), `capture.py` (reference screenshots
  from the installed game at the views of a shot list, driven through its
  password-protected local network console; your settings files are backed up
  and restored), `capture_ui.py` (the same for the menus and HUD, driving the
  menus with keys posted to the game window), `imgdiff.py` (side-by-side
  comparison with ours),
  `vscript_oracle.py` (runs Squirrel snippets in the game's VM and ours).
- `re/shots/` – shot lists for parity checks.
- `docs/` – notes on formats and engine behaviour, written in our own words.

## Parity checks

```bash
python -I re/tools/capture.py re/shots/chapter1.txt
cargo run --release -p portal2 -- --shots re/shots/chapter1.txt
python -I re/tools/imgdiff.py re/out/ref re/out/ours re/out/diff
```

A shot line may end with console commands to run at that view, for example
`r_drawviewmodel 1` to include the first-person weapon (see
`re/shots/models.txt`, which checks model shading).

Campaign checks: `--smoke sp` loads every single-player map and reports
script errors and inputs nothing handles; `--chain sp_a1_intro1` rides each
level's exit elevator (or takes its transition trigger) and reports whether
the map's own logic changed to the right next level.

Copies of game binaries and analysis databases used during reverse engineering
live in `re/bin/`, which is git-ignored and must never be committed.
