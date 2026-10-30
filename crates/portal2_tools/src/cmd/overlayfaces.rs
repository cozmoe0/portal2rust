//! Prints the faces an overlay covers: `overlayfaces <map> <overlay id>`.
use source_formats::bsp::Bsp;

pub fn main() {
    let mut args = crate::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a1_intro2".to_owned());
    let id: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let Some(ov) = bsp.overlays.iter().find(|o| o.id == id) else {
        println!("no overlay {id}");
        return;
    };
    println!(
        "overlay {} origin {:?} normal {:?} u {:?} v {:?} uv points {:?} order {}",
        ov.id, ov.origin, ov.basis_normal, ov.u, ov.v, ov.uv_points, ov.render_order
    );
    for &fi in &ov.faces {
        let face = &bsp.faces[fi as usize];
        let plane = &bsp.planes[face.plane as usize];
        let verts = bsp.face_vertices(face);
        println!("face {fi}: disp {} plane {:?} {} side {} verts {:?}", face.dispinfo, plane.normal, plane.dist, face.side, verts);
        if face.dispinfo >= 0 {
            let d = &bsp.disp_infos[face.dispinfo as usize];
            let n = d.side_verts();
            let verts = &bsp.disp_verts[d.disp_vert_start as usize..d.disp_vert_start as usize + n * n];
            let alphas: Vec<f32> = verts.iter().map(|v| v.alpha).collect();
            let mean = alphas.iter().sum::<f32>() / alphas.len() as f32;
            let max = alphas.iter().copied().fold(0.0, f32::max);
            let min = alphas.iter().copied().fold(f32::MAX, f32::min);
            println!("  disp start {:?} power {} alpha mean {mean:.1} min {min} max {max}", d.start_position, d.power);
        }
    }
}
