//! Prints every keyvalue of entities whose targetname or classname contains
//! a substring (case-insensitive). Usage: `entdump <map> <substring>`.
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a1_intro2".to_owned());
    let needle = args.next().unwrap_or_default().to_ascii_lowercase();
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for (i, e) in bsp.entities.iter().enumerate() {
        let name = e.get("targetname").unwrap_or("").to_ascii_lowercase();
        if !name.contains(&needle) && !e.classname().to_ascii_lowercase().contains(&needle) {
            continue;
        }
        println!("#{i}");
        for (k, v) in &e.pairs {
            println!("  {k} = {}", v.replace('\u{1b}', ","));
        }
    }
}
