//! Lists game files under a directory (recursively), optionally with one
//! extension: `gamels <dir> [ext]`.
use source_formats::fs::GameFileSystem;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_default();
    let ext = args.next();
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    for p in fs.list(&dir, ext.as_deref(), true) {
        println!("{p}");
    }
}
