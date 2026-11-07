//! The browser build's link to the page: the asset stream that serves the
//! game's files, saved games kept by the browser, and the page's loader.
//!
//! The page's `loader.js` sets up a global `p2r` object before the game
//! starts. It fetches the server's index of the game's files and the files
//! the menus need, keeps every file it has fetched in memory (and in the
//! browser's IndexedDB, so a second visit loads from the local disk), and
//! answers the game's synchronous reads from memory. A file it does not
//! have yet is fetched synchronously from the server, which is slow, so
//! before a map loads the game asks the loader to fetch everything that map
//! read the last time anyone played it, in a few large parallel requests,
//! and waits for that while the loading screen shows. What a map reads is
//! reported back to the server, which keeps the lists.
//!
//! The player's files (saved games, settings) live in the browser too: the
//! loader reads them all from IndexedDB at startup, the game reads and
//! writes them in memory, and every write goes back to IndexedDB.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use js_sys::Uint8Array;
use source_formats::fs::{FileStore, GameFileSystem, ROOT_PREFIX};
use source_formats::text::normalize_path;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// Resolves once the index, the menus' files and the player's files are
    /// in memory.
    #[wasm_bindgen(js_namespace = p2r)]
    fn ready() -> js_sys::Promise;
    /// The server's index: `path\tsize\tversion` lines.
    #[wasm_bindgen(js_namespace = p2r, js_name = takeIndex)]
    fn take_index() -> Uint8Array;
    /// The game's loose folders, highest priority first.
    #[wasm_bindgen(js_namespace = p2r, js_name = looseDirs)]
    fn loose_dirs() -> Vec<String>;
    /// The page address's query as command-line words.
    #[wasm_bindgen(js_namespace = p2r)]
    fn args() -> Vec<String>;
    /// A file the loader has in memory.
    #[wasm_bindgen(js_namespace = p2r, js_name = cacheGet)]
    fn cache_get(key: &str) -> Option<Uint8Array>;
    /// Fetches a file from the server, blocking (`root:` prefix: a file
    /// under the install root).
    #[wasm_bindgen(js_namespace = p2r, js_name = fetchSync)]
    fn fetch_sync(key: &str) -> Option<Uint8Array>;
    /// Starts fetching what `phase` read last time (once) and returns the
    /// progress: 1 or more once everything is in memory.
    #[wasm_bindgen(js_namespace = p2r)]
    fn ensure(phase: &str) -> f64;
    /// Sends the server the files `phase` read (one per line).
    #[wasm_bindgen(js_namespace = p2r)]
    fn report(phase: &str, files: &str);
    /// The player's files: `[path, bytes, modified (ms since 1970)]` triples.
    #[wasm_bindgen(js_namespace = p2r, js_name = userFiles)]
    fn user_files() -> js_sys::Array;
    #[wasm_bindgen(js_namespace = p2r, js_name = userPut)]
    fn user_put(path: &str, data: &[u8], modified: f64);
    #[wasm_bindgen(js_namespace = p2r, js_name = userDelete)]
    fn user_delete(path: &str);
    /// Whether the game wants the mouse captured (the page locks the pointer
    /// on the next click; the browser allows nothing else).
    #[wasm_bindgen(js_namespace = p2r, js_name = wantPointerLock)]
    pub fn want_pointer_lock(on: bool);
    /// A line for the page's status overlay (empty hides it).
    #[wasm_bindgen(js_namespace = p2r)]
    pub fn status(text: &str);
}

/// The page address's query as command-line words.
pub fn url_args() -> Vec<String> {
    args()
}

/// The game's files as the page's loader serves them.
struct WebStore {
    /// Every file of the merged search paths, with its size.
    index: HashMap<String, u64>,
    loose: Vec<String>,
}

impl WebStore {
    fn fetch(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(data) = cache_get(key) {
            return Some(data.to_vec());
        }
        fetch_sync(key).map(|d| d.to_vec())
    }
}

impl FileStore for WebStore {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if !self.index.contains_key(path) {
            return None;
        }
        self.fetch(path)
    }

    fn exists(&self, path: &str) -> bool {
        self.index.contains_key(path)
    }

    fn for_each_path(&self, f: &mut dyn FnMut(&str)) {
        for path in self.index.keys() {
            f(path);
        }
    }

    fn read_root(&self, rel: &str) -> Option<Vec<u8>> {
        self.fetch(&format!("{ROOT_PREFIX}{rel}"))
    }

    fn loose_dirs(&self) -> Vec<String> {
        self.loose.clone()
    }
}

