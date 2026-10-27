//! Lists entity outputs whose target, input or parameter contains a
//! substring (case-insensitive), with the sending entity's name, class and
//! position. Usage: `iofind <map> <substring>`.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let mut args = std::env::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a1_intro2".to_owned());
    let needle = args.next().unwrap_or_else(|| "transition".to_owned()).to_ascii_lowercase();
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for (i, e) in bsp.entities.iter().enumerate() {
        let name = e.get("targetname").unwrap_or("");
        for (key, value) in &e.pairs {
            if !key.to_ascii_lowercase().starts_with("on") {
                continue;
            }
            let fields: Vec<&str> = if value.contains('\u{1b}') { value.split('\u{1b}').collect() } else { value.split(',').collect() };
            if fields.len() < 2 {
                continue;
            }
            let hit = fields.iter().take(3).any(|f| f.to_ascii_lowercase().contains(&needle))
                || name.to_ascii_lowercase().contains(&needle);
            if hit {
                println!(
                    "#{i} {} '{name}' at {:?}: {key} -> {} . {} ({}) delay {}",
                    e.classname(),
                    e.origin(),
                    fields[0],
                    fields[1],
                    fields.get(2).copied().unwrap_or(""),
                    fields.get(3).copied().unwrap_or("0"),
                );
            }
        }
    }
}
