//! Compares static prop lighting file layout with model mesh vertex counts.
use source_formats::bsp::Bsp;
use source_formats::studio::StudioModel;
use source_formats::zip::ZipArchive;

pub fn main() {
    let map = crate::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let pak = ZipArchive::new(bsp.pakfile.clone()).unwrap();
    for i in [0usize, 1, 2, 20] {
        let Some(prop) = bsp.static_props.props.get(i) else { continue };
        let Ok(d) = pak.read(&format!("sp_hdr_{i}.vhv")) else {
            println!("prop {i}: no vhv");
            continue;
        };
        let h: Vec<u32> = (0..10).map(|k| u32::from_le_bytes(d[k * 4..k * 4 + 4].try_into().unwrap())).collect();
        let nmesh = h[5] as usize;
        let meshes: Vec<(u32, u32, u32)> = (0..nmesh)
            .map(|m| {
                let o = 40 + m * 28;
                let r = |k: usize| u32::from_le_bytes(d[o + k * 4..o + k * 4 + 4].try_into().unwrap());
                (r(0), r(1), r(2))
            })
            .collect();
        let model = StudioModel::load(&fs, &bsp.static_props.models[prop.model as usize]).unwrap();
        let mdl_counts: Vec<usize> = model.meshes.iter().map(|m| m.vertices.len()).collect();
        let first = meshes.first().map(|m| m.2 as usize).unwrap_or(0);
        println!(
            "prop {i}: header version {} vflags {:#x} vsize {} verts {} meshes {nmesh} -> (lod, verts, offset) {:?}; mdl mesh verts {:?}; file {} bytes; first bytes {:?}",
            h[0],
            h[2],
            h[3],
            h[4],
            meshes,
            mdl_counts,
            d.len(),
            &d[first..(first + 12).min(d.len())]
        );
    }
}
