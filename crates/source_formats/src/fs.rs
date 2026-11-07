//! Layered game filesystem: loose directories, VPK archives and the current
//! map's embedded pakfile, searched in priority order like the original
//! engine's search paths.
//!
//! In the browser there is no install to read: the whole game is one
//! [`FileStore`] that streams files from the machine serving the page, and
//! the filesystem can keep a log of what each map reads so that the server
//! can send a map's files in one go the next time it is loaded.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use crate::text::normalize_path;
use crate::vpk::Vpk;
use crate::zip::ZipArchive;
use crate::{Error, Result};

/// The game's files kept somewhere other than this machine's disk: the
/// browser build's asset stream, which answers from files it already has
/// and fetches the rest from the machine serving the page.
pub trait FileStore: Send + Sync {
    /// A (normalised) file's bytes.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Whether the (normalised) file exists.
    fn exists(&self, path: &str) -> bool;
    /// Calls `f` with every file the store holds (normalised paths).
    fn for_each_path(&self, f: &mut dyn FnMut(&str));
    /// A file under the install root, such as `bin/portal2.fgd` or
    /// `portal2/resource/closecaption_english.dat`.
    fn read_root(&self, rel: &str) -> Option<Vec<u8>>;
    /// The game's loose folders (`update`, `portal2_dlc2`, ... `platform`),
    /// highest priority first.
    fn loose_dirs(&self) -> Vec<String>;
}

pub enum Source {
    Dir(LooseDir),
    Vpk(Arc<Vpk>),
    Pak(Arc<ZipArchive>),
    Store(Arc<dyn FileStore>),
}

impl Source {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Source::Dir(dir) => dir.has(path).then(|| std::fs::read(dir.path.join(path)).ok()).flatten(),
            Source::Vpk(v) => v.contains(path).then(|| v.read(path).ok()).flatten(),
            Source::Pak(z) => z.contains(path).then(|| z.read(path).ok()).flatten(),
            Source::Store(s) => s.read(path),
        }
    }

    fn exists(&self, path: &str) -> bool {
        match self {
            Source::Dir(dir) => dir.has(path) && dir.path.join(path).is_file(),
            Source::Vpk(v) => v.contains(path),
            Source::Pak(z) => z.contains(path),
            Source::Store(s) => s.exists(path),
        }
    }
}

/// One file of the merged search paths, as the browser build's index lists
/// it: the copy `read` would return, its size and a version that changes
/// when the file does (the archive's CRC, or the loose file's size and time).
#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub path: String,
    pub size: u64,
    pub version: u64,
}

/// The files read while one phase (the menus, or one map) was current.
#[derive(Default)]
struct AccessLog {
    phase: String,
    seen: HashSet<String>,
    files: Vec<String>,
}

/// Prefix of root-relative files (see [`GameFileSystem::read_root`]) in an
/// access log.
pub const ROOT_PREFIX: &str = "root:";

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
    /// Files read per phase, while recording (see [`Self::record_access`]).
    access: Mutex<Option<AccessLog>>,
}

const MAP_PAK: &str = "<map pakfile>";
const STORE: &str = "<asset stream>";

