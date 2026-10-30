//! Prints lines of game files (under a directory, with an extension) that
//! contain a substring, case-insensitively: `vpkgrep <needle> <dir> <ext>`.

pub fn main() {
    let mut args = crate::args().skip(1);
    let needle = args.next().expect("usage: vpkgrep <needle> <dir> <ext>").to_ascii_lowercase();
    let dir = args.next().unwrap_or_else(|| "scripts/vscripts".into());
    let ext = args.next().unwrap_or_else(|| "nut".into());
    let fs = crate::game_fs();
    for p in fs.list(&dir, Some(&ext), true) {
        let Some(text) = fs.read_string(&p) else { continue };
        for (i, line) in text.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&needle) {
                println!("{p}:{}: {}", i + 1, line.trim());
            }
        }
    }
}
