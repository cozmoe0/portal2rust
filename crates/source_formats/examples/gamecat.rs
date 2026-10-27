//! Prints a file from the installed game's search paths (VPKs included) to
//! stdout. Usage: `gamecat <path>`, e.g. `gamecat scripts/vscripts/mapspawn.nut`.
use std::io::Write;

use source_formats::fs::GameFileSystem;

fn main() {
    let path = std::env::args().nth(1).expect("usage: gamecat <path>");
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    match fs.read(&path) {
        Some(data) => std::io::stdout().write_all(&data).unwrap(),
        None => {
            eprintln!("{path}: not found");
            std::process::exit(1);
        }
    }
}
