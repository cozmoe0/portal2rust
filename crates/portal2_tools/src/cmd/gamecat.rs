//! Prints a file from the installed game's search paths (VPKs included) to
//! stdout. Usage: `gamecat <path>`, e.g. `gamecat scripts/vscripts/mapspawn.nut`.
use std::io::Write;

pub fn main() {
    let path = crate::args().nth(1).expect("usage: gamecat <path>");
    let fs = crate::game_fs();
    match fs.read(&path) {
        Some(data) => std::io::stdout().write_all(&data).unwrap(),
        None => {
            eprintln!("{path}: not found");
            std::process::exit(1);
        }
    }
}
