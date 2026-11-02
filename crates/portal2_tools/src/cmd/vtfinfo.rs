//! Prints the header of textures from the installed game: size, format,
//! flags, frames, mip count and reflectivity, plus with `--stats` the mean
//! and maximum of each channel of the first mip (and the colour weighted by
//! alpha), and any particle sheet.
//! Pixel data is never echoed.
//! Usage: `vtfinfo [--stats] <material-relative name> [...]`, e.g.
//! `vtfinfo glass/refract_light_normal`.
use source_formats::vtf::Vtf;

fn stats(v: &Vtf) -> String {
    let mut sum = [0f64; 4];
    let mut max = [f32::MIN; 4];
    let mut n = 0usize;
    // Colour where alpha is high (a self-illumination mask's lit part).
    let (mut lit, mut lit_weight) = ([0f64; 3], 0f64);
    for face in 0..v.faces.max(1) {
        for p in v.decode_rgba_f32(0, 0, face) {
            for c in 0..4 {
                sum[c] += p[c] as f64;
                max[c] = max[c].max(p[c]);
            }
            for c in 0..3 {
                lit[c] += (p[c] * p[3]) as f64;
            }
            lit_weight += p[3] as f64;
            n += 1;
        }
    }
    let mean = sum.map(|s| s / n.max(1) as f64);
    let lit = lit.map(|s| s / lit_weight.max(1e-9));
    format!("mean={mean:.4?} max={max:.4?} alpha-weighted rgb={lit:.4?}")
}

pub fn main() {
    let fs = crate::game_fs();
    let mut args: Vec<String> = crate::args().skip(1).collect();
    let with_stats = args.first().is_some_and(|a| a == "--stats");
    if with_stats {
        args.remove(0);
    }
    for name in args {
        let path = format!("materials/{}.vtf", name.trim_end_matches(".vtf"));
        let Some(bytes) = fs.read(&path) else {
            println!("{path}: not found");
            continue;
        };
        match Vtf::parse(&bytes) {
            Ok(v) => {
                println!(
                    "{path}: {}x{} {:?} flags={:#x} frames={} mips={} cube={} reflectivity={:?}",
                    v.width,
                    v.height,
                    v.format,
                    v.flags,
                    v.frames,
                    v.mip_count,
                    v.is_cubemap(),
                    v.reflectivity
                );
                if with_stats {
                    println!("  {}", stats(&v));
                }
                if let Some(sheet) = &v.sheet {
                    let seqs: Vec<String> = sheet
                        .sequences
                        .iter()
                        .map(|s| format!("{}:{}f/{}t/flags{}", s.number, s.frames.len(), s.total_time, s.flags))
                        .collect();
                    println!("  sheet: {}", seqs.join(" "));
                }
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}
