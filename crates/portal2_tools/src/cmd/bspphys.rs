//! Lists the physics collision (`LUMP_PHYSCOLLIDE`) of one brush model: each
//! solid's convex pieces with their bounds, in model space:
//! `bspphys <map> <model index>`.
use source_formats::bsp::Bsp;
use source_formats::phy::Phy;
use source_formats::reader::Reader;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().expect("usage: bspphys <map> <model index>");
    let want: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let data = &bsp.phys_collide;
    let mut r = Reader::new(data);
    while let (Ok(model), Ok(size), Ok(key_size), Ok(solids)) = (r.i32(), r.i32(), r.i32(), r.i32()) {
        if model < 0 {
            break;
        }
        let start = r.pos();
        if model == want {
            let mut at = start;
            for s in 0..solids {
                let len = i32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
                let block = &data[at + 4..at + 4 + len];
                if let Some(solid) = Phy::parse_surface(block) {
                    for (i, p) in solid.pieces.iter().enumerate() {
                        let mins = p.points.iter().fold(glam::Vec3::splat(f32::MAX), |a, b| a.min(*b));
                        let maxs = p.points.iter().fold(glam::Vec3::splat(f32::MIN), |a, b| a.max(*b));
                        println!("solid {s} piece {i}: {} points {mins:?} .. {maxs:?}", p.points.len());
                    }
                }
                at += 4 + len;
            }
            println!(
                "keydata: {}",
                String::from_utf8_lossy(&data[start + size as usize..start + size as usize + key_size as usize]).trim()
            );
        }
        r.seek(start + size as usize + key_size as usize);
    }
}
