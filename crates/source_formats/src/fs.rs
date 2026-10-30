//! Layered game filesystem: loose directories, VPK archives and the current
//! map's embedded pakfile, searched in priority order like the original
//! engine's search paths.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::text::normalize_path;
use crate::vpk::Vpk;
use crate::zip::ZipArchive;
use crate::{Error, Result};

pub enum Source {
    Dir(LooseDir),
    Vpk(Arc<Vpk>),
    Pak(Arc<ZipArchive>),
}

impl Source {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Source::Dir(dir) => dir.has(path).then(|| std::fs::read(dir.path.join(path)).ok()).flatten(),
            Source::Vpk(v) => v.contains(path).then(|| v.read(path).ok()).flatten(),
            Source::Pak(z) => z.contains(path).then(|| z.read(path).ok()).flatten(),
        }
    }

    fn exists(&self, path: &str) -> bool {
        match self {
            Source::Dir(dir) => dir.has(path) && dir.path.join(path).is_file(),
            Source::Vpk(v) => v.contains(path),
            Source::Pak(z) => z.contains(path),
        }
    }
}

/// A loose folder of the game, with the files in it listed once when it is
/// mounted: most lookups are for files that live in the VPKs searched after
/// it, and those then cost no filesystem access. Files added to the folder
/// later are not seen until it is mounted again.
pub struct LooseDir {
    pub path: PathBuf,
    /// Normalised paths of the files under `path`.
    files: HashSet<String>,
}

impl LooseDir {
    pub fn new(path: PathBuf) -> Self {
        let mut files = HashSet::new();
        let mut stack = vec![path.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let full = entry.path();
                // Follow links the way opening the file would.
                let is_dir = match entry.file_type() {
                    Ok(t) if t.is_symlink() => full.is_dir(),
                    Ok(t) => t.is_dir(),
                    Err(_) => continue,
                };
                if is_dir {
                    stack.push(full);
                } else if let Ok(rel) = full.strip_prefix(&path) {
                    files.insert(normalize_path(&rel.to_string_lossy()));
                }
            }
        }
        Self { path, files }
    }

    /// Whether the folder held the (normalised) file when it was mounted.
    pub fn has(&self, path: &str) -> bool {
        self.files.contains(path)
    }
}

pub struct SearchPath {
    pub name: String,
    pub source: Source,
}

pub struct GameFileSystem {
    /// Install root (directory containing `portal2.exe`).
    pub root: PathBuf,
    paths: RwLock<Vec<SearchPath>>,
}

const MAP_PAK: &str = "<map pakfile>";

