//! Lists the faces passing through a point with their materials:
//! `faceat <map> <x> <y> <z>`.
use glam::Vec3;
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = args.first().cloned().unwrap_or_else(|| "sp_a1_intro2".to_owned());
    let p = Vec3::new(args[1].parse().unwrap(), args[2].parse().unwrap(), args[3].parse().unwrap());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for (fi, face) in bsp.faces.iter().enumerate() {
        let plane = &bsp.planes[face.plane as usize];
        let n = if face.side != 0 { -plane.normal } else { plane.normal };
        let d = plane.normal.dot(p) - plane.dist;
        if d.abs() > 2.0 && face.dispinfo < 0 {
            continue;
        }
        let verts = bsp.face_vertices(face);
        if verts.len() < 3 {
            continue;
        }
        // Inside test against the edges, in the face plane.
        let inside = (0..verts.len()).all(|i| {
            let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
            (b - a).cross(p - a).dot(n) <= 0.5
        }) || (0..verts.len()).all(|i| {
            let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
            (b - a).cross(p - a).dot(n) >= -0.5
        });
        if !inside || (face.dispinfo >= 0 && d.abs() > 64.0) {
            continue;
        }
        let ti = &bsp.texinfo[face.texinfo as usize];
        let td = &bsp.texdata[ti.texdata as usize];
        let name = bsp.texdata_strings.get(td.name_index as usize).cloned().unwrap_or_default();
        let model =
            bsp.models.iter().position(|m| (m.first_face..m.first_face + m.num_faces).contains(&(fi as i32))).map_or(-1, |m| m as i32);
        let owner = bsp
            .entities
            .iter()
            .find(|e| e.get("model") == Some(format!("*{model}").as_str()))
            .map(|e| format!("{} {}", e.get("classname").map_or("", |s| s), e.get("targetname").map_or("", |s| s)))
            .unwrap_or_default();
        let wind = (verts[1] - verts[0]).cross(verts[2] - verts[0]).normalize_or_zero();
        println!("  model {model} {owner} winding cross {wind:?} verts {verts:?}");
        let lm = bsp.face_lightmap(face, 0).map(|l| {
            let sum: Vec3 = l.iter().copied().sum();
            (sum / l.len().max(1) as f32, l.len())
        });
        println!(
            "face {fi}: {name} disp {} normal {n:?} dist {d:.2} texinfo {} styles {:?} lightofs {} size {:?} mean light {lm:?}",
            face.dispinfo, face.texinfo, face.styles, face.light_offset, face.lightmap_size
        );
    }
}
