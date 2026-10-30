//! Lists a map's overlays by material, with the material's shader and the
//! number of faces they cover: `overlays <map>`.
use std::collections::BTreeMap;

use source_formats::bsp::Bsp;

pub fn main() {
    let map = crate::args().nth(1).unwrap_or_else(|| "sp_a1_intro2".to_owned());
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let mut by_material: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for ov in &bsp.overlays {
        let Some(ti) = bsp.texinfo.get(ov.texinfo.max(0) as usize) else { continue };
        let td = &bsp.texdata[ti.texdata as usize];
        let name = bsp.texdata_strings.get(td.name_index as usize).cloned().unwrap_or_default();
        let e = by_material.entry(name).or_default();
        e.0 += 1;
        e.1 += ov.faces.len();
    }
    for (name, (count, faces)) in by_material {
        let path = source_formats::vmt::material_path(&name);
        let shader = fs
            .read_string(&path)
            .and_then(|t| source_formats::vmt::Vmt::parse(&t, &mut |p| fs.read_string(&format!("materials/{p}"))))
            .map(|v| v.shader)
            .unwrap_or_else(|| "?".into());
        println!("{count:3} x {name} ({shader}), {faces} faces");
    }
}
