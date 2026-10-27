//! Prints the collision solids of a model's `.phy`: per convex piece its
//! point count and bounds, then the solid keyvalues.
//! Usage: `phypieces <models/....mdl>`.
use glam::Vec3;
use source_formats::fs::GameFileSystem;
use source_formats::phy::Phy;

fn main() {
    let model = std::env::args().nth(1).expect("usage: phypieces <model.mdl>");
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let base = model.trim_end_matches(".mdl");
    let phy = Phy::parse(&fs.read(&format!("{base}.phy")).expect("no .phy")).unwrap();
    for (i, s) in phy.solids.iter().enumerate() {
        println!("solid {i}: {} pieces", s.pieces.len());
        for (j, p) in s.pieces.iter().enumerate() {
            let lo = p.points.iter().fold(Vec3::splat(f32::MAX), |a, b| a.min(*b));
            let hi = p.points.iter().fold(Vec3::splat(f32::MIN), |a, b| a.max(*b));
            println!("  piece {j}: {} points, {} triangles, {lo:.1} .. {hi:.1}", p.points.len(), p.triangles.len());
        }
    }
    println!("{}", phy.keyvalues);
}
