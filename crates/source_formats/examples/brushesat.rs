//! Lists brush entities whose model bounds contain a point, with their
//! class, name and bounds. Usage: `brushesat <map> x,y,z [margin]`.
use glam::Vec3;
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let mut args = std::env::args().skip(1);
    let map = args.next().expect("usage: brushesat <map> x,y,z [margin]");
    let p: Vec<f32> = args.next().expect("point").split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let point = Vec3::new(p[0], p[1], p[2]);
    let margin: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for (i, e) in bsp.entities.iter().enumerate() {
        let Some(index) = e.get("model").and_then(|m| m.strip_prefix('*')).and_then(|n| n.parse::<usize>().ok()) else { continue };
        let Some(model) = bsp.models.get(index) else { continue };
        let origin = e.origin();
        let (lo, hi) = (model.mins + origin - Vec3::splat(margin), model.maxs + origin + Vec3::splat(margin));
        if point.cmpge(lo).all() && point.cmple(hi).all() {
            println!("#{i} {} '{}' {lo:.0} .. {hi:.0}", e.classname(), e.get("targetname").unwrap_or(""));
        }
    }
}
