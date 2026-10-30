//! Prints the numeric keyvalues of one entity (by lump index): numbers and
//! vectors only, so names and text never appear.
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let index: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let Some(e) = bsp.entities.get(index) else { return };
    println!("#{index} {}", e.classname());
    for (k, v) in &e.pairs {
        let numeric = !v.is_empty() && v.split_whitespace().all(|p| p.parse::<f64>().is_ok());
        if numeric && !k.to_ascii_lowercase().starts_with("on") {
            println!("  {k} = {v}");
        }
    }
}
