//! Prints origins of entities whose classname contains a substring.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let mut args = std::env::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let needle = args.next().unwrap_or_else(|| "light".to_owned());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for e in bsp.entities.iter().filter(|e| e.classname().contains(&needle)) {
        println!("{} origin {:?} angles {:?}", e.classname(), e.origin(), e.angles());
    }
}
