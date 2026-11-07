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
- **Build.** The `web` profile compiles at full optimisation with 16 codegen units, `panic =
  "abort"` and 128-bit SIMD, and without link-time optimisation: fat LTO into one codegen unit
  inlined Bevy's systems into functions so large that V8 optimised them poorly, and frames took
  2.5 times the CPU time (15-17 ms instead of 6-7 in the portal views). `--opt` adds
  `wasm-opt -O3`.
- **Resolution.** The canvas renders at most `r_pixel_budget` megapixels (2.1 by default, about
  1920 x 1080): on a larger or high-density screen the page draws the canvas at a smaller size and
  the browser scales it up, so a 2560 x 1440 page renders 1920 x 1010 pixels. Mouse input is read
  in the canvas's own pixels and stays exact.

### Dynamic quality

A page cannot tell how strong the visiting machine is, and it shares the GPU and its memory with
every other tab and program. What costs most is resolution-sized: the main view (4x multisampled)
and four portal views (two portals and the portals seen inside them), each with its own colour and
depth buffers, plus the overhaul's screen passes. When they no longer fit in video memory the
driver starts paging and frames take ten times longer.

The page measures every frame (the interval between frames and the CPU time inside them) and the
game (`crates/portal2/src/web/quality.rs`) steps through quality levels from those numbers:

| Level | Render scale | Portal views | Portals in portals | MSAA | SSR, volumetrics |
| ---: | ---: | ---: | :---: | :---: | :---: |
| 0 | 1.0 | 1.0 | yes | 4x | on |
| 1 | 1.0 | 0.75 | yes | 4x | on |
| 2 | 0.9 | 0.6 | yes | 4x | on |
| 3 | 0.8 | 0.5 | yes | 4x | on |
| 4 | 0.7 | 0.5 | no | 4x | on |
| 5 | 0.6 | 0.5 | no | off | off |
| 6 | 0.5 | 0.4 | no | off | off |

It steps down when frames fall below about 54 per second because of the GPU (two levels at once
when they are far slower, as when memory pages), and steps back up by probing: after a few steady
seconds it tries the next better level and keeps it unless frames get slower; a level that proved
too much is retried after 20 s, then 40, 80, and so on. When the CPU rather than the GPU is the
limit, resolution would not help, so it only leaves out the portals seen inside portals (two views
of the world fewer) and keeps the full resolution. The frames right after a map loads, when new
materials compile their pipelines, do not count. `r_quality` prints the current level;
`r_dynamic_quality 0` hands the settings back to the console (`r_render_scale`, `r_portal_scale`,
`r_portal_depth`, `r_msaa`), and the page's developer console can send any command with
`p2r.console('...')`.

Portal views render at `r_portal_scale` of the window's resolution into smaller textures (the
portal surfaces are told the size of the target they are drawn into and sample accordingly); the
desktop build keeps 1.0.

### Browser-specific fixes

Found by profiling:

- **WebGPU limits.** wgpu's WebGPU backend reads a device's limits back from the browser (some
  thirty calls into JavaScript) every time they are asked for, and Bevy asks on every draw: in busy
  views that was a sixth of the frame. `p2web build` compiles against a copy of the locked wgpu
  release that keeps the first answer (made in p2web's data folder and passed to Cargo with
  `--config patch...` for that build only; `Cargo.lock` is restored afterwards). The page also keeps
  `device.limits` and `device.features` as plain objects.
- **Video memory never freed.** The same backend never destroys textures or buffers when they are
  dropped; their memory comes back only when the JavaScript garbage collector happens to collect
  the objects, and it knows nothing of GPU memory. Every reallocated render target (a resize, a
  portal view, a change of render scale) stayed in video memory until the GPU paged. The patched
  copy gives each texture and buffer a guard shared by its views and the bind groups that use it
  (as native wgpu keeps them alive) and destroys the resource at the start of the frame after the
  last of them is gone, once every command using it has been submitted. Video memory now stays flat
  through any number of reallocations.
- **Heap growth.** Rust's allocator grows the page's memory 64 KiB at a time, and each
  `memory.grow` is costly in the browser: loading a map grew it ten thousand times and stalled for
  over six seconds. The browser build's allocator (`crates/portal2/src/web.rs`) grows it by a
  quarter of the heap at a time instead.
- **Material churn.** The portals passed the time to their shader in their material, so all six
  portal materials were prepared again every frame (a new uniform buffer and bind group each);
  the shader now takes the time from the view, and materials change only while a portal opens.
  Spotlight halos did the same through an unconditional `get_mut`.
- **Unseen movies.** In-world Bink screens decode on the game's own thread in the browser; a movie
  no view saw in the last frame now holds still, and its YUV to RGBA conversion does the chroma
  work once per pixel pair. Every camera also uses a single light cluster (the game has no Bevy
  lights) instead of building a grid of thousands each frame.

### Audio

The game mixes on the page's thread, so with Web Audio's buffer scheduling a slow frame made
buffers start late and overlap: the sound went robotic. The browser build now mixes a little ahead
of time into a ring buffer of shared memory that an audio worklet plays on the browser's audio
thread (`device::web` in `portal2_audio`, the worklet in `loader.js`). It keeps 60 ms queued,
grows that by 20 ms (up to 180 ms) each time the queue runs dry although frames kept coming
within it, and eases back after a few smooth seconds; a stall longer than any queue (a map
loading) leaves a short silence, faded at both ends, instead of noise. Shared memory needs the
page to be cross-origin isolated, which `p2web` serves; elsewhere the old Web Audio path remains.

With dynamic quality, the portal benchmark views below hold 60 frames a second at 2560 x 1440
(rendering 1920 x 1010) on an RTX 5060 Ti with 8 GB, which other programs already filled to
7.4 GB; before, those views ran at 3 to 9 frames a second at that size.

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
| `crates/portal2/src/web.rs` | Starting in the browser: the canvas window, the pointer lock and the heap allocator. |
| `crates/portal2/src/web/quality.rs` | Dynamic quality: pixel budget, render scale, portal views, MSAA. |
