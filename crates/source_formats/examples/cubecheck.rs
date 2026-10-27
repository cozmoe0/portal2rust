//! Reports formats and value ranges of a map's baked cube maps.
use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;
use source_formats::vtf::Vtf;
use source_formats::zip::ZipArchive;

fn main() {
    let map = std::env::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let pak = ZipArchive::new(bsp.pakfile.clone()).unwrap();
    let mut names: Vec<String> = pak.paths().filter(|p| p.ends_with(".vtf")).map(|s| s.to_owned()).collect();
    names.sort();
    for n in names.iter().take(6) {
        let v = Vtf::parse(&pak.read(n).unwrap()).unwrap();
        let px = v.decode_rgba_f32(0, 0, 4);
        let mut mx = [0f32; 4];
        let mut mean = [0f64; 4];
        for p in &px { for i in 0..4 { mx[i] = mx[i].max(p[i]); mean[i] += p[i] as f64; } }
        for m in mean.iter_mut() { *m /= px.len() as f64; }
        println!("{n}: {:?} {}x{} mips {} faces {} flags {:#x} max {:?} mean(up face) {:?}", v.format, v.width, v.height, v.mip_count, v.faces, v.flags, mx, mean);
    }
}