static GAME_FS: OnceLock<Arc<GameFileSystem>> = OnceLock::new();

/// Waits for the page's loader, then mounts the asset stream as the game's
/// filesystem and loads the player's files. Run before the app starts.
pub async fn boot() -> Arc<GameFileSystem> {
    if let Err(e) = wasm_bindgen_futures::JsFuture::from(ready()).await {
        panic!("the page could not load the game's files: {e:?}");
    }
    let raw = take_index().to_vec();
    let text = String::from_utf8_lossy(&raw);
    let mut index = HashMap::with_capacity(text.len() / 48);
    for line in text.lines() {
        let mut parts = line.split('\t');
        if let (Some(path), Some(size)) = (parts.next(), parts.next()) {
            index.insert(normalize_path(path), size.parse().unwrap_or(0));
        }
    }
    let files = index.len();
    let store = WebStore { index, loose: loose_dirs() };
    let fs = Arc::new(GameFileSystem::from_store(Arc::new(store)));
    // The menus' files are logged until the first map loads, then each
    // map's own.
    fs.record_access(true);
    bevy::log::info!("asset stream: {files} files");
    load_user_files();
    let _ = GAME_FS.set(fs.clone());
    fs
}

/// The filesystem `boot` mounted.
pub fn game_fs() -> Option<Arc<GameFileSystem>> {
    GAME_FS.get().cloned()
}

/// Whether everything `map` read last time is in memory; starts fetching it
/// the first time it is asked.
pub fn map_files_ready(map: &str) -> bool {
    let progress = ensure(map);
    if progress < 1.0 {
        status(&format!("Loading {map} \u{2014} {:.0}%", progress.max(0.0) * 100.0));
        return false;
    }
    status("");
    true
}

/// Sends the server what a phase read, so it can send it in one piece next
/// time.
pub fn report_access(phase: &str, files: &[String]) {
    if !files.is_empty() {
        report(phase, &files.join("\n"));
    }
}

/// One of the player's files.
pub struct UserFile {
    pub data: Vec<u8>,
    /// Milliseconds since 1970.
    pub modified: f64,
}

thread_local! {
    static USER_FILES: RefCell<BTreeMap<String, UserFile>> = const { RefCell::new(BTreeMap::new()) };
}

fn load_user_files() {
    let list = user_files();
    USER_FILES.with_borrow_mut(|files| {
        for entry in list.iter() {
            let entry: js_sys::Array = entry.unchecked_into();
            let (Some(path), data, modified) =
                (entry.get(0).as_string(), Uint8Array::new(&entry.get(1)).to_vec(), entry.get(2).as_f64().unwrap_or(0.0))
            else {
                continue;
            };
            files.insert(path, UserFile { data, modified });
        }
        bevy::log::info!("{} saved files in this browser", files.len());
    });
}

/// The player's files kept by this browser, for [`crate::userfs`].
pub mod user {
    use super::*;

    pub fn read(path: &str) -> Option<Vec<u8>> {
        USER_FILES.with_borrow(|f| f.get(path).map(|u| u.data.clone()))
    }

    pub fn modified(path: &str) -> Option<f64> {
        USER_FILES.with_borrow(|f| f.get(path).map(|u| u.modified))
    }

    pub fn write(path: &str, data: Vec<u8>) {
        let modified = js_sys::Date::now();
        user_put(path, &data, modified);
        USER_FILES.with_borrow_mut(|f| f.insert(path.to_owned(), UserFile { data, modified }));
    }

    pub fn remove(path: &str) -> bool {
        let gone = USER_FILES.with_borrow_mut(|f| f.remove(path));
        if gone.is_some() {
            user_delete(path);
        }
        gone.is_some()
    }

    pub fn rename(from: &str, to: &str) -> bool {
        let Some(file) = USER_FILES.with_borrow_mut(|f| f.remove(from)) else { return false };
        user_delete(from);
        user_put(to, &file.data, file.modified);
        USER_FILES.with_borrow_mut(|f| f.insert(to.to_owned(), file));
        true
    }

    /// Paths of the files directly in `dir` (`""` for the top level).
    pub fn list(dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{}/", dir.trim_end_matches('/')) };
        USER_FILES.with_borrow(|f| f.keys().filter(|k| k.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('/'))).cloned().collect())
    }
}
