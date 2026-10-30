//! Collision pieces of a model's .phy: `phyinfo <model.mdl>` prints each
//! convex piece's vertex count and bounds in model space.
use glam::Vec3;
use source_formats::phy::Phy;

pub fn main() {
    let path = crate::args().nth(1).expect("usage: phyinfo <model.mdl>");
    let fs = crate::game_fs();
    let phy_path = path.trim_end_matches(".mdl").to_owned() + ".phy";
    let data = fs.read(&phy_path).expect("no .phy");
    let phy = Phy::parse(&data).expect("bad .phy");
    for (si, solid) in phy.solids.iter().enumerate() {
        for (pi, piece) in solid.pieces.iter().enumerate() {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &piece.points {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
            println!("solid {si} piece {pi}: {} verts, bounds {lo} .. {hi}", piece.points.len());
        }
    }
}
