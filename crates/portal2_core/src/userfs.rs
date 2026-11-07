//! The player's own files: saved games, settings and the like.
//!
//! On the desktop these are ordinary files. In the browser they belong to
//! the browser visiting the page, not to the machine serving it: they are
//! kept in the browser's IndexedDB (see [`crate::web`]) under the same
//! relative paths, so the code that saves and loads them is the same.

use std::io;
use std::path::{Path, PathBuf};

/// Seconds since 1970 (UTC).
pub fn unix_now() -> u64 {
    #[cfg(target_arch = "wasm32")]
    return (js_sys::Date::now() / 1000.0) as u64;
    #[cfg(not(target_arch = "wasm32"))]
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(target_arch = "wasm32")]
fn key(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    s.trim_start_matches("./").trim_start_matches('/').to_owned()
}

#[cfg(target_arch = "wasm32")]
fn not_found(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{}: not found", path.display()))
}

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    #[cfg(target_arch = "wasm32")]
    return crate::web::user::read(&key(path)).ok_or_else(|| not_found(path));
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::read(path)
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Writes the whole file, replacing it at once (through a temporary file on
/// the desktop, so a crash never leaves half a file). Missing folders are
/// created.
pub fn write(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    #[cfg(target_arch = "wasm32")]
    {
        crate::web::user::write(&key(path), data.as_ref().to_vec());
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)
    }
}

pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    #[cfg(target_arch = "wasm32")]
    return if crate::web::user::rename(&key(from), &key(to)) { Ok(()) } else { Err(not_found(from)) };
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::rename(from, to)
}

pub fn remove(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    #[cfg(target_arch = "wasm32")]
    return if crate::web::user::remove(&key(path)) { Ok(()) } else { Err(not_found(path)) };
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::remove_file(path)
}

pub fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    #[cfg(target_arch = "wasm32")]
    return crate::web::user::modified(&key(path)).is_some();
    #[cfg(not(target_arch = "wasm32"))]
    path.is_file()
}

/// The files directly in `dir`, with when each was last written (seconds
/// since 1970).
pub fn list(dir: impl AsRef<Path>) -> Vec<(PathBuf, u64)> {
    let dir = dir.as_ref();
    #[cfg(target_arch = "wasm32")]
    return crate::web::user::list(&key(dir))
        .into_iter()
        .map(|k| {
            let modified = crate::web::user::modified(&k).unwrap_or(0.0);
            (PathBuf::from(k), (modified / 1000.0) as u64)
        })
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| {
                let modified = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                (e.path(), modified)
            })
            .collect()
    }
}
