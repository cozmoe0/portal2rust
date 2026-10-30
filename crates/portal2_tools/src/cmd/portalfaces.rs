//! Lists the largest portal-accepting wall/floor faces near a point
//! (centre, normal, area). Usage: portalfaces <map> <x> <y> <z> [count]
use source_formats::bsp::{Bsp, surf};

pub fn main() {
    let a: Vec<String> = crate::args().skip(1).collect();
    let map = &a[0];
    let p = glam::Vec3::new(a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap());
    let count: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(8);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let mut faces: Vec<(f32, glam::Vec3, glam::Vec3, f32)> = Vec::new();
    for f in &bsp.faces {
        if f.texinfo < 0 || f.dispinfo >= 0 || f.area < 64.0 * 112.0 {
            continue;
        }
        let ti = &bsp.texinfo[f.texinfo as usize];
        if ti.flags & (surf::NOPORTAL | surf::NODRAW | surf::SKY | surf::TRIGGER) != 0 {
            continue;
        }
        let v = bsp.face_vertices(f);
        let c = v.iter().fold(glam::Vec3::ZERO, |s, &x| s + x) / v.len() as f32;
        let n = bsp.planes[f.plane as usize].normal;
        if n.dot(p - c) <= 0.0 {
            continue;
        } // must face the point
        faces.push((c.distance(p), c, n, f.area));
    }
    faces.sort_by(|x, y| x.0.total_cmp(&y.0));
    for (d, c, n, area) in faces.iter().take(count) {
        println!("dist {d:6.0} centre ({:.0}, {:.0}, {:.0}) normal ({:.2}, {:.2}, {:.2}) area {area:.0}", c.x, c.y, c.z, n.x, n.y, n.z);
    }
}
