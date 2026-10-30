//! Lists keyvalue names and defaults the installed entity definitions declare
//! for given classes (names and defaults only).
use source_formats::fgd::Fgd;

pub fn main() {
    let root = crate::install_root();
    let bin = root.join("bin");
    let read = |name: &str| std::fs::read(bin.join(name)).ok().map(|b| source_formats::text::decode_text(&b));
    let fgd = Fgd::parse(&read("portal2.fgd").unwrap(), &mut |n| read(n));
    for class in crate::args().skip(1) {
        match fgd.resolve(&class) {
            Some(c) => {
                let mut keys: Vec<_> =
                    c.defaults.iter().filter(|(k, _)| !matches!(k.as_str(), "targetname" | "parentname" | "angles" | "origin")).collect();
                keys.sort();
                let list: Vec<String> = keys.iter().map(|(k, v)| format!("{k}={v}")).collect();
                println!("{class}: model {} | {}", c.studio_model.is_some(), list.join(" "));
            }
            None => println!("{class}: missing"),
        }
    }
}
