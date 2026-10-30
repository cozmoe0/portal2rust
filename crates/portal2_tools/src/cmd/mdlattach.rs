//! Lists a model's attachments with their bones: `mdlattach <model path>`.
use source_formats::studio::StudioModel;

pub fn main() {
    let path = crate::args().nth(1).expect("usage: mdlattach <model path>");
    let fs = crate::game_fs();
    let model = StudioModel::load(&fs, &path).expect("model loads");
    for (i, a) in model.mdl.attachments.iter().enumerate() {
        let bone = model.mdl.bones.get(a.bone.max(0) as usize).map(|b| b.name.as_str()).unwrap_or("?");
        println!("{i}: {} on {} at {:?}", a.name, bone, a.local.w_axis.truncate());
    }
}
