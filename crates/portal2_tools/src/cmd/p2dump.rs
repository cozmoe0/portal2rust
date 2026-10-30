//! Diagnostic dump of a Portal 2 installation: search paths, a map's lumps,
//! materials and texture formats. Usage: `p2dump [map_name]`.

use std::collections::BTreeMap;

use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::vmt::{Vmt, material_path};
use source_formats::vtf::Vtf;
use source_formats::zip::ZipArchive;

pub fn main() {
    let map = crate::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let root = crate::install_root();
    println!("install: {}", root.display());
    let fs = GameFileSystem::portal2(&root).expect("mount");
    println!("search paths: {:?}", fs.search_path_names());

    let t = std::time::Instant::now();
    let bytes = fs.read(&format!("maps/{map}.bsp")).expect("map not found");
    let bsp = Bsp::parse(&bytes).expect("bsp parse");
    println!("parsed {map} in {:?}: v{} rev {}", t.elapsed(), bsp.version, bsp.map_revision);
    println!(
        "planes {} verts {} faces {} texinfo {} texdata {} leafs {} nodes {} models {} brushes {} sides {}",
        bsp.planes.len(),
        bsp.vertices.len(),
        bsp.faces.len(),
        bsp.texinfo.len(),
        bsp.texdata.len(),
        bsp.leafs.len(),
        bsp.nodes.len(),
        bsp.models.len(),
        bsp.brushes.len(),
        bsp.brush_sides.len()
    );
    println!(
        "disp {} dispverts {} lighting {} bytes (hdr {}) cubemaps {} overlays {} worldlights {} ambient idx {} samples {}",
        bsp.disp_infos.len(),
        bsp.disp_verts.len(),
        bsp.lighting.len(),
        bsp.lighting_is_hdr,
        bsp.cubemaps.len(),
        bsp.overlays.len(),
        bsp.world_lights.len(),
        bsp.leaf_ambient_index.len(),
        bsp.leaf_ambient_samples.len()
    );
    println!("primitives {} primverts {} primindices {}", bsp.primitives.len(), bsp.prim_verts.len(), bsp.prim_indices.len());
    for g in &bsp.game_lumps {
        println!("game lump {:?} v{} {} bytes", String::from_utf8_lossy(&g.id), g.version, g.data.len());
    }
    println!(
        "static props v{}: {} models, {} instances",
        bsp.static_props.version,
        bsp.static_props.models.len(),
        bsp.static_props.props.len()
    );

    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    for e in &bsp.entities {
        *classes.entry(e.classname().to_owned()).or_default() += 1;
    }
    println!("{} entities, {} classes:", bsp.entities.len(), classes.len());
    for (c, n) in &classes {
        print!("{c}({n}) ");
    }
    println!();

    let pak = ZipArchive::new(bsp.pakfile.clone()).expect("pak");
    println!("pakfile entries: {}", pak.len());
    fs.mount_map_pakfile(pak);

    // Materials used by the map.
    let mut shaders: BTreeMap<String, usize> = BTreeMap::new();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing = 0;
    for name in &bsp.texdata_strings {
        let path = material_path(name);
        let Some(text) = fs.read_string(&path) else {
            missing += 1;
            println!("  missing material #{missing}");
            continue;
        };
        let mut inc = |p: &str| fs.read_string(p);
        let Some(vmt) = Vmt::parse(&text, &mut inc) else {
            println!("  unparsable {path}");
            continue;
        };
        *shaders.entry(vmt.shader.clone()).or_default() += 1;
        for param in ["$basetexture", "$bumpmap", "$envmap", "$envmapmask", "$basetexture2", "$selfillummask", "$detail"] {
            if let Some(tex) = vmt.texture_path(param) {
                if param == "$envmap" && tex.ends_with("env_cubemap.vtf") {
                    continue;
                }
                match fs.read(&tex).map(|b| Vtf::parse(&b)) {
                    Some(Ok(vtf)) => {
                        *formats.entry(format!("{param} {:?}", vtf.format)).or_default() += 1;
                    }
                    Some(Err(e)) => println!("  bad vtf {tex}: {e}"),
                    None => println!("  missing texture {tex} ({param} of {path})"),
                }
            }
        }
    }
    println!("materials: {} missing {missing}", bsp.texdata_strings.len());
    println!("shaders: {shaders:?}");
    for (f, n) in &formats {
        println!("  {f}: {n}");
    }
}
