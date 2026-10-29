//! Prints a sound script entry: parameters, waves (format and length) and
//! caption. Usage: `sndscript <name>`; a name that is not an entry lists the
//! entries containing it. Without arguments, prints totals.
use source_formats::captions::{Captions, plain_text};
use source_formats::fs::GameFileSystem;
use source_formats::soundscript::{Interval, SoundScripts};
use source_formats::wav::Wave;

fn range(i: Interval) -> String {
    if i.is_fixed() { format!("{}", i.start) } else { format!("{}..{}", i.start, i.end) }
}

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let scripts = SoundScripts::load(&fs).unwrap();
    let Some(name) = std::env::args().nth(1) else {
        println!(
            "{} entries from {} files; {} duplicate definitions ignored; {} missing files; {} warnings",
            scripts.len(),
            scripts.files.len(),
            scripts.duplicates.len(),
            scripts.missing_files.len(),
            scripts.warnings.len()
        );
        for w in &scripts.warnings {
            println!("  {w}");
        }
        return;
    };
    let Some(e) = scripts.get(&name) else {
        let needle = name.to_ascii_lowercase();
        let found: Vec<&str> = scripts.iter().filter(|e| e.name.to_ascii_lowercase().contains(&needle)).map(|e| e.name.as_str()).collect();
        println!("no entry {name:?}; {} entries contain it", found.len());
        for f in found.iter().take(50) {
            println!("  {f}");
        }
        return;
    };
    println!("{} ({}, version {})", e.name, e.file, e.version);
    println!(
        "  channel {:?}, volume {}, pitch {}, soundlevel {} dB{}{}",
        e.channel,
        range(e.volume),
        range(e.pitch),
        range(e.soundlevel),
        if e.compatibility { " (compatibility)" } else { "" },
        if e.preload { ", preload" } else { "" }
    );
    if e.delay_msec != 0 || e.play_to_owner_only {
        println!("  delay {} ms, owner only {}", e.delay_msec, e.play_to_owner_only);
    }
    for w in &e.waves {
        let Some(path) = w.file_path() else {
            println!("  wave {:?} (not a file)", w.raw);
            continue;
        };
        let info = match fs.read(&path).map(|d| Wave::parse_header(&d)) {
            Some(Ok(h)) => {
                format!("{:?} {} ch {} Hz {:.2} s, loop {:?}", h.codec, h.channels, h.rate, h.duration(), h.loop_start)
            }
            Some(Err(err)) => format!("unreadable: {err}"),
            None => "MISSING".to_owned(),
        };
        println!("  wave {:?} chars {:#05x}: {info}", w.raw, w.chars.bits());
    }
    if let Some(stacks) = &e.operator_stacks {
        for stack in stacks.children() {
            let ops: Vec<&str> = stack.children().iter().map(|op| op.key.as_str()).collect();
            println!("  {} {ops:?}", stack.key);
        }
    }
    for k in &e.extra {
        println!("  ignored key {:?}", k.key);
    }
    let captions = Captions::load(&fs, "english");
    match captions.lookup(&e.name) {
        Some(text) => println!("  caption {:?}", plain_text(&text)),
        None => println!("  no caption"),
    }
}
