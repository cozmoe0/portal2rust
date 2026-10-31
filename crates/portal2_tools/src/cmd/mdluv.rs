//! Prints the range of a model's texture coordinates and normals (LOD 0
//! vertices): `mdluv <model.mdl>`.
use source_formats::vvd::Vvd;

pub fn main() {
    let path = crate::args().nth(1).expect("usage: mdluv <model.mdl>");
    let fs = crate::game_fs();
    let vvd = Vvd::parse(&fs.read(&path.replace(".mdl", ".vvd")).expect("no .vvd")).expect("bad .vvd");
    let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
    let mut bad_normals = 0;
    for v in &vvd.vertices {
        lo = lo.min(v.uv);
        hi = hi.max(v.uv);
        if (v.normal.length() - 1.0).abs() > 0.05 {
            bad_normals += 1;
        }
    }
    println!("{} vertices, uv {lo:?} .. {hi:?}, {bad_normals} normals not unit", vvd.vertices.len());
    for v in vvd.vertices.iter().take(6) {
        println!("  pos {:?} uv {:?} bones {:?} weights {:?}", v.pos, v.uv, v.bones, v.weights);
    }
}
