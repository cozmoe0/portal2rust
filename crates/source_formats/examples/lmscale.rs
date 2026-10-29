//! Compares floor lightmap luxels with the +Z ambient cube face just above them.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let map = std::env::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let mut ratios = Vec::new();
    for f in &bsp.faces {
        if f.light_offset < 0 || f.dispinfo >= 0 || f.texinfo < 0 {
            continue;
        }
        let n = bsp.planes[f.plane as usize].normal;
        if n.z < 0.95 {
            continue;
        }
        let verts = bsp.face_vertices(f);
        let center = verts.iter().fold(glam::Vec3::ZERO, |a, &b| a + b) / verts.len() as f32;
        let ti = &bsp.texinfo[f.texinfo as usize];
        let ls = ti.lightmap_vecs[0];
        let lt = ti.lightmap_vecs[1];
        let s = center.x * ls[0] + center.y * ls[1] + center.z * ls[2] + ls[3] - f.lightmap_mins[0] as f32;
        let t = center.x * lt[0] + center.y * lt[1] + center.z * lt[2] + lt[3] - f.lightmap_mins[1] as f32;
        let w = f.lightmap_size[0] + 1;
        let (si, tii) = (s.round().clamp(0.0, (w - 1) as f32) as i32, t.round().clamp(0.0, f.lightmap_size[1] as f32) as i32);
        let Some(lm) = bsp.face_lightmap(f, 0) else { continue };
        let lum = |c: glam::Vec3| c.x * 0.2125 + c.y * 0.7154 + c.z * 0.0721;
        let l = lum(lm[(tii * w + si) as usize]);
        let Some(cube) = bsp.ambient_cube(center + glam::Vec3::Z * 16.0) else { continue };
        let a = lum(cube[4]);
        if l > 1e-4 && a > 1e-6 {
            ratios.push(a / l);
        }
    }
    ratios.sort_by(|a, b| a.total_cmp(b));
    if ratios.is_empty() {
        println!("no samples");
        return;
    }
    println!(
        "{map}: {} floor samples, ambient(+Z)/lightmap median {:.3} p25 {:.3} p75 {:.3}",
        ratios.len(),
        ratios[ratios.len() / 2],
        ratios[ratios.len() / 4],
        ratios[ratios.len() * 3 / 4]
    );
}