impl GameFileSystem {
    pub fn empty(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), paths: RwLock::new(Vec::new()) }
    }

    /// Mounts the standard Portal 2 search paths found under `root`:
    /// `update`, `portal2_dlcN` (highest N first), `portal2`, `platform`.
    pub fn portal2(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        if !root.join("portal2").join("gameinfo.txt").is_file() {
            return Err(Error::NotFound(format!("{}/portal2/gameinfo.txt", root.display())));
        }
        let fs = Self::empty(root.clone());
        let mut games = vec!["update".to_owned()];
        let mut dlcs: Vec<(u32, String)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(n) = name.strip_prefix("portal2_dlc").and_then(|s| s.parse::<u32>().ok())
                    && !entry.path().join("dlc_disabled.txt").exists()
                {
                    dlcs.push((n, name));
                }
            }
        }
        dlcs.sort_by_key(|d| std::cmp::Reverse(d.0));
        games.extend(dlcs.into_iter().map(|(_, n)| n));
        games.push("portal2".to_owned());
        games.push("platform".to_owned());
        for game in games {
            let dir = root.join(&game);
            if !dir.is_dir() {
                continue;
            }
            fs.add_dir(&game, dir.clone());
            let vpk = dir.join("pak01_dir.vpk");
            if vpk.is_file() {
                match Vpk::open(&vpk) {
                    Ok(v) => fs.add_vpk(&format!("{game}/pak01_dir.vpk"), Arc::new(v)),
                    Err(e) => log::warn!("failed to open {}: {e}", vpk.display()),
                }
            }
        }
        Ok(fs)
    }

    /// Default Steam location on Windows, or `PORTAL2_DIR` if set.
    pub fn find_install() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("PORTAL2_DIR") {
            let p = PathBuf::from(dir);
            if p.join("portal2").is_dir() {
                return Some(p);
            }
        }
        let mut candidates = vec![PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Portal 2")];
        // Extra Steam libraries listed in libraryfolders.vdf.
        let vdf = PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(vdf)
            && let Some(root) = crate::kv::parse(&text)
        {
            for lib in root.children() {
                if let Some(path) = lib.get_str("path") {
                    candidates.push(PathBuf::from(path.replace("\\\\", "\\")).join("steamapps/common/Portal 2"));
                }
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(".local/share/Steam/steamapps/common/Portal 2"));
        }
        candidates.into_iter().find(|p| p.join("portal2").join("gameinfo.txt").is_file())
    }

    pub fn add_dir(&self, name: &str, dir: PathBuf) {
        self.paths.write().unwrap().push(SearchPath { name: name.to_owned(), source: Source::Dir(LooseDir::new(dir)) });
    }

    pub fn add_vpk(&self, name: &str, vpk: Arc<Vpk>) {
        self.paths.write().unwrap().push(SearchPath { name: name.to_owned(), source: Source::Vpk(vpk) });
    }

    /// Mounts a map's pakfile at the highest priority, replacing any previous one.
    pub fn mount_map_pakfile(&self, zip: ZipArchive) {
        let mut paths = self.paths.write().unwrap();
        paths.retain(|p| p.name != MAP_PAK);
        paths.insert(0, SearchPath { name: MAP_PAK.to_owned(), source: Source::Pak(Arc::new(zip)) });
    }

    pub fn unmount_map_pakfile(&self) {
        self.paths.write().unwrap().retain(|p| p.name != MAP_PAK);
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let key = normalize_path(path);
        self.paths.read().unwrap().iter().find_map(|p| p.source.read(&key))
    }

    pub fn read_string(&self, path: &str) -> Option<String> {
        self.read(path).map(|b| crate::text::decode_text(&b))
    }

    pub fn exists(&self, path: &str) -> bool {
        let key = normalize_path(path);
        self.paths.read().unwrap().iter().any(|p| p.source.exists(&key))
    }

    /// Which search path would serve a file (for diagnostics).
    pub fn locate(&self, path: &str) -> Option<String> {
        let key = normalize_path(path);
        self.paths.read().unwrap().iter().find(|p| p.source.exists(&key)).map(|p| p.name.clone())
    }

    /// Lists files directly or recursively under `dir` with an optional extension,
    /// merged across all search paths. Returned paths are normalised.
    pub fn list(&self, dir: &str, ext: Option<&str>, recursive: bool) -> Vec<String> {
        let dir = normalize_path(dir);
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let mut out = std::collections::BTreeSet::new();
        let matches = |p: &str| -> bool {
            let Some(rest) = p.strip_prefix(&prefix) else { return false };
            if !recursive && rest.contains('/') {
                return false;
            }
            ext.is_none_or(|e| crate::text::extension(rest).eq_ignore_ascii_case(e))
        };
        for sp in self.paths.read().unwrap().iter() {
            match &sp.source {
                Source::Vpk(v) => out.extend(v.paths().filter(|p| matches(p)).map(str::to_owned)),
                Source::Pak(z) => out.extend(z.paths().filter(|p| matches(p)).map(str::to_owned)),
                Source::Dir(loose) => {
                    let base = &loose.path;
                    let start = base.join(&dir);
                    let mut stack = vec![start];
                    while let Some(d) = stack.pop() {
                        let Ok(rd) = std::fs::read_dir(&d) else { continue };
                        for e in rd.flatten() {
                            let path = e.path();
                            if path.is_dir() {
                                if recursive {
                                    stack.push(path);
                                }
                                continue;
                            }
                            if let Ok(rel) = path.strip_prefix(base) {
                                let rel = normalize_path(&rel.to_string_lossy());
                                if matches(&rel) {
                                    out.insert(rel);
                                }
                            }
                        }
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    pub fn search_path_names(&self) -> Vec<String> {
        self.paths.read().unwrap().iter().map(|p| p.name.clone()).collect()
    }
}
