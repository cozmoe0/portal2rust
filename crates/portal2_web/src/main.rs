//! `p2web`: portal2rust in the browser.
//!
//! ```text
//! p2web build [--dev] [--opt]       build the game for the browser (WebAssembly)
//! p2web serve [--port 8080] ...     serve it, with this machine's Portal 2 files
//! p2web run   [build and serve options]
//! p2web prepare [--maps sp]         record what every map reads, so the first
//!                                   visit to each map loads in one piece
//! ```
//!
//! The game runs entirely in the visiting browser (WebGPU, Web Audio, the
//! player's saves in the browser's IndexedDB); this machine only serves the
//! page, the WebAssembly build and the game's files from its Portal 2 install.
//! See `docs/web.md`.

#[cfg(not(target_arch = "wasm32"))]
mod assets;
#[cfg(not(target_arch = "wasm32"))]
mod bundle;
#[cfg(not(target_arch = "wasm32"))]
mod server;
#[cfg(not(target_arch = "wasm32"))]
mod tls;

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((c, rest)) if !c.starts_with('-') => (c.as_str(), rest.to_vec()),
        _ => ("run", args.clone()),
    };
    let opts = Options::parse(&rest);
    let result = match command {
        "build" => bundle::build(&opts),
        "serve" => server::serve(&opts),
        "run" => bundle::build(&opts).and_then(|()| server::serve(&opts)),
        "prepare" => prepare(&opts),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!("unknown command `{other}` (see `p2web help`)")),
    };
    if let Err(e) = result {
        eprintln!("p2web: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn print_help() {
    println!(
        "p2web: portal2rust in the browser

USAGE
  p2web build   [--dev] [--opt] [--pkg <dir>]
  p2web serve   [--port <n>] [--bind <addr>] [--http] [--cert <pem> --key <pem>]
                [--pkg <dir>] [--data <dir>] [--game <dir>]
  p2web run     (build, then serve; takes the options of both)
  p2web prepare [--maps sp|<map,map,...>] [--exe <portal2.exe>] [--data <dir>]

build     Builds the game for the browser into the package folder
          (default target/web/pkg). --dev builds faster without full
          optimisation; --opt also runs binaryen's wasm-opt if installed.
serve     Serves the page on every network interface (port 8080 by default)
          over HTTPS with a self-signed certificate made for this machine's
          addresses (browsers only allow WebGPU on secure pages; accept the
          certificate warning once). --http serves plain HTTP, which only
          works from this machine (http://localhost:<port>).
prepare   Runs the desktop build over the given maps (default: the whole
          single-player campaign) and records which files each one reads,
          so that a browser visiting a map for the first time fetches all of
          it at once instead of file by file. The server also learns this
          from the browsers that play.

The game's files come from this machine's Portal 2 install (found like the
desktop game does, or --game / PORTAL2_DIR). Saved games stay in each
visiting browser."
    );
}

/// Command-line options shared by the commands.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub struct Options {
    pub dev: bool,
    pub opt: bool,
    pub port: u16,
    pub bind: String,
    pub http: bool,
    pub cert: Option<std::path::PathBuf>,
    pub key: Option<std::path::PathBuf>,
    pub pkg: Option<std::path::PathBuf>,
    pub data: Option<std::path::PathBuf>,
    pub game: Option<std::path::PathBuf>,
    pub maps: String,
    pub exe: Option<std::path::PathBuf>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Options {
    fn parse(args: &[String]) -> Self {
        let mut o = Options {
            dev: false,
            opt: false,
            port: 8080,
            bind: "0.0.0.0".into(),
            http: false,
            cert: None,
            key: None,
            pkg: None,
            data: None,
            game: None,
            maps: "sp".into(),
            exe: None,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value = || it.next().cloned().unwrap_or_default();
            match a.as_str() {
                "--dev" => o.dev = true,
                "--opt" => o.opt = true,
                "--http" => o.http = true,
                "--port" => o.port = value().parse().unwrap_or(o.port),
                "--bind" => o.bind = value(),
                "--cert" => o.cert = Some(value().into()),
                "--key" => o.key = Some(value().into()),
                "--pkg" => o.pkg = Some(value().into()),
                "--data" => o.data = Some(value().into()),
                "--game" => o.game = Some(value().into()),
                "--maps" => o.maps = value(),
                "--exe" => o.exe = Some(value().into()),
                other => eprintln!("p2web: ignoring unknown option {other}"),
            }
        }
        o
    }

    /// Where `p2web` keeps its own files: the per-map file lists and the
    /// TLS certificate.
    pub fn data_dir(&self) -> std::path::PathBuf {
        if let Some(d) = &self.data {
            return d.clone();
        }
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_DATA_HOME"))
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("portal2rust")
            .join("web")
    }

    /// The browser build's folder (`target/web/pkg` by default).
    pub fn pkg_dir(&self) -> std::path::PathBuf {
        if let Some(p) = &self.pkg {
            return p.clone();
        }
        workspace_target().join("web").join("pkg")
    }
}

/// The workspace's `target` folder: `CARGO_TARGET_DIR`, the folder this
/// executable was built into, or `./target`.
#[cfg(not(target_arch = "wasm32"))]
pub fn workspace_target() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return dir.into();
    }
    // target/<profile>/p2web(.exe)
    if let Ok(exe) = std::env::current_exe()
        && let Some(target) = exe.parent().and_then(|p| p.parent())
        && target.join("CACHEDIR.TAG").is_file()
    {
        return target.to_path_buf();
    }
    std::path::PathBuf::from("target")
}

/// Runs the desktop build's campaign smoke test with the access log on, so
/// every map's file list is written to the data folder.
#[cfg(not(target_arch = "wasm32"))]
fn prepare(opts: &Options) -> Result<(), String> {
    let exe_name = if cfg!(windows) { "portal2.exe" } else { "portal2" };
    let exe = match &opts.exe {
        Some(e) => e.clone(),
        None => {
            let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(exe_name)));
            let release = workspace_target().join("release").join(exe_name);
            beside
                .filter(|p| p.is_file())
                .or(Some(release).filter(|p| p.is_file()))
                .ok_or_else(|| format!("{exe_name} not found; build it with `cargo build --release -p portal2` or pass --exe"))?
        }
    };
    let manifests = opts.data_dir().join("manifests");
    let work = opts.data_dir().join("prepare");
    std::fs::create_dir_all(&manifests).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    println!("p2web: recording the files of {} with {}", opts.maps, exe.display());
    println!("p2web: lists go to {}", manifests.display());
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["--smoke", &opts.maps, "--nosound"]).env("P2R_ACCESS_LOG", &manifests).current_dir(&work);
    if let Some(game) = &opts.game {
        cmd.env("PORTAL2_DIR", game);
    }
    let status = cmd.status().map_err(|e| format!("{}: {e}", exe.display()))?;
    if !status.success() {
        return Err(format!("{} exited with {status}", exe.display()));
    }
    let count = std::fs::read_dir(&manifests).map(|d| d.count()).unwrap_or(0);
    println!("p2web: {count} file lists ready");
    Ok(())
}
