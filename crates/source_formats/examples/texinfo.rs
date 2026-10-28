//! Prints a texinfo's material and the material's text: `texinfo <map> <index>`.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let mut args = std::env::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a1_intro2".to_owned());
    let index: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let ti = &bsp.texinfo[index];
    let td = &bsp.texdata[ti.texdata as usize];
    let name = bsp.texdata_strings.get(td.name_index as usize).cloned().unwrap_or_default();
    println!("texinfo {index}: {name} flags {:#x} ({}x{})", ti.flags, td.width, td.height);
    let path = source_formats::vmt::material_path(&name);
    if let Some(text) = fs.read_string(&path) {
        println!("{text}");
    }
}
