//! Distances from a point to solid world geometry along the six axes, for
//! placing cameras: `openspace <map> x,y,z`. Marks the point when it is in
//! solid.
use glam::Vec3;
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

/// CONTENTS_SOLID | CONTENTS_WINDOW | CONTENTS_GRATE.
const SOLID: u32 = 0x1 | 0x2 | 0x8;

fn main() {
    let mut args = std::env::args().skip(1);
    let map = args.next().expect("usage: openspace <map> x,y,z");
    let p: Vec<f32> = args.next().expect("point").split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let point = Vec3::new(p[0], p[1], p[2]);
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let inside = bsp.point_contents(point) & SOLID != 0;
    println!("{point} {}", if inside { "IN SOLID" } else { "open" });
    for (name, dir) in [("+x", Vec3::X), ("-x", -Vec3::X), ("+y", Vec3::Y), ("-y", -Vec3::Y), ("+z", Vec3::Z), ("-z", -Vec3::Z)] {
        let mut d = 0.0;
        while d < 4096.0 && bsp.point_contents(point + dir * d) & SOLID == 0 {
            d += 4.0;
        }
        println!("  {name}: {d}");
    }
}
