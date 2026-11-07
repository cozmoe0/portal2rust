//! The game's files as the browser build sees them: one index of every file
//! in the merged search paths, the files themselves (singly, or many at a
//! time in a "pack"), and the per-phase lists of what was read (`boot` for
//! the menus, then one per map) that say which files to send before a map
//! loads.
//!
//! Only game files are served: the files of the mounted search paths (minus
//! archives, binaries and the player's own game saves), plus the few files
//! the engine reads from beside them (`bin/*.fgd` and the caption files).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use source_formats::fs::{GameFileSystem, IndexEntry, ROOT_PREFIX};

/// Files never served even though they sit in a search path.
const HIDDEN_EXTENSIONS: &[&str] = &["vpk", "dll", "exe", "so", "dylib", "pdb", "sav", "dmp", "mdmp", "log", "bat", "sh", "lnk"];
const HIDDEN_DIRS: &[&str] = &["save", "screenshots", "downloads", "bin"];
/// What may be read from beside the search paths (see `read_root`).
const ROOT_EXTENSIONS: &[&str] = &["dat", "txt", "res", "fgd"];

pub struct GameAssets {
    pub fs: GameFileSystem,
    pub index: HashMap<String, IndexEntry>,
    /// The index as the browser downloads it: `path\tsize\tversion` lines,
    /// gzip-compressed.
    pub index_gz: Vec<u8>,
    /// Changes whenever any file does.
    pub version: String,
    pub loose: Vec<String>,
    manifest_dir: PathBuf,
    manifests: Mutex<HashMap<String, Manifest>>,
    /// The map played after each map (from browsers' level changes), so a
    /// browser can fetch the next map's files while the current one plays.
    next: Mutex<HashMap<String, String>>,
}

#[derive(Default)]
struct Manifest {
    list: Vec<String>,
    set: HashSet<String>,
}

fn extension(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or("", |(_, e)| e)
}

