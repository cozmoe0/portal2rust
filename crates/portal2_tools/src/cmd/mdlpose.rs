//! Prints bones of a model posed by one sequence across its cycle:
//! `mdlpose <model path> <sequence> [steps] <bone substring>...`. Each line
//! has the bone's model-space position, its forward (x) and up (z) axes and
//! its rotation against its parent in degrees.
use source_formats::studio::StudioModel;

pub fn main() {
    let mut args = crate::args().skip(1);
    let usage = "usage: mdlpose <model path> <sequence> [steps] <bone substring>...";
    let path = args.next().expect(usage);
    let seq_name = args.next().expect(usage);
    let mut rest: Vec<String> = args.collect();
    let steps = match rest.first().and_then(|s| s.parse::<usize>().ok()) {
        Some(n) => {
            rest.remove(0);
            n.max(1)
        }
        None => 10,
    };
    let fs = crate::game_fs();
    let model = StudioModel::load(&fs, &path).expect("model loads");
    let seq = model.find_sequence(&seq_name).or_else(|| seq_name.parse().ok()).expect("sequence exists");
    let wanted: Vec<usize> = model
        .mdl
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| rest.is_empty() || rest.iter().any(|r| b.name.to_ascii_lowercase().contains(&r.to_ascii_lowercase())))
        .map(|(i, _)| i)
        .collect();
    let mut local = model.bind_pose();
    for step in 0..=steps {
        let cycle = step as f32 / steps as f32;
        model.sample_sequence(seq, cycle, &mut local);
        let world = model.bone_matrices(&local);
        println!("cycle {cycle:.2}");
        for &i in &wanted {
            let m = world[i];
            let (x, y, z) = local[i].1.to_euler(glam::EulerRot::ZYX);
            println!(
                "  {:<28} at {:7.2?} fwd {:5.2?} up {:5.2?} local rot z{:7.1} y{:7.1} x{:7.1} pos {:6.2?}",
                model.mdl.bones[i].name,
                m.w_axis.truncate(),
                m.x_axis.truncate(),
                m.z_axis.truncate(),
                x.to_degrees(),
                y.to_degrees(),
                z.to_degrees(),
                local[i].0,
            );
        }
    }
}
