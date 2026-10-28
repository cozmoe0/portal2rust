//! Searches the English localization files for tokens whose key or text
//! contains a substring: `locfind <substring>`.
use source_formats::fs::GameFileSystem;

fn main() {
    let needle = std::env::args().nth(1).expect("usage: locfind <substring>").to_lowercase();
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    for file in ["resource/portal2_english.txt", "resource/basemodui_english.txt", "resource/gameui_english.txt"] {
        let Some(text) = fs.read_string(file) else { continue };
        let Some(root) = source_formats::kv::parse(&text) else { continue };
        let Some(tokens) = root.get("tokens") else { continue };
        for kv in tokens.children() {
            let v = kv.as_str().unwrap_or("");
            if kv.key.to_lowercase().contains(&needle) || v.to_lowercase().contains(&needle) {
                println!("{file}: {} = {v}", kv.key);
            }
        }
    }
}