/// A phase name safe as a file name (`sp_a1_intro1`, `boot`).
pub fn phase_name(raw: &str) -> Option<String> {
    let name: String = raw.trim().to_ascii_lowercase();
    (!name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(name)
}

impl GameAssets {
    pub fn mount(game: Option<&Path>, data_dir: &Path) -> Result<Self, String> {
        let root = match game {
            Some(g) => g.to_path_buf(),
            None => GameFileSystem::find_install().ok_or(
                "Portal 2 installation not found. Install Portal 2 through Steam, or pass --game / set PORTAL2_DIR to the folder that \
                 contains portal2.exe.",
            )?,
        };
        let fs = GameFileSystem::portal2(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        let loose = fs.loose_dirs();
        let started = std::time::Instant::now();
        let mut index = HashMap::new();
        for e in fs.index() {
            let top = e.path.split('/').next().unwrap_or("");
            if HIDDEN_EXTENSIONS.contains(&extension(&e.path)) || (e.path.contains('/') && HIDDEN_DIRS.contains(&top)) {
                continue;
            }
            index.insert(e.path.clone(), e);
        }
        let mut sorted: Vec<&IndexEntry> = index.values().collect();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        let mut text = String::with_capacity(sorted.len() * 64);
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for e in &sorted {
            text.push_str(&format!("{}\t{}\t{:x}\n", e.path, e.size, e.version));
            for b in e.path.bytes().chain(e.version.to_le_bytes()).chain(e.size.to_le_bytes()) {
                hash = (hash ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        let index_gz = gz.finish().map_err(|e| e.to_string())?;
        println!(
            "p2web: Portal 2 at {} — {} files indexed in {:.1}s ({} KiB index)",
            root.display(),
            index.len(),
            started.elapsed().as_secs_f32(),
            index_gz.len() / 1024
        );
        let manifest_dir = data_dir.join("manifests");
        let assets = Self {
            fs,
            index,
            index_gz,
            version: format!("{hash:016x}"),
            loose,
            manifest_dir,
            manifests: Mutex::new(HashMap::new()),
            next: Mutex::new(HashMap::new()),
        };
        assets.load_manifests();
        Ok(assets)
    }

    fn next_path(&self) -> PathBuf {
        self.manifest_dir.join("next.tsv")
    }

    /// The map that followed `phase` last time, if any browser has played on.
    pub fn next_of(&self, phase: &str) -> Option<String> {
        self.next.lock().unwrap().get(phase).cloned()
    }

    /// A browser went from map `from` to map `to`.
    pub fn learn_next(&self, from: &str, to: &str) {
        let mut next = self.next.lock().unwrap();
        if next.get(from).is_some_and(|t| t == to) {
            return;
        }
        next.insert(from.to_owned(), to.to_owned());
        let mut lines: Vec<String> = next.iter().map(|(f, t)| format!("{f}\t{t}")).collect();
        lines.sort();
        let _ = std::fs::create_dir_all(&self.manifest_dir);
        let _ = std::fs::write(self.next_path(), lines.join("\n") + "\n");
    }

    fn load_manifests(&self) {
        if let Ok(text) = std::fs::read_to_string(self.next_path()) {
            let mut next = self.next.lock().unwrap();
            for line in text.lines() {
                if let Some((from, to)) = line.split_once('\t')
                    && let (Some(from), Some(to)) = (phase_name(from), phase_name(to))
                {
                    next.insert(from, to);
                }
            }
        }
        let Ok(dir) = std::fs::read_dir(&self.manifest_dir) else { return };
        let mut manifests = self.manifests.lock().unwrap();
        for entry in dir.flatten() {
            let path = entry.path();
            let Some(name) = path.file_stem().and_then(|s| s.to_str()).and_then(phase_name) else { continue };
            if path.extension().is_none_or(|e| e != "txt") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let m = manifests.entry(name).or_default();
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if m.set.insert(line.to_owned()) {
                    m.list.push(line.to_owned());
                }
            }
        }
        let files: usize = manifests.values().map(|m| m.list.len()).sum();
        println!("p2web: {} file lists ({} entries) in {}", manifests.len(), files, self.manifest_dir.display());
    }

    /// Whether a root-relative file may be served: `bin/*.fgd`, or caption
    /// and text files inside one of the game's loose folders.
    fn root_allowed(&self, rel: &str) -> bool {
        if rel.is_empty() || rel.contains("..") || rel.contains('\\') || rel.contains(':') || rel.starts_with('/') {
            return false;
        }
        let ext = extension(rel).to_ascii_lowercase();
        match rel.split('/').next() {
            Some("bin") => ext == "fgd" && rel.matches('/').count() == 1,
            Some(dir) => self.loose.iter().any(|l| l == dir) && ROOT_EXTENSIONS.contains(&ext.as_str()) && rel.contains("/resource/"),
            None => false,
        }
    }

    /// Size and version of a servable file (index entry or root file).
    pub fn stat(&self, key: &str) -> Option<(u64, u64)> {
        if let Some(rel) = key.strip_prefix(ROOT_PREFIX) {
            if !self.root_allowed(rel) {
                return None;
            }
            let meta = std::fs::metadata(self.fs.root.join(rel)).ok()?;
            let time = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as u64;
            return Some((meta.len(), time ^ meta.len().rotate_left(48)));
        }
        self.index.get(key).map(|e| (e.size, e.version))
    }

    /// A servable file's bytes.
    pub fn read(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(rel) = key.strip_prefix(ROOT_PREFIX) {
            return self.root_allowed(rel).then(|| std::fs::read(self.fs.root.join(rel)).ok()).flatten();
        }
        if !self.index.contains_key(key) {
            return None;
        }
        self.fs.read(key)
    }

    /// The files a phase read before, as `key\tsize\tversion` lines.
    pub fn manifest(&self, phase: &str) -> String {
        let manifests = self.manifests.lock().unwrap();
        let Some(m) = manifests.get(phase) else { return String::new() };
        let mut out = String::with_capacity(m.list.len() * 64);
        for key in &m.list {
            if let Some((size, version)) = self.stat(key) {
                out.push_str(&format!("{key}\t{size}\t{version:x}\n"));
            }
        }
        out
    }

    /// Adds files a browser reported for a phase; returns how many were new.
    pub fn learn(&self, phase: &str, body: &str) -> usize {
        let mut manifests = self.manifests.lock().unwrap();
        let m = manifests.entry(phase.to_owned()).or_default();
        let mut added = 0;
        for key in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !m.set.contains(key) && self.stat(key).is_some() {
                m.set.insert(key.to_owned());
                m.list.push(key.to_owned());
                added += 1;
            }
        }
        if added > 0 {
            let _ = std::fs::create_dir_all(&self.manifest_dir);
            let path = self.manifest_dir.join(format!("{phase}.txt"));
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, m.list.join("\n") + "\n").and_then(|()| std::fs::rename(&tmp, &path)).is_err() {
                eprintln!("p2web: could not write {}", path.display());
            }
        }
        added
    }
}