impl GameFileSystem {
    pub fn empty(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), paths: RwLock::new(Vec::new()), access: Mutex::new(None) }
    }

    /// The game served by a [`FileStore`] (the browser build).
    pub fn from_store(store: Arc<dyn FileStore>) -> Self {
        let fs = Self::empty(PathBuf::new());
        fs.paths.write().unwrap().push(SearchPath { name: STORE.to_owned(), source: Source::Store(store) });
        fs
    }

    fn store(&self) -> Option<Arc<dyn FileStore>> {
        self.paths.read().unwrap().iter().find_map(|p| match &p.source {
            Source::Store(s) => Some(s.clone()),
            _ => None,
        })
    }

    /// Starts (or stops) logging which files are read, per phase.
    pub fn record_access(&self, on: bool) {
        let mut access = self.access.lock().unwrap();
        match (on, access.is_some()) {
            (true, false) => *access = Some(AccessLog { phase: "boot".to_owned(), ..Default::default() }),
            (false, true) => *access = None,
            _ => {}
        }
    }

    /// Starts a new phase of the access log (`boot` for the menus, or a map
    /// name), returning the phase that ended and the files it read.
    pub fn set_access_phase(&self, phase: &str) -> Option<(String, Vec<String>)> {
        let mut access = self.access.lock().unwrap();
        let log = access.as_mut()?;
        let old = std::mem::replace(log, AccessLog { phase: phase.to_owned(), ..Default::default() });
        Some((old.phase, old.files))
    }

    /// The files read in the current phase since the last call (the phase
    /// goes on).
    pub fn take_access(&self) -> Option<(String, Vec<String>)> {
        let mut access = self.access.lock().unwrap();
        let log = access.as_mut()?;
        Some((log.phase.clone(), std::mem::take(&mut log.files)))
    }

    fn log_read(&self, key: &str) {
        if let Ok(mut access) = self.access.lock()
            && let Some(log) = access.as_mut()
            && log.seen.insert(key.to_owned())
        {
            log.files.push(key.to_owned());
        }
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
        let paths = self.paths.read().unwrap();
        let (source, data) = paths.iter().find_map(|p| p.source.read(&key).map(|d| (p, d)))?;
        // The map's own pakfile comes with the map; everything else is
        // what a map needs from the game.
        if !matches!(source.source, Source::Pak(_)) {
            self.log_read(&key);
        }
        Some(data)
    }

    /// A file under the install root rather than in the search paths, such
    /// as `bin/portal2.fgd`; `rel` uses `/` separators.
    pub fn read_root(&self, rel: &str) -> Option<Vec<u8>> {
        let rel = rel.replace('\\', "/");
        let data = match self.store() {
            Some(store) => store.read_root(&rel),
            None => std::fs::read(self.root.join(&rel)).ok(),
        }?;
        self.log_read(&format!("{ROOT_PREFIX}{rel}"));
        Some(data)
    }

    /// The loose folders searched (`update`, `portal2_dlc2`, ...,
    /// `platform`), highest priority first.
    pub fn loose_dirs(&self) -> Vec<String> {
        if let Some(store) = self.store() {
            return store.loose_dirs();
        }
        self.paths.read().unwrap().iter().filter(|p| matches!(p.source, Source::Dir(_))).map(|p| p.name.clone()).collect()
    }

    /// Every file of the merged search paths with the copy `read` returns
    /// (map pakfiles left out), for the browser build's index.
    pub fn index(&self) -> Vec<IndexEntry> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for sp in self.paths.read().unwrap().iter() {
            match &sp.source {
                Source::Vpk(v) => {
                    for path in v.paths() {
                        if let Some(e) = v.entry(path)
                            && seen.insert(path.to_owned())
                        {
                            out.push(IndexEntry { path: path.to_owned(), size: e.size() as u64, version: e.crc as u64 });
                        }
                    }
                }
                Source::Dir(dir) => {
                    for path in &dir.files {
                        if seen.contains(path) {
                            continue;
                        }
                        let Ok(meta) = std::fs::metadata(dir.path.join(path)) else { continue };
                        let time = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map_or(0, |d| d.as_nanos() as u64);
                        seen.insert(path.clone());
                        out.push(IndexEntry { path: path.clone(), size: meta.len(), version: time ^ meta.len().rotate_left(48) });
                    }
                }
                Source::Pak(_) | Source::Store(_) => {}
            }
        }
        out
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
                Source::Store(store) => store.for_each_path(&mut |p| {
                    if matches(p) {
                        out.insert(p.to_owned());
                    }
                }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A store holding a few files in memory, standing in for the browser's
    /// asset stream.
    struct MemoryStore(HashMap<String, Vec<u8>>);

    impl FileStore for MemoryStore {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).cloned()
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn for_each_path(&self, f: &mut dyn FnMut(&str)) {
            self.0.keys().filter(|k| !k.starts_with(ROOT_PREFIX)).for_each(|k| f(k));
        }
        fn read_root(&self, rel: &str) -> Option<Vec<u8>> {
            self.0.get(&format!("{ROOT_PREFIX}{rel}")).cloned()
        }
        fn loose_dirs(&self) -> Vec<String> {
            vec!["update".into(), "portal2".into()]
        }
    }

    fn store_fs() -> GameFileSystem {
        let files = [
            ("materials/a.vmt", b"a".to_vec()),
            ("materials/sub/b.vmt", b"b".to_vec()),
            ("scripts/c.txt", b"c".to_vec()),
            ("root:bin/portal2.fgd", b"fgd".to_vec()),
        ];
        let store = MemoryStore(files.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        GameFileSystem::from_store(Arc::new(store))
    }

    #[test]
    fn store_serves_reads_listings_and_root_files() {
        let fs = store_fs();
        assert_eq!(fs.read("Materials\\A.vmt").as_deref(), Some(&b"a"[..]));
        assert!(fs.exists("scripts/c.txt"));
        assert!(!fs.exists("scripts/missing.txt"));
        assert_eq!(fs.list("materials", Some("vmt"), false), vec!["materials/a.vmt".to_owned()]);
        assert_eq!(fs.list("materials", Some("vmt"), true).len(), 2);
        assert_eq!(fs.read_root("bin/portal2.fgd").as_deref(), Some(&b"fgd"[..]));
        assert_eq!(fs.loose_dirs(), vec!["update".to_owned(), "portal2".to_owned()]);
    }

    #[test]
    fn access_log_splits_reads_by_phase() {
        let fs = store_fs();
        fs.read("scripts/c.txt");
        assert!(fs.take_access().is_none(), "nothing is logged until recording starts");
        fs.record_access(true);
        fs.read("scripts/c.txt");
        fs.read("scripts/c.txt");
        fs.read_root("bin/portal2.fgd");
        fs.read("scripts/missing.txt");
        let (phase, files) = fs.set_access_phase("sp_a1_intro1").unwrap();
        assert_eq!(phase, "boot");
        assert_eq!(files, vec!["scripts/c.txt".to_owned(), "root:bin/portal2.fgd".to_owned()]);
        fs.read("materials/a.vmt");
        assert_eq!(fs.take_access().unwrap(), ("sp_a1_intro1".to_owned(), vec!["materials/a.vmt".to_owned()]));
        // A file read again in the same phase is not logged twice.
        fs.read("materials/a.vmt");
        assert_eq!(fs.take_access().unwrap().1, Vec::<String>::new());
    }
}
