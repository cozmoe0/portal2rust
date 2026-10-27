//! Writes one frame of a Bink video as an RGBA PNG.
//!
//! `cargo run -p bink --example bik2png -- <file.bik> <frame> <out.png>`
use std::error::Error;
use std::fs::File;
use std::io::BufWriter;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, frame, output] = args.as_slice() else {
        eprintln!("usage: bik2png <file.bik> <frame> <out.png>");
        std::process::exit(2);
    };
    let mut video = bink::Bink::open(std::fs::read(input)?)?;
    video.seek(frame.parse()?)?;
    let frame = video.next_frame()?;
    let (w, h) = (frame.width(), frame.height());
    let mut rgba = vec![0; w as usize * h as usize * 4];
    frame.to_rgba(&mut rgba);

    let mut png = png::Encoder::new(BufWriter::new(File::create(output)?), w, h);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()?.write_image_data(&rgba)?;

    println!(
        "{input}: BIK{rev} {w}x{h} {fps:.3} fps, {frames} frames, {tracks} audio tracks; \
         frame {n} ({kind}) -> {output}",
        rev = video.header().revision as char,
        fps = video.fps(),
        frames = video.frame_count(),
        tracks = video.audio_tracks().len(),
        n = video.position() - 1,
        kind = if video.is_keyframe(video.position() - 1) { "keyframe" } else { "inter" },
    );
    Ok(())
}
