//! Decodes a `.vfont` and lists its TrueType tables: `vfontinfo <path>`.
use source_formats::fs::GameFileSystem;

fn main() {
    let path = std::env::args().nth(1).expect("usage: vfontinfo <path>");
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let data = fs.read(&path).expect("not found");
    println!("{} bytes, tail {:?}", data.len(), String::from_utf8_lossy(&data[data.len().saturating_sub(8)..]));
    let Some(font) = source_formats::vfont::decode(&data) else {
        println!("not a vfont");
        return;
    };
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    println!("decoded {} bytes, {n} tables", font.len());
    for i in 0..n {
        let r = &font[12 + i * 16..28 + i * 16];
        let tag = String::from_utf8_lossy(&r[0..4]).into_owned();
        let off = u32::from_be_bytes([r[8], r[9], r[10], r[11]]);
        let len = u32::from_be_bytes([r[12], r[13], r[14], r[15]]);
        let fits = (off as usize + len as usize) <= font.len();
        println!("  {tag} off {off} len {len}{}", if fits { "" } else { "  (past end)" });
    }
}
