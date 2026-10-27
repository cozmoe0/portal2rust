//! Prints selected numeric parameters of the materials a map uses, filtered by
//! shader. Only the requested parameters are shown; material files are never
//! echoed. Usage: `vmtparams <map> <shader> <$param> [$param...]`.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::vmt::{Vmt, material_path};
use source_formats::zip::ZipArchive;

fn main() {
    let mut args = std::env::args().skip(1);
    let map = args.next().unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let shader = args.next().unwrap_or_else(|| "water".to_owned());
    let params: Vec<String> = args.collect();
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    fs.mount_map_pakfile(ZipArchive::new(bsp.pakfile.clone()).unwrap());
    let mut count = 0;
    for name in &bsp.texdata_strings {
        let Some(text) = fs.read_string(&material_path(name)) else { continue };
        let mut inc = |p: &str| fs.read_string(p);
        let Some(vmt) = Vmt::parse(&text, &mut inc) else { continue };
        if vmt.shader != shader {
            continue;
        }
        count += 1;
        let values: Vec<String> = params
            .iter()
            .map(|p| match vmt.get_vec3(p) {
                Some(v) => format!("{p}={:.3},{:.3},{:.3}", v.x, v.y, v.z),
                None => format!("{p}=-"),
            })
            .collect();
        println!("#{count}: {}", values.join(" "));
    }
}
