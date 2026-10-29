//! Measures which axis mapping makes collision points line up with model
//! hull bounds, across many installed models. Prints counts only.
use source_formats::fs::GameFileSystem;
use source_formats::mdl::Mdl;
use source_formats::phy::Phy;

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let models = fs.list("models", Some("mdl"), true);
    // Candidate mappings applied to the currently converted point (x, y, z).
    let maps: [(&str, fn(glam::Vec3) -> glam::Vec3); 4] = [
        ("current (x,y,z)", |p| p),
        ("swap (x,-z,y)", |p| glam::Vec3::new(p.x, -p.z, p.y)),
        ("swap (x,z,-y)", |p| glam::Vec3::new(p.x, p.z, -p.y)),
        ("swap (-x,z,y)", |p| glam::Vec3::new(-p.x, p.z, p.y)),
    ];
    let mut wins = [0usize; 4];
    let mut tested = 0;
    let mut flat_tested = 0;
    let mut flat_wins = [0usize; 4];
    for m in models.iter().take(1500) {
        let base = m.trim_end_matches(".mdl");
        let (Some(pb), Some(mb)) = (fs.read(&format!("{base}.phy")), fs.read(m)) else { continue };
        let (Ok(phy), Ok(mdl)) = (Phy::parse(&pb), Mdl::parse(mb)) else { continue };
        let (lo, hi) = (mdl.hull_min - glam::Vec3::splat(2.0), mdl.hull_max + glam::Vec3::splat(2.0));
        let size = mdl.hull_max - mdl.hull_min;
        if size.min_element() <= 0.0 {
            continue;
        }
        let pts: Vec<glam::Vec3> = phy.solids.iter().flat_map(|s| s.pieces.iter().flat_map(|p| p.points.iter().copied())).collect();
        if pts.is_empty() {
            continue;
        }
        let scores: Vec<f32> = maps
            .iter()
            .map(|(_, f)| {
                pts.iter()
                    .filter(|p| {
                        let q = f(**p);
                        q.cmpge(lo).all() && q.cmple(hi).all()
                    })
                    .count() as f32
                    / pts.len() as f32
            })
            .collect();
        let best = (0..4).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap();
        if scores[best] < 0.95 {
            continue;
        }
        // Only count models where the mappings disagree (not cube-like).
        if scores.iter().filter(|s| **s >= 0.95).count() == 4 {
            continue;
        }
        tested += 1;
        wins[best] += 1;
        // Flat models discriminate axes most clearly.
        if size.z < 0.5 * size.x.min(size.y) || size.z > 2.0 * size.x.max(size.y) {
            flat_tested += 1;
            flat_wins[best] += 1;
        }
    }
    for (i, (name, _)) in maps.iter().enumerate() {
        println!("{name}: best for {} of {tested} discriminating models ({} of {flat_tested} flat/tall)", wins[i], flat_wins[i]);
    }
}
