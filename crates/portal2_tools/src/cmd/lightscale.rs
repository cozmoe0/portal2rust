//! Compares lightmap values on floors with the direct light predicted from
//! the map's world lights (PVS-visible point and spot lights only).
use source_formats::bsp::Bsp;

pub fn main() {
    let map = crate::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let mut kinds = std::collections::BTreeMap::new();
    for l in &bsp.world_lights {
        *kinds.entry(l.kind).or_insert(0) += 1;
    }
    println!("world lights by type {kinds:?}");
    if let Some(l) = bsp.world_lights.iter().find(|l| l.kind == 1 || l.kind == 2) {
        println!(
            "sample light: intensity {:?} attn c{} l{} q{} radius {} exponent {} stopdot {} {}",
            l.intensity, l.constant_attn, l.linear_attn, l.quadratic_attn, l.radius, l.exponent, l.stopdot, l.stopdot2
        );
    }
    let lum = |c: glam::Vec3| c.x * 0.2125 + c.y * 0.7154 + c.z * 0.0721;
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
        let p = verts.iter().fold(glam::Vec3::ZERO, |a, &b| a + b) / verts.len() as f32;
        let leaf = &bsp.leafs[bsp.find_leaf(p + n * 2.0)];
        let Some(vis) = bsp.visibility.as_ref() else { break };
        if leaf.cluster < 0 {
            continue;
        }
        let pvs = vis.pvs(leaf.cluster as usize);
        let mut direct = glam::Vec3::ZERO;
        for l in &bsp.world_lights {
            if !(l.kind == 1 || l.kind == 2) {
                continue;
            }
            if l.cluster >= 0 && pvs[(l.cluster / 8) as usize] & (1 << (l.cluster % 8)) == 0 {
                continue;
            }
            let d = l.origin - p;
            let dist = d.length().max(1.0);
            let dir = d / dist;
            let ndl = n.dot(dir).max(0.0);
            let mut att = 1.0 / (l.constant_attn + l.linear_attn * dist + l.quadratic_attn * dist * dist).max(1e-4);
            if l.kind == 2 {
                let dot = (-dir).dot(l.normal);
                if dot < l.stopdot2 {
                    continue;
                }
                if dot < l.stopdot {
                    att *= (dot - l.stopdot2) / (l.stopdot - l.stopdot2);
                }
            }
            direct += l.intensity * ndl * att;
        }
        let ti = &bsp.texinfo[f.texinfo as usize];
        let (ls, lt) = (ti.lightmap_vecs[0], ti.lightmap_vecs[1]);
        let s = p.x * ls[0] + p.y * ls[1] + p.z * ls[2] + ls[3] - f.lightmap_mins[0] as f32;
        let t = p.x * lt[0] + p.y * lt[1] + p.z * lt[2] + lt[3] - f.lightmap_mins[1] as f32;
        let w = f.lightmap_size[0] + 1;
        let si = s.round().clamp(0.0, (w - 1) as f32) as i32;
        let ti2 = t.round().clamp(0.0, f.lightmap_size[1] as f32) as i32;
        let Some(lm) = bsp.face_lightmap(f, 0) else { continue };
        let l = lum(lm[(ti2 * w + si) as usize]);
        let dl = lum(direct);
        if dl > 1e-5 && l > 1e-4 {
            ratios.push(l / dl);
        }
    }
    ratios.sort_by(|a, b| a.total_cmp(b));
    if ratios.is_empty() {
        println!("no samples");
        return;
    }
    println!(
        "{map}: {} samples, lightmap/direct median {:.4} p25 {:.4} p75 {:.4}",
        ratios.len(),
        ratios[ratios.len() / 2],
        ratios[ratios.len() / 4],
        ratios[ratios.len() * 3 / 4]
    );
}
