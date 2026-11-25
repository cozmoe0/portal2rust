//! Skins a model's meshes on the CPU in one sequence pose and prints each
//! mesh's bounds, and the bounds of the vertices that mostly follow each
//! bone: `mdlskin <model path> <sequence> [cycle] [mesh index]`. For finding
//! parts an animation throws out of place (or folds to nothing).
use source_formats::studio::StudioModel;

pub fn main() {
    let mut args = crate::args().skip(1);
    let usage = "usage: mdlskin <model path> <sequence> [cycle] [mesh index]";
    let path = args.next().expect(usage);
    let seq_name = args.next().expect(usage);
    let cycle: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let only: Option<usize> = args.next().and_then(|s| s.parse().ok());
    let fs = crate::game_fs();
    let model = StudioModel::load(&fs, &path).expect("model loads");
    let seq = model.find_sequence(&seq_name).or_else(|| seq_name.parse().ok()).expect("sequence exists");
    let bind = model.bone_matrices(&model.bind_pose());
    let mut local = model.bind_pose();
    model.sample_sequence(seq, cycle, &mut local);
    let posed = model.bone_matrices(&local);
    let skin: Vec<glam::Mat4> = posed.iter().zip(&bind).map(|(p, b)| *p * b.inverse()).collect();
    for (i, mesh) in model.meshes.iter().enumerate() {
        if only.is_some_and(|o| o != i) || mesh.vertices.is_empty() {
            continue;
        }
        let mut groups: std::collections::BTreeMap<usize, (glam::Vec3, glam::Vec3, usize)> = Default::default();
        let mut lo = glam::Vec3::splat(f32::MAX);
        let mut hi = glam::Vec3::splat(f32::MIN);
        for v in &mesh.vertices {
            let n = v.num_bones.max(1) as usize;
            let mut p = glam::Vec3::ZERO;
            let mut total = 0.0;
            for k in 0..n {
                let m = skin.get(v.bones[k] as usize).copied().unwrap_or(glam::Mat4::IDENTITY);
                p += m.transform_point3(v.pos) * v.weights[k];
                total += v.weights[k];
            }
            if total > 0.0 {
                p /= total;
            }
            lo = lo.min(p);
            hi = hi.max(p);
            let main = (0..n).max_by(|&a, &b| v.weights[a].total_cmp(&v.weights[b])).unwrap_or(0);
            let g = groups.entry(v.bones[main] as usize).or_insert((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN), 0));
            g.0 = g.0.min(p);
            g.1 = g.1.max(p);
            g.2 += 1;
        }
        println!("mesh {i} (body part {} model {}): {} verts, bounds {lo:.1?}..{hi:.1?}", mesh.body_part, mesh.model, mesh.vertices.len());
        for (b, (l, h, c)) in groups {
            let name = model.mdl.bones.get(b).map_or("?", |x| x.name.as_str());
            println!("  bone {b:>3} {name:<30} {c:>6} verts, bounds {l:.1?}..{h:.1?} size {:.1?}", h - l);
        }
    }
}
