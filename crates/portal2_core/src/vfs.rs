//! Access to the user's Portal 2 installation (in the browser, to the asset
//! stream from the machine serving the page; see [`crate::web`]).
//!
//! What each map reads is logged per phase (`boot` for the menus, then one
//! phase per map) when the game runs in the browser, which reports it to the
//! server, or when `P2R_ACCESS_LOG=<dir>` is set on the desktop, which
//! writes `<dir>/<phase>.txt`. The browser build's server sends a map's
//! listed files in one piece before the map loads; running the campaign
//! (`--smoke sp`) with the variable set prepares the lists for every map.

use std::sync::Arc;

use bevy::prelude::*;
use source_formats::fs::GameFileSystem;

#[derive(Resource, Clone, Deref)]
pub struct GameFs(pub Arc<GameFileSystem>);

pub struct VfsPlugin;

impl Plugin for VfsPlugin {
    fn build(&self, app: &mut App) {
        let fs = mount();
        app.insert_resource(GameFs(fs)).add_systems(Last, flush_access_log);
    }
}

#[cfg(target_arch = "wasm32")]
fn mount() -> Arc<GameFileSystem> {
    crate::web::game_fs().expect("the asset stream is mounted before the app starts")
}

#[cfg(not(target_arch = "wasm32"))]
fn mount() -> Arc<GameFileSystem> {
    let Some(root) = GameFileSystem::find_install() else {
        panic!(
            "Portal 2 installation not found. Install Portal 2 through Steam or set PORTAL2_DIR \
             to the folder that contains portal2.exe."
        );
    };
    let fs = GameFileSystem::portal2(&root).expect("failed to mount Portal 2 search paths");
    info!("mounted Portal 2 from {} ({:?})", root.display(), fs.search_path_names());
    if access_log_dir().is_some() {
        fs.record_access(true);
    }
    Arc::new(fs)
}

#[cfg(not(target_arch = "wasm32"))]
fn access_log_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("P2R_ACCESS_LOG").map(std::path::PathBuf::from)
}

/// Whether the files `map` needs are at hand: always on the desktop; in the
/// browser once the asset stream has fetched what the map read last time
/// (the first call starts that).
pub fn map_files_ready(map: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    return crate::web::map_files_ready(map);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = map;
        true
    }
}

/// A map starts loading: what is read from now on belongs to it.
pub fn begin_map_phase(fs: &GameFileSystem, map: &str) {
    if let Some((phase, files)) = fs.set_access_phase(map) {
        write_access(&phase, &files);
    }
}

fn write_access(phase: &str, files: &[String]) {
    if files.is_empty() {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    crate::web::report_access(phase, files);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(dir) = access_log_dir() {
        let name: String = phase.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
        let path = dir.join(format!("{name}.txt"));
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let mut lines: Vec<&str> = old.lines().filter(|l| !l.is_empty()).collect();
        let known: std::collections::HashSet<&str> = lines.iter().copied().collect();
        let added: Vec<&str> = files.iter().map(String::as_str).filter(|f| !known.contains(f)).collect();
        if added.is_empty() {
            return;
        }
        lines.extend(added);
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = std::fs::write(&path, lines.join("\n") + "\n") {
            warn!("access log {}: {e}", path.display());
        }
    }
}

/// Hands the files read lately to the log every few seconds, so a map's
/// list grows as it is played (sounds, scenes and models it only needs
/// later) and survives the page being closed.
fn flush_access_log(fs: Res<GameFs>, time: Res<Time<Real>>, mut last: Local<f32>) {
    let now = time.elapsed_secs();
    if now - *last < 5.0 {
        return;
    }
    *last = now;
    if let Some((phase, files)) = fs.take_access() {
        write_access(&phase, &files);
    }
}
