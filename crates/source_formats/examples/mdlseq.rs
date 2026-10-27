//! Prints a model's pose parameters, bones (with `--bones`) and sequences
//! (flags, blend grid, pose parameter axes).
//! Usage: `mdlseq <models/x.mdl> [--bones] [filter]`.
use source_formats::fs::GameFileSystem;
use source_formats::mdl::seq_flags;
use source_formats::studio::StudioModel;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: mdlseq <model> [--bones] [filter]");
    let rest: Vec<String> = args.collect();
    let bones = rest.iter().any(|a| a == "--bones");
    let filter = rest.iter().find(|a| !a.starts_with("--")).map(|s| s.to_ascii_lowercase());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let model = StudioModel::load(&fs, &path).expect("model not found");
    let mdl = &model.mdl;
    for (i, p) in mdl.pose_parameters.iter().enumerate() {
        println!("pose {i}: {p:?}");
    }
    if bones {
        for (i, b) in mdl.bones.iter().enumerate() {
            println!("bone {i}: {} parent {}", b.name, b.parent);
        }
    }
    for i in 0..model.sequence_count() {
        let Some((owner, local)) = model.sequence(i) else { continue };
        let s = &owner.mdl.sequences[local];
        if filter.as_ref().is_some_and(|f| !s.label.to_ascii_lowercase().contains(f)) {
            continue;
        }
        let flags = [(seq_flags::LOOPING, "loop"), (seq_flags::DELTA, "delta"), (seq_flags::POST, "post"), (seq_flags::AUTOPLAY, "autoplay")]
            .iter()
            .filter(|(f, _)| s.flags & f != 0)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "seq {i}: {} [{flags}] grid {:?} params {:?} {:?}..{:?} anims {} fade {}/{}",
            s.label,
            s.group_size,
            s.param_index,
            s.param_start,
            s.param_end,
            s.anims.len(),
            s.fade_in,
            s.fade_out
        );
    }
}
