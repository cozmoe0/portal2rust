//! Winding and lightmap brightness diagnostics.
use source_formats::bsp::{Bsp, surf};
use source_formats::fs::GameFileSystem;

fn newell(v: &[glam::Vec3]) -> glam::Vec3 {
    let mut n = glam::Vec3::ZERO;
    for i in 0..v.len() { let a = v[i]; let b = v[(i + 1) % v.len()]; n += glam::Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y)); }
    n.normalize_or_zero()
}

fn main() {
    let map = std::env::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let (mut a_side, mut d_side, mut a_plane, mut d_plane) = (0, 0, 0, 0);
    let mut by_side = [[0; 2]; 2];
    for f in bsp.faces.iter().filter(|f| f.dispinfo < 0) {
        let v = bsp.face_vertices(f);
        if v.len() < 3 { continue; }
        let n = newell(&v);
        let pn = bsp.planes[f.plane as usize].normal;
        let ps = pn * if f.side != 0 { -1.0 } else { 1.0 };
        if n.dot(ps) > 0.5 { a_side += 1 } else { d_side += 1 }
        if n.dot(pn) > 0.5 { a_plane += 1 } else { d_plane += 1 }
        by_side[(f.side != 0) as usize][(n.dot(pn) > 0.5) as usize] += 1;
    }
    println!("vs side-adjusted normal: agree {a_side} opposite {d_side}; vs raw plane normal: agree {a_plane} opposite {d_plane}; by side [side][agrees raw] {by_side:?}");
    // Brightness of lit faces by material keyword.
    for key in ["white", "metal", "glass", "concrete"] {
        let mut vals: Vec<f32> = Vec::new();
        for f in &bsp.faces {
            if f.texinfo < 0 || f.light_offset < 0 { continue; }
            let ti = &bsp.texinfo[f.texinfo as usize];
            if ti.flags & surf::NODRAW != 0 { continue; }
            let name = bsp.texinfo_material(f.texinfo as usize).unwrap_or("").to_lowercase();
            if !name.contains(key) { continue; }
            if let Some(s) = bsp.face_lightmap(f, 0) { vals.extend(s.iter().map(|c| c.max_element())); }
        }
        vals.sort_by(|a, b| a.total_cmp(b));
        if vals.is_empty() { continue; }
        let p = |q: f32| vals[((vals.len() - 1) as f32 * q) as usize];
        println!("{key}: n={} p10 {:.3} p50 {:.3} p90 {:.3} p99 {:.3}", vals.len(), p(0.1), p(0.5), p(0.9), p(0.99));
    }
    // A few raw samples from the brightest face.
    let mut best = (0.0f32, 0usize);
    for (i, f) in bsp.faces.iter().enumerate() { if let Some(s) = bsp.face_lightmap(f, 0) { let m = s.iter().map(|c| c.max_element()).sum::<f32>() / s.len() as f32; if m > best.0 { best = (m, i); } } }
    let f = &bsp.faces[best.1];
    let o = f.light_offset as usize;
    println!("brightest face {} mean {:.3} mat {:?} raw {:?} styles {:?} flags {:#x}", best.1, best.0, bsp.texinfo_material(f.texinfo as usize), &bsp.lighting[o..o + 16], f.styles, bsp.texinfo[f.texinfo as usize].flags);
}
