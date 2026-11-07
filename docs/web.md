# Playing in the browser

portal2rust also builds to WebAssembly and runs in a browser tab. One machine, the one with
Portal 2 installed, runs `p2web`: it serves the page, the browser build and the game's files from
its install. Any machine on the network opens the page and plays; the game itself runs entirely
in the visiting browser (WebGPU for rendering, Web Audio for sound), and saved games stay in that
browser.

```bash
rustup target add wasm32-unknown-unknown   # once
cargo run --release -p portal2_web -- run  # build for the browser, then serve on port 8080
```

`p2web` prints the addresses to open, for example `https://192.168.1.20:8080/`. The page is served
over HTTPS with a self-signed certificate made for the machine's name and addresses (browsers only
offer WebGPU to secure pages, and a page from another machine is only secure over HTTPS), so each
browser warns once: choose *Advanced* and *Proceed*. To avoid the warning, pass a trusted
certificate of your own, for example one made with [mkcert](https://github.com/FiloSottile/mkcert):
`--cert cert.pem --key key.pem`.

Browsers with WebGPU: current Chrome and Edge (Windows, macOS, ChromeOS, Android), Firefox 141 or
newer on Windows, Safari 26 or newer.

## Commands

| Command | |
| --- | --- |
| `p2web build [--dev] [--opt]` | Build the browser package into `target/web/pkg`. `--dev` builds faster with less optimisation; `--opt` also runs binaryen's `wasm-opt -O3` if it is installed. |
| `p2web serve [--port 8080] [--bind 0.0.0.0]` | Serve the page. `--http` serves plain HTTP (WebGPU then only works on `http://localhost`); `--cert`/`--key` use your own certificate; `--game <dir>` points at the Portal 2 folder; `--data <dir>` moves p2web's own files. |
| `p2web run` | `build`, then `serve`. |
| `p2web prepare [--maps sp]` | Run the desktop build over every single-player map and record which files each one reads (see below). Needs `cargo build --release -p portal2` first. |

The page takes the game's command-line options in its address: `?map=sp_a2_laser_intro`,
`?novid`, `?fx=off`, `?+sv_cheats=1`, or `?args=-novid --fx medium` for several at once.

## How the files get to the browser

The game reads its files synchronously, the way it does from the disk on the desktop; a browser can
only fetch them asynchronously. The page's loader (`crates/portal2_web/web/loader.js`) bridges the
two with a cache it fills ahead of the game:

1. **Index.** The page downloads one index of every file the game can see (the merged `update`,
   DLC, `portal2` and `platform` search paths: about 60,000 paths, a few hundred KiB compressed).
   Existence checks and directory listings are answered from it without asking the server.
2. **Phases.** The game logs every file it reads, per *phase*: `boot` for the menus, then one phase
   per map. The browser reports each phase's list to the server (`POST /api/learn/<phase>`), which
   keeps the lists in its data folder (`%LOCALAPPDATA%/portal2rust/web/manifests`).
3. **Prefetch.** Before a map loads, the game asks the loader for the map's list
   (`p2r.ensure(map)`) and waits behind its loading screen while the loader fetches it: what this
   browser already has from an earlier visit comes from its IndexedDB, the rest arrives from the
   server in four parallel streams ("packs", gzip-compressed when the server is another machine).
   The menus' files are fetched the same way while the page shows its Play button, at the same time
   as the engine itself downloads and compiles.
4. **Fallback.** A file no list has yet is fetched with a synchronous request on the spot. That is
   slow if it happens a lot, but each one happens once: the file is then on the list for that map,
   for every browser.
5. **The next map.** The server also remembers which map browsers load after each one
   (`POST /api/next/<map>`). Fifteen seconds into a map, the loader fetches the next map's files in
   the background, so a level change usually finds them in memory already.

So the first visit to a map costs one download of what it needs, and every later visit loads from
the browser's own disk. To make even the first visits fast, `p2web prepare` runs the desktop build's
campaign smoke test (`--smoke sp`) with `P2R_ACCESS_LOG` pointing at the lists folder, which records
what every map reads as it loads; the browsers then add what maps read later in play.

Downloaded files stay in the browser's IndexedDB (*Clear downloaded game files* on the page removes
them; saved games are kept). The game keeps up to about 1 GiB of files in memory beyond the menus'
and the current maps'.

