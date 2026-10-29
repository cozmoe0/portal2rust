//! Counts drawn faces whose surface accepts portals (no no-portal flag and no
//! %noportal material). Prints counts only.
use source_formats::bsp::{Bsp, surf};
use source_formats::fs::GameFileSystem;
use source_formats::vmt::{Vmt, material_path};
use source_formats::zip::ZipArchive;

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    for map in ["sp_a2_laser_intro", "sp_a1_intro4", "sp_a2_intro", "sp_a2_catapult_intro"] {
        let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
        fs.mount_map_pakfile(ZipArchive::new(bsp.pakfile.clone()).unwrap());
        let (mut yes, mut no_flag, mut no_mat, mut area_yes, mut area_all) = (0, 0, 0, 0.0f32, 0.0f32);
        for f in &bsp.faces {
            if f.texinfo < 0 || f.dispinfo >= 0 {
                continue;
            }
            let ti = &bsp.texinfo[f.texinfo as usize];
            if ti.flags & (surf::NODRAW | surf::SKY | surf::TRIGGER | surf::SKIP | surf::HINT) != 0 {
                continue;
            }
            area_all += f.area;
            if ti.flags & surf::NOPORTAL != 0 {
                no_flag += 1;
                continue;
            }
            let name = bsp.texinfo_material(f.texinfo as usize).unwrap_or("");
            let text = fs.read_string(&material_path(name)).unwrap_or_default();
            let mut inc = |p: &str| fs.read_string(p);
            let noportal = Vmt::parse(&text, &mut inc).is_some_and(|v| v.get_bool("%noportal"));
            if noportal {
                no_mat += 1
            } else {
                yes += 1;
                area_yes += f.area;
            }
        }
        println!(
            "{map}: portalable faces {yes} ({:.0}% of drawn area), rejected by flag {no_flag}, by material {no_mat}",
            100.0 * area_yes / area_all.max(1.0)
        );
    }
}
