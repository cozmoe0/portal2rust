//! Where a model's meshes of one material sit in the bind pose:
//! `mdlmesh <model path> <material substring> [skin]`. Prints each matching
//! mesh's centroid and area-weighted normal in model space, the bone most of
//! its vertices follow, and the same centroid and normal in that bone's
//! frame (for placing an effect on a moving part, such as an eye).
use source_formats::studio::StudioModel;

pub fn main() {
    let mut args = crate::args().skip(1);
    let usage = "usage: mdlmesh <model path> <material substring> [skin]";
    let path = args.next().expect(usage);
    let needle = args.next().expect(usage).to_ascii_lowercase();
    let skin: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fs = crate::game_fs();
    let model = StudioModel::load(&fs, &path).expect("model loads");
    let bind = model.bone_matrices(&model.bind_pose());
    let family = model.mdl.skins.get(skin).or_else(|| model.mdl.skins.first());
    for (i, mesh) in model.meshes.iter().enumerate() {
        let texture = family.and_then(|f| f.get(mesh.material)).map_or(mesh.material, |&t| t.max(0) as usize);
        let material = model.material_paths.get(texture).and_then(|p| p.first()).cloned().unwrap_or_default();
        if !material.to_ascii_lowercase().contains(&needle) {
            continue;
        }
        let mut centroid = glam::Vec3::ZERO;
        let mut normal = glam::Vec3::ZERO;
        let mut area = 0.0;
        for t in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|v| mesh.vertices[v as usize].pos);
            let n = (b - a).cross(c - a);
            let w = n.length() * 0.5;
            centroid += (a + b + c) / 3.0 * w;
            normal += n;
            area += w;
        }
        centroid /= area.max(1e-6);
        let normal = normal.normalize_or_zero();
        let mut counts = vec![0usize; model.mdl.bones.len()];
        for v in &mesh.vertices {
            let k = (0..v.num_bones.max(1) as usize).max_by(|&x, &y| v.weights[x].total_cmp(&v.weights[y])).unwrap_or(0);
            if let Some(c) = counts.get_mut(v.bones[k] as usize) {
                *c += 1;
            }
        }
        // Parts of the mesh that follow other bones (a lens sharing its
        // material with body lights).
        for (b, &c) in counts.iter().enumerate().filter(|(_, c)| **c >= 8) {
            let mut sum = glam::Vec3::ZERO;
            let mut n = glam::Vec3::ZERO;
            let mut k = 0;
            for v in mesh.vertices.iter().filter(|v| v.bones[0] as usize == b) {
                sum += v.pos;
                n += v.normal;
                k += 1;
            }
            if k > 0 {
                let to = bind.get(b).map_or(glam::Mat4::IDENTITY, |m| m.inverse());
                let (c_local, n_local) = (to.transform_point3(sum / k as f32), to.transform_vector3(n).normalize_or_zero());
                let name = model.mdl.bones.get(b).map_or("?", |x| x.name.as_str());
                println!("  part on bone {b} {name}: {c} verts, centroid {c_local:.2?} normal {n_local:.2?} in its frame");
            }
        }
        let bone = counts.iter().enumerate().max_by_key(|(_, c)| **c).map_or(0, |(b, _)| b);
        let to_bone = bind.get(bone).map_or(glam::Mat4::IDENTITY, |m| m.inverse());
        println!(
            "mesh {i} ({material}): {} verts, area {area:.1}, centroid {centroid:.2?} normal {normal:.2?}",
            mesh.vertices.len()
        );
        println!(
            "  mostly on bone {bone} {}: centroid {:.2?} normal {:.2?} in its frame",
            model.mdl.bones.get(bone).map_or("?", |b| b.name.as_str()),
            to_bone.transform_point3(centroid),
            to_bone.transform_vector3(normal).normalize_or_zero(),
        );
    }
}
