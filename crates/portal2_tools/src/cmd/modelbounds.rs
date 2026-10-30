//! Prints a brush model's bounds: `modelbounds <map> <model index>`.
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().unwrap();
    let index: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let m = &bsp.models[index];
    println!("model {index}: mins {:?} maxs {:?} origin {:?}", m.mins, m.maxs, m.origin);
}
