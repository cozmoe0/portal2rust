//! Lists the events of a model's sequences: `mdlevents <model> [filter]`.
use source_formats::fs::GameFileSystem;
use source_formats::studio::StudioModel;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: mdlevents <model> [filter]");
    let filter = args.next().map(|s| s.to_ascii_lowercase());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let model = StudioModel::load(&fs, &path).expect("model not found");
    for i in 0..model.sequence_count() {
        let Some((owner, local)) = model.sequence(i) else { continue };
        let s = &owner.mdl.sequences[local];
        if filter.as_ref().is_some_and(|f| !s.label.to_ascii_lowercase().contains(f)) {
            continue;
        }
        let (fps, frames) = model.sequence_timing(i);
        for e in &s.events {
            println!(
                "{} ({:.2} s): cycle {:.4} event {} name {:?} options {:?}",
                s.label,
                (frames - 1) as f32 / fps,
                e.cycle,
                e.event,
                e.name,
                e.options
            );
        }
    }
}