## Saved games

Saves, quick saves, autosaves, `fx.cfg` and the game instructor's counters are written to the
browser's IndexedDB (database `portal2rust-saves`), under the same names as on the desktop. They
belong to the browser and to the page's address: opening the page under another address (the
machine's name instead of its IP, another port) starts with no saves.

## Performance

The browser build is the same engine; what differs is the platform:

- **One thread.** Bevy runs its systems and its render extraction on the page's single thread
  (browsers give WebGPU to one thread, and Bevy's task pools do not use web workers), where the
  desktop spreads them over all cores and renders on a thread of its own. Physics (Avian, Parry)
  runs single-threaded too.
- **WebGPU.** Rendering goes through the browser's WebGPU implementation (Dawn in Chrome, wgpu in
  Firefox), which validates every call; the same pipelines and shaders run as on the desktop, and
  BC (DXT) textures upload as they are where the GPU has them (expanded to RGBA8 where it does not,
  as on most phones).
- **Build.** The `web` profile compiles with fat LTO, one codegen unit, `panic = "abort"` and
  128-bit SIMD; `--opt` adds `wasm-opt -O3`.
- **Resolution.** The canvas renders at the page's CSS size rather than the display's device
  pixels, like the desktop build's default window.

Two browser-specific costs were found by profiling and removed:

- **WebGPU limits.** wgpu's WebGPU backend reads a device's limits back from the browser (some
  thirty calls into JavaScript) every time they are asked for, and Bevy asks on every draw: in busy
  views that was a sixth of the frame. `p2web build` compiles against a copy of the locked wgpu
  release that keeps the first answer (made in p2web's data folder and passed to Cargo with
  `--config patch...` for that build only; `Cargo.lock` is restored afterwards). The page also keeps
  `device.limits` and `device.features` as plain objects.
- **Heap growth.** Rust's allocator grows the page's memory 64 KiB at a time, and each
  `memory.grow` is costly in the browser: loading a map grew it ten thousand times and stalled for
  over six seconds. The browser build's allocator (`crates/portal2/src/web.rs`) grows it by a
  quarter of the heap at a time instead.

Measured on the author's machine (Chrome, WebGPU, 1280 x 705), at the benchmark views of
`re/shots/bench.txt`, against the desktop build at the same size with vsync off. The browser is
held to the display's 60 Hz, so its CPU time per frame is the comparable figure:

| View | Desktop frame time | Browser CPU per frame | Browser frame rate |
| --- | ---: | ---: | ---: |
| Both portals open, in view | 7.1 ms | 5.3 ms | 58 fps |
| Lasers and an emancipation grill | 17.0 ms | 7.1 ms | 38 fps (GPU-bound) |
| A large underground vista | 16.9 ms | 7.5 ms | 60 fps |
| Two portals across a room | 20.0 ms | 5.6 ms | 60 fps |

Loading `sp_a1_intro3` (920 files, 255 MiB) takes about 1.5 s from Play to the map on a second visit
(files from IndexedDB) and a few seconds more on the first, depending on the network; packs stream
at about 150 MiB/s from the same machine and compress to 54 % for other machines.

## Files

| | |
| --- | --- |
| `crates/portal2_web` | `p2web`: build, HTTPS server, asset index, packs and phase lists. |
| `crates/portal2_web/web` | The page (`index.html`) and its loader (`loader.js`). |
| `crates/portal2_core/src/web.rs` | The game's side of the loader: the asset stream as a `FileStore`, saved files, map prefetch. |
| `crates/portal2_core/src/userfs.rs` | The player's files: ordinary files on the desktop, IndexedDB in the browser. |
| `crates/source_formats/src/fs.rs` | `FileStore` (search paths that are not on this disk) and the per-phase access log. |
| `crates/portal2/src/web.rs` | Starting in the browser: the canvas window and the pointer lock. |
