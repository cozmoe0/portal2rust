//! Lists the faces that show the sky (`SURF_SKY` / `SURF_SKY2D`) of a map:
//! centre, normal, area and the BSP area they face from: `skyfaces <map>`.
use glam::Vec3;
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let map = std::env::args().nth(1).unwrap_or_else(|| "sp_a1_intro3".to_owned());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let sky_area = bsp
        .entities
        .iter()
        .find(|e| e.classname() == "sky_camera")
        .map(|e| bsp.leafs[bsp.find_leaf(e.origin())].area);
    println!("sky camera area {sky_area:?}");
    for (fi, f) in bsp.faces.iter().enumerate() {
        let flags = bsp.texinfo.get(f.texinfo as usize).map_or(0, |t| t.flags);
        if flags & 0x6 == 0 {
            continue;
        }
        let verts = bsp.face_vertices(f);
        if verts.is_empty() {
            continue;
        }
        let c = verts.iter().copied().sum::<Vec3>() / verts.len() as f32;
        let n = bsp.planes[f.plane as usize].normal;
        let model = bsp
            .models
            .iter()
            .position(|m| (m.first_face..m.first_face + m.num_faces).contains(&(fi as i32)))
            .map_or(-1, |m| m as i32);
        let leaf_area = bsp.leafs[bsp.find_leaf(c + n * 2.0)].area;
        if Some(leaf_area) == sky_area {
            continue;
        }
        println!("face {fi:5} model {model} flags {flags:#x} centre {c:?} normal {n:?} size {:.0} bsp area {leaf_area}", f.area);
    }
}
