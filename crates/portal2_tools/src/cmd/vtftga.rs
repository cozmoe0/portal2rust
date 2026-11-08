//! Writes the first mip of a texture from the installed game to an
//! uncompressed TGA (any image viewer opens it), to look at what a material
//! really shows. Usage: `vtftga <material-relative name> <out.tga> [frame]`,
//! e.g. `vtftga effects/projected_wall wall.tga`.
use source_formats::vtf::Vtf;

pub fn main() {
    let mut args = crate::args().skip(1);
    let (Some(name), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: vtftga <material-relative name> <out.tga> [frame]");
        std::process::exit(1)
    };
    let frame: u32 = args.next().and_then(|f| f.parse().ok()).unwrap_or(0);
    let path = format!("materials/{}.vtf", name.trim_end_matches(".vtf"));
    let Some(data) = crate::game_fs().read(&path) else {
        eprintln!("{path}: not found");
        std::process::exit(1)
    };
    let vtf = Vtf::parse(&data).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1)
    });
    let (w, h, _) = vtf.mip_dims(0);
    let rgba = vtf.decode_rgba8(0, frame.min(vtf.frames.saturating_sub(1)), 0);
    // 18-byte header: uncompressed true colour, 32 bits, origin top left.
    let mut tga = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    tga.extend((w as u16).to_le_bytes());
    tga.extend((h as u16).to_le_bytes());
    tga.extend([32, 0x28]);
    for p in rgba.as_chunks::<4>().0 {
        tga.extend([p[2], p[1], p[0], p[3]]);
    }
    std::fs::write(&out, tga).expect("write TGA");
    println!("{path}: {w}x{h} -> {out}");
}
