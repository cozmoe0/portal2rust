//! Layered game filesystem: loose directories, VPK archives and the current
//! map's embedded pakfile, searched in priority order like the original
//! engine's search paths.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::text::normalize_path;
use crate::vpk::Vpk;
use crate::zip::ZipArchive;
use crate::{Error, Result};

pub enum Source {
    Dir(PathBuf),
    Vpk(Arc<Vpk>),
    Pak(Arc<ZipArchive>),
}

impl Source {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Source::Dir(dir) => std::fs::read(dir.join(path)).ok(),
            Source::Vpk(v) => v.contains(path).then(|| v.read(path).ok()).flatten(),
            Source::Pak(z) => z.contains(path).then(|| z.read(path).ok()).flatten(),
        }
    }

    fn exists(&self, path: &str) -> bool {
        match self {
            Source::Dir(dir) => dir.join(path).is_file(),
            Source::Vpk(v) => v.contains(path),
            Source::Pak(z) => z.contains(path),
        }
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
        dlcs.sort_by(|a, b| b.0.cmp(&a.0));
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
        self.paths.write().unwrap().push(SearchPath { name: name.to_owned(), source: Source::Dir(dir) });
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
                Source::Dir(base) => {
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
