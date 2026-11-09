//! Loads studio models and prints structural stats (no content dump).
use source_formats::studio::StudioModel;

pub fn main() {
    let fs = crate::game_fs();
    let paths: Vec<String> = crate::args().skip(1).collect();
    let paths = if paths.is_empty() {
        vec![
            "models/props/metal_box.mdl".into(),
            "models/props/portal_door_combined.mdl".into(),
            "models/props/laser_emitter_center.mdl".into(),
            "models/weapons/v_portalgun.mdl".into(),
            "models/props/elevator_tube_opener.mdl".into(),
            "models/props_destruction/wall_dest_section03.mdl".into(),
        ]
    } else {
        paths
    };
    for p in paths {
        match StudioModel::load(&fs, &p) {
            Ok(m) => {
                let verts: usize = m.meshes.iter().map(|x| x.vertices.len()).sum();
                let tris: usize = m.meshes.iter().map(|x| x.indices.len() / 3).sum();
                println!(
                    "{p}: bones {} meshes {} verts {verts} tris {tris} textures {} skins {} seqs {} (total {}) anims {} includes {} attachments {} poseparams {} anim_block {:?}",
                    m.mdl.bones.len(),
                    m.meshes.len(),
                    m.mdl.textures.len(),
                    m.mdl.skins.len(),
                    m.mdl.sequences.len(),
                    m.sequence_count(),
                    m.mdl.anims.len(),
                    m.includes.len(),
                    m.mdl.attachments.len(),
                    m.mdl.pose_parameters.len(),
                    m.anim_block_data.as_ref().map(|d| d.len())
                );
                println!(
                    "   illum position {:?} hull {:?}..{:?} contents {:#x} surfaceprop {}",
                    m.mdl.illum_position, m.mdl.hull_min, m.mdl.hull_max, m.mdl.contents, m.mdl.surface_prop
                );
                for t in 0..m.mdl.textures.len() {
                    println!("   material[{t}] -> {:?}", m.resolve_material(&fs, t));
                }
                let seqs: Vec<String> =
                    (0..m.sequence_count().min(12)).filter_map(|i| m.sequence(i).map(|(o, l)| o.mdl.sequences[l].label.clone())).collect();
                println!("   sequences: {seqs:?}");
                // Sample the last listed sequence halfway to check animation decoding.
                if m.sequence_count() > 0 {
                    let mut pose = vec![(glam::Vec3::ZERO, glam::Quat::IDENTITY); m.mdl.bones.len()];
                    let s = m.sequence_count() - 1;
                    m.sample_sequence(s, 0.5, &mut pose);
                    let bad = pose.iter().filter(|(p, q)| !p.is_finite() || !q.is_finite() || (q.length() - 1.0).abs() > 0.01).count();
                    println!("   sampled seq {s}: non-finite/unnormalized bones {bad}, root {:?}", pose.first());
                }
            }
            Err(e) => println!("{p}: ERROR {e}"),
        }
    }
}
