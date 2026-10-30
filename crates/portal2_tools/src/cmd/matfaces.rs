//! Lists the faces whose material name contains a substring, with centre,
//! normal and size: `matfaces <map> <substring>`.
use glam::Vec3;
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a2_laser_over_goo".to_owned());
    let needle = args.next().unwrap_or_else(|| "toxicslime".to_owned()).to_ascii_lowercase();
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    for (fi, f) in bsp.faces.iter().enumerate() {
        let Some(name) = bsp.texinfo_material(f.texinfo as usize) else { continue };
        if !name.to_ascii_lowercase().contains(&needle) {
            continue;
        }
        let verts = bsp.face_vertices(f);
        if verts.is_empty() {
            continue;
        }
        let c = verts.iter().copied().sum::<Vec3>() / verts.len() as f32;
        let n = bsp.planes[f.plane as usize].normal;
        println!("face {fi:5} {name} centre {c:?} normal {n:?} size {:.0}", f.area);
    }
}
