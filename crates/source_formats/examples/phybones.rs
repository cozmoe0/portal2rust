//! Tests aligning collision solids with their bone's default pose.
//! Usage: phybones <map> <classname>. Prints numbers only.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::kv;
use source_formats::phy::Phy;
use source_formats::studio::StudioModel;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{}.bsp", a[0])).unwrap()).unwrap();
    let ent = bsp.entities.iter().find(|e| e.classname() == a[1]).unwrap();
    let model = ent.get("model").unwrap();
    let sm = StudioModel::load(&fs, model).unwrap();
    let phy = Phy::parse(&fs.read(&format!("{}.phy", model.trim_end_matches(".mdl"))).unwrap()).unwrap();
    let mut local = vec![(glam::Vec3::ZERO, glam::Quat::IDENTITY); sm.mdl.bones.len()];
    sm.sample_sequence(0, 0.0, &mut local);
    let pose = sm.bone_matrices(&local);
    println!("root default pose rotation {:.3?}", local[0].1);
    // Bone name per solid index from the keyvalue text.
    let doc = kv::parse_document(&phy.keyvalues);
    let mut solid_bone = std::collections::HashMap::new();
    for r in &doc.roots {
        if r.key.eq_ignore_ascii_case("solid") {
            let idx = r.get_i32("index").unwrap_or(-1);
            let bone = r.get_str("name").and_then(|n| sm.mdl.find_bone(n));
            solid_bone.insert(idx, bone);
        }
    }
    let (lo, hi) = (sm.mdl.hull_min - glam::Vec3::splat(2.0), sm.mdl.hull_max + glam::Vec3::splat(2.0));
    for (label, f) in [("raw", 0), ("bone pose * p", 1), ("bone pose * pose_to_bone * p", 2)] {
        let mut inside = 0;
        let mut total = 0;
        for (i, s) in phy.solids.iter().enumerate() {
            let b = solid_bone.get(&(i as i32)).copied().flatten().unwrap_or(0);
            let m = match f {
                0 => glam::Mat4::IDENTITY,
                1 => pose[b],
                _ => pose[b] * sm.mdl.bones[b].pose_to_bone,
            };
            for p in s.pieces.iter().flat_map(|p| p.points.iter()) {
                let q = m.transform_point3(*p);
                total += 1;
                if q.cmpge(lo).all() && q.cmple(hi).all() {
                    inside += 1;
                }
            }
        }
        println!("{label}: {inside}/{total} points inside model hull");
    }
}
