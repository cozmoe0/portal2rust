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
cargo run --release -p portal2 -- sp_a2_laser_intro
```

The installation is found in the default Steam library, any library listed in
Steam's `libraryfolders.vdf`, or the folder given by the `PORTAL2_DIR`
environment variable (the folder that contains `portal2.exe`).

Useful options:

| Option | Meaning |
| --- | --- |
| `<map>` / `--map <map>` | Map to load, e.g. `sp_a1_intro3` |
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

Controls: click to capture the mouse, `WASD` move, `Space` jump, `Ctrl`
crouch, `E` use or pick up / drop, left / right mouse button fire the two portals, `V` toggle
noclip, `Esc` release the mouse.

The portal gun is available from the start unless the map has one to pick
up; override with `--portalgun none|blue|both`. Portal openings are drawn
with a simple coloured ring around the linked view.

## Layout

- `crates/source_formats` – engine-agnostic readers: KeyValues, VPK, VTF, VMT,
  BSP, MDL/VVD/VTX, map pakfiles and the layered game filesystem.
- `crates/portal2` – the Bevy game: rendering, world loading, cameras.
- `re/tools` – small static-analysis helpers (for example `convar_dump.py`,
  which lists console variables and their defaults from a game DLL).
- `docs/` – notes on formats and engine behaviour, written in our own words.

Copies of game binaries and analysis databases used during reverse engineering
live in `re/bin/`, which is git-ignored and must never be committed.
