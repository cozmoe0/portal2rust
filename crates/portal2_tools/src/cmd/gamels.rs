//! Lists game files under a directory (recursively), optionally with one
//! extension: `gamels <dir> [ext]`.

pub fn main() {
    let mut args = crate::args().skip(1);
    let dir = args.next().unwrap_or_default();
    let ext = args.next();
    let fs = crate::game_fs();
    for p in fs.list(&dir, ext.as_deref(), true) {
        println!("{p}");
    }
}
