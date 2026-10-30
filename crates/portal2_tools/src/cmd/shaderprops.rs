//! Lists the static props and model entities of maps whose materials (in the
//! skin they use) are drawn with a given shader, or with `env_cubemap` as the
//! shader name, whose materials reflect the nearest cube map: map, kind, model,
//! skin, origin and material name only. Usage: `shaderprops <shader> <map>
//! [map...]`, e.g. `shaderprops refract sp_a1_wakeup`.
use std::collections::HashMap;

use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::studio::StudioModel;
use source_formats::vmt::Vmt;

pub fn main() {
    let mut args = crate::args().skip(1);
    let shader = args.next().expect("usage: shaderprops <shader> <map> [map...]").to_ascii_lowercase();
    let fs = crate::game_fs();
    let mut matches: HashMap<String, bool> = HashMap::new();
    let mut matches_of = |fs: &GameFileSystem, path: &str| -> bool {
        *matches.entry(path.to_owned()).or_insert_with(|| {
            let text = fs.read_string(path).unwrap_or_default();
            let mut inc = |p: &str| fs.read_string(p);
            Vmt::parse(&text, &mut inc).is_some_and(|v| match shader.as_str() {
                "env_cubemap" => v.get("$envmap").is_some_and(|e| e.eq_ignore_ascii_case("env_cubemap")),
                _ => v.shader == shader,
            })
        })
    };
    for map in args {
        let Some(bytes) = fs.read(&format!("maps/{map}.bsp")) else {
            println!("{map}: not found");
            continue;
        };
        let Ok(bsp) = Bsp::parse(&bytes) else { continue };
        let mut uses: Vec<(String, String, i32, glam::Vec3)> = Vec::new();
        for prop in &bsp.static_props.props {
            if let Some(m) = bsp.static_props.models.get(prop.model as usize) {
                uses.push(("static".into(), m.clone(), prop.skin, prop.origin));
            }
        }
        for e in &bsp.entities {
            if let Some(m) = e.get("model").filter(|m| m.ends_with(".mdl")) {
                let skin = e.get("skin").and_then(|s| s.parse().ok()).unwrap_or(0);
                uses.push((e.classname().to_owned(), m.to_owned(), skin, e.origin()));
            }
        }
        for (kind, model, skin, origin) in uses {
            let Ok(studio) = StudioModel::load(&fs, &model) else { continue };
            let family = studio.mdl.skins.get(skin.max(0) as usize).or_else(|| studio.mdl.skins.first());
            let slots = family.map(|f| f.len()).unwrap_or(studio.mdl.textures.len());
            for slot in 0..slots {
                let tex = family.and_then(|f| f.get(slot)).map(|&t| t.max(0) as usize).unwrap_or(slot);
                let Some(material) = studio.resolve_material(&fs, tex) else { continue };
                if matches_of(&fs, &material) {
                    println!("{map} {kind} {model} skin {skin} at {origin:?}: {material}");
                }
            }
        }
    }
}
