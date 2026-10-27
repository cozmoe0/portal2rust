//! Compiles every script under a directory (default: the installed game's
//! `portal2/scripts/vscripts`) and reports failures.
//!
//!     cargo run -p squirrel --example compile_all [dir]

use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("nut")) {
            out.push(p);
        }
    }
}

fn main() {
    let dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        let root = std::env::var("PORTAL2_DIR").unwrap_or_else(|_| r"C:\Program Files (x86)\Steam\steamapps\common\Portal 2".into());
        PathBuf::from(root).join("portal2/scripts/vscripts")
    });
    let mut files = Vec::new();
    collect(&dir, &mut files);
    files.sort();
    let mut failed = 0;
    for f in &files {
        let src = std::fs::read(f).expect("readable script");
        let mut vm = squirrel::Vm::new();
        let name = f.strip_prefix(&dir).unwrap_or(f).display().to_string();
        if let Err(e) = vm.compile(&src, &name) {
            failed += 1;
            println!("FAIL {e}");
        }
    }
    println!("{} scripts, {} failed", files.len(), failed);
}
