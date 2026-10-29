//! Validates collision hulls: every hull point should lie near the model's
//! own hull bounds. Prints counts only.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::mdl::Mdl;
use source_formats::phy::Phy;

fn main() {
    let map = std::env::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let (mut ok, mut bad, mut missing, mut pieces_total) = (0, 0, 0, 0);
    for (i, path) in bsp.static_props.models.iter().enumerate() {
        let base = path.trim_end_matches(".mdl");
        let Some(phy_bytes) = fs.read(&format!("{base}.phy")) else {
            missing += 1;
            continue;
        };
        let mdl = Mdl::parse(fs.read(path).unwrap()).unwrap();
        let phy = match Phy::parse(&phy_bytes) {
            Ok(p) => p,
            Err(e) => {
                println!("model {i}: parse error {e}");
                bad += 1;
                continue;
            }
        };
        let (lo, hi) =
            (mdl.hull_min.min(mdl.view_bbmin) - glam::Vec3::splat(8.0), mdl.hull_max.max(mdl.view_bbmax) + glam::Vec3::splat(8.0));
        let mut outside = 0;
        let mut pts = 0;
        for s in &phy.solids {
            for p in &s.pieces {
                pieces_total += 1;
                for q in &p.points {
                    pts += 1;
                    if q.cmplt(lo).any() || q.cmpgt(hi).any() {
                        outside += 1;
                    }
                }
            }
        }
        if pts == 0 || outside * 10 > pts {
            bad += 1;
            println!("model {i}: {pts} points, {outside} outside bounds, solids {}", phy.solids.len());
        } else {
            ok += 1;
        }
    }
    println!("{map}: {ok} models ok, {bad} suspicious, {missing} without .phy, {pieces_total} convex pieces");
}
