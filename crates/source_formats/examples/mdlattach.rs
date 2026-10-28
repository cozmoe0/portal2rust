//! Lists a model's attachments with their bones: `mdlattach <model path>`.
use source_formats::fs::GameFileSystem;
use source_formats::studio::StudioModel;

fn main() {
    let path = std::env::args().nth(1).expect("usage: mdlattach <model path>");
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let model = StudioModel::load(&fs, &path).expect("model loads");
    for (i, a) in model.mdl.attachments.iter().enumerate() {
        let bone = model.mdl.bones.get(a.bone.max(0) as usize).map(|b| b.name.as_str()).unwrap_or("?");
        println!("{i}: {} on {} at {:?}", a.name, bone, a.local.w_axis.truncate());
    }
}
