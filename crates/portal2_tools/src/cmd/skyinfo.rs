//! Per singleplayer map: the 2D sky name, the 3D sky camera and how many
//! faces show the sky: `skyinfo [map ...]`.
use source_formats::bsp::Bsp;

pub fn main() {
    let fs = crate::game_fs();
    let mut maps: Vec<String> = crate::args().skip(1).collect();
    if maps.is_empty() {
        maps = fs
            .list("maps", Some("bsp"), false)
            .into_iter()
            .filter_map(|p| {
                let name = p.rsplit('/').next()?.trim_end_matches(".bsp").to_owned();
                name.starts_with("sp_").then_some(name)
            })
            .collect();
        maps.sort();
    }
    for map in maps {
        let Some(bytes) = fs.read(&format!("maps/{map}.bsp")) else { continue };
        let Ok(bsp) = Bsp::parse(&bytes) else { continue };
        let sky = bsp.entities.iter().find(|e| e.get("classname") == Some("worldspawn")).and_then(|e| e.get("skyname")).unwrap_or("");
        let cams: Vec<String> = bsp
            .entities
            .iter()
            .filter(|e| e.get("classname") == Some("sky_camera"))
            .map(|e| format!("{} scale {}", e.get("origin").unwrap_or(""), e.get("scale").unwrap_or("")))
            .collect();
        let (mut sky3d, mut sky2d) = (0, 0);
        for f in &bsp.faces {
            let flags = bsp.texinfo.get(f.texinfo as usize).map_or(0, |t| t.flags);
            if flags & 0x4 != 0 {
                sky3d += 1;
            }
            if flags & 0x2 != 0 {
                sky2d += 1;
            }
        }
        println!("{map:28} sky {sky:20} faces sky {sky3d:4} sky2d {sky2d:4} cameras {cams:?}");
    }
}
