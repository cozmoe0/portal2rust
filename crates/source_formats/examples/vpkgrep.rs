//! Prints lines of game files (under a directory, with an extension) that
//! contain a substring, case-insensitively: `vpkgrep <needle> <dir> <ext>`.
use source_formats::fs::GameFileSystem;

fn main() {
    let mut args = std::env::args().skip(1);
    let needle = args.next().expect("usage: vpkgrep <needle> <dir> <ext>").to_ascii_lowercase();
    let dir = args.next().unwrap_or_else(|| "scripts/vscripts".into());
    let ext = args.next().unwrap_or_else(|| "nut".into());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    for p in fs.list(&dir, Some(&ext), true) {
        let Some(text) = fs.read_string(&p) else { continue };
        for (i, line) in text.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&needle) {
                println!("{p}:{}: {}", i + 1, line.trim());
            }
        }
    }
}
