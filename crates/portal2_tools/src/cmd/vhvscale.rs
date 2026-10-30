//! Compares baked static prop vertex colours with ambient-cube brightness at
//! the prop origin under different decodings.
use source_formats::bsp::Bsp;
use source_formats::zip::ZipArchive;

pub fn main() {
    let map = crate::args().nth(1).unwrap_or_else(|| "sp_a2_laser_intro".to_owned());
    let fs = crate::game_fs();
    let bsp = Bsp::parse(&fs.read(&format!("maps/{map}.bsp")).unwrap()).unwrap();
    let pak = ZipArchive::new(bsp.pakfile.clone()).unwrap();
    let mut ratios = [Vec::new(), Vec::new(), Vec::new()];
    for (i, prop) in bsp.static_props.props.iter().enumerate() {
        let Ok(d) = pak.read(&format!("sp_hdr_{i}.vhv")) else { continue };
        let off = u32::from_le_bytes(d[48..52].try_into().unwrap()) as usize;
        let n = u32::from_le_bytes(d[44..48].try_into().unwrap()) as usize;
        let mut sums = [0f64; 3];
        for v in 0..n {
            let p = &d[off + v * 4..off + v * 4 + 4];
            let l = (p[2] as f64 * 0.2125 + p[1] as f64 * 0.7154 + p[0] as f64 * 0.0721) / 255.0;
            sums[0] += l;
            sums[1] += (l * 2.0).powf(2.2);
            sums[2] += l.powf(2.2);
        }
        let Some(cube) = bsp.ambient_cube(prop.origin + glam::Vec3::Z * 8.0) else { continue };
        let amb: f64 = cube.iter().map(|c| (c.x * 0.2125 + c.y * 0.7154 + c.z * 0.0721) as f64).sum::<f64>() / 6.0;
        if amb < 1e-4 {
            continue;
        }
        for k in 0..3 {
            ratios[k].push(sums[k] / n as f64 / amb);
        }
    }
    for (k, name) in ["linear c/255", "gamma (2c/255)^2.2", "gamma (c/255)^2.2"].iter().enumerate() {
        let r = &mut ratios[k];
        r.sort_by(|a, b| a.total_cmp(b));
        if r.is_empty() {
            continue;
        }
        println!(
            "{name}: n={} median ratio to ambient {:.2} p25 {:.2} p75 {:.2}",
            r.len(),
            r[r.len() / 2],
            r[r.len() / 4],
            r[r.len() * 3 / 4]
        );
    }
}
