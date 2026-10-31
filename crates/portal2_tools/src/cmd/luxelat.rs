//! Prints a face's lightmap luxel nearest a world point (displacements are
//! mapped through their base face): `luxelat <map> <face> x,y,z`.
use glam::Vec3;
use source_formats::bsp::Bsp;

pub fn main() {
    let args: Vec<String> = crate::args().skip(1).collect();
    let bsp = Bsp::parse(&crate::game_fs().read(&format!("maps/{}.bsp", args[0])).unwrap()).unwrap();
    let face = &bsp.faces[args[1].parse::<usize>().unwrap()];
    let v: Vec<f32> = args[2].split(',').map(|s| s.parse().unwrap()).collect();
    let p = Vec3::new(v[0], v[1], v[2]);
    let ti = &bsp.texinfo[face.texinfo as usize];
    let lm = |row: [f32; 4]| p.x * row[0] + p.y * row[1] + p.z * row[2] + row[3];
    let s = lm(ti.lightmap_vecs[0]) - face.lightmap_mins[0] as f32;
    let t = lm(ti.lightmap_vecs[1]) - face.lightmap_mins[1] as f32;
    let (w, h) = (face.lightmap_size[0] + 1, face.lightmap_size[1] + 1);
    let (x, y) = ((s.round() as i32).clamp(0, w - 1), (t.round() as i32).clamp(0, h - 1));
    let luxels = bsp.face_lightmap(face, 0).unwrap();
    println!("luxel ({s:.1}, {t:.1}) of {w}x{h}: {:?}", luxels[(y * w + x) as usize]);
}
