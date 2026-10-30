//! Prints format, length, loop points and metadata of sound files.
//! Usage: `wavinfo [path ...]` with paths under `sound/` (the prefix is
//! optional); without arguments, summarises every `.wav` in the installation.
use std::collections::BTreeMap;

use source_formats::wav::{Codec, SentenceData, Wave};

fn describe(w: &Wave) -> String {
    let codec = match (w.codec, w.mp3) {
        (Codec::Mp3, Some(m)) => format!("MPEG-{} layer {} {}-{} kbit/s", m.version, m.layer, m.kbps.0, m.kbps.1),
        (Codec::Pcm, _) => format!("PCM {}-bit", w.bits),
        (c, _) => format!("{c:?}"),
    };
    format!("{codec}, {} ch, {} Hz", w.channels, w.rate)
}

pub fn main() {
    let fs = crate::game_fs();
    let args: Vec<String> = crate::args().skip(1).collect();
    if args.is_empty() {
        let mut formats = BTreeMap::<String, (usize, f64)>::new();
        let (mut looped, mut failed) = (0, 0);
        for p in fs.list("sound", Some("wav"), true) {
            match fs.read(&p).map(|d| Wave::parse_header(&d)) {
                Some(Ok(w)) => {
                    let e = formats.entry(describe(&w)).or_default();
                    e.0 += 1;
                    e.1 += w.duration() as f64;
                    looped += w.loop_start.is_some() as usize;
                }
                Some(Err(e)) => {
                    failed += 1;
                    println!("{p}: {e}");
                }
                None => failed += 1,
            }
        }
        for (k, (n, secs)) in formats {
            println!("{n:6} files {:7.1} min  {k}", secs / 60.0);
        }
        println!("{looped} looped, {failed} failed");
        return;
    }
    for arg in args {
        let path = if arg.to_ascii_lowercase().starts_with("sound") { arg } else { format!("sound/{arg}") };
        let w = match Wave::load(&fs, &path) {
            Ok(w) => w,
            Err(e) => {
                println!("{path}: {e}");
                continue;
            }
        };
        println!("{path} [{}]: {}, {} frames, {:.3} s", fs.locate(&path).unwrap_or_default(), describe(&w), w.frames, w.duration());
        if let Some(m) = w.mp3 {
            println!("  {} MPEG frames of {} samples, crc {}, {} trailing bytes", m.frame_count, m.samples_per_frame, m.crc, m.trailing);
        }
        if !w.samples.is_empty() {
            let peak = w.samples.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0) as f64 / 32768.0;
            let rms = (w.samples.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum::<f64>() / w.samples.len() as f64).sqrt();
            println!("  peak {:.1} dBFS, rms {:.1} dBFS", 20.0 * peak.max(1e-9).log10(), 20.0 * rms.max(1e-9).log10());
        }
        if w.loop_start.is_some() || w.loop_end.is_some() {
            println!("  loop start {:?}, smpl end {:?}", w.loop_start, w.loop_end);
        }
        for c in &w.cues {
            println!("  cue {} at sample {}", c.id, c.sample_offset);
        }
        for l in &w.labels {
            println!("  {} for cue {}: {:?} length {:?}", String::from_utf8_lossy(&l.kind), l.cue, l.text, l.length);
        }
        match &w.sentence {
            Some(SentenceData::Parsed(s)) => println!(
                "  sentence v{}: {} words, {} emphasis samples, options {:?}, text {:?}",
                s.version,
                s.words.len(),
                s.emphasis.len(),
                s.options,
                s.text
            ),
            Some(SentenceData::Raw(b)) => println!("  sentence: {} raw bytes", b.len()),
            None => {}
        }
        if !w.other_chunks.is_empty() {
            let ids: Vec<String> = w.other_chunks.iter().map(|id| String::from_utf8_lossy(id).into_owned()).collect();
            println!("  skipped chunks {ids:?}");
        }
    }
}
